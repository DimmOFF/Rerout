use std::ffi::{c_char, CStr};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
};

const ERROR_BUFFER_OVERFLOW: u32 = 111;
const IF_TYPE_LOOPBACK: u32 = 24;
const IF_OPER_STATUS_UP: i32 = 1;

#[derive(Clone, Debug)]
pub struct Adapter {
    pub guid: String,
    pub name: String,
    pub description: String,
    pub if_index: u32,
    pub if_index6: u32,
    pub if_type: u32,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub up: bool,
}

impl Adapter {
    /// Human-friendly kind of the interface.
    pub fn kind(&self) -> &'static str {
        let d = self.description.to_lowercase();
        if d.contains("vpn") || d.contains("wireguard") || d.contains("wintun") || d.contains("tap-") || d.contains("openvpn") {
            return "VPN";
        }
        match self.if_type {
            6 => "Ethernet",
            71 => "Wi-Fi",
            243 | 244 => "Mobile",
            131 | 53 | 1 => "Virtual",
            23 => "VPN",
            _ => "Other",
        }
    }

    pub fn display(&self) -> String {
        if self.ipv4.is_empty() {
            self.name.clone()
        } else {
            format!("{} — {}", self.name, self.ipv4.join(", "))
        }
    }
}

unsafe fn wstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

pub fn list() -> Vec<Adapter> {
    let mut out = Vec::new();
    unsafe {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 16 * 1024;
        let mut buf: Vec<u64>; // u64 -> выравнивание по 8 байт
        let mut tries = 0;
        loop {
            buf = vec![0u64; (size as usize + 7) / 8];
            let ret = GetAdaptersAddresses(
                0, // AF_UNSPEC
                flags,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            );
            if ret == 0 {
                break;
            }
            tries += 1;
            if ret != ERROR_BUFFER_OVERFLOW || tries > 4 {
                return out;
            }
        }

        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            p = a.Next;

            if a.IfType == IF_TYPE_LOOPBACK || a.AdapterName.is_null() {
                continue;
            }
            let guid = CStr::from_ptr(a.AdapterName as *const c_char)
                .to_string_lossy()
                .into_owned();

            let mut ipv4 = Vec::new();
            let mut ipv6 = Vec::new();
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                let ua = &*u;
                let sa = ua.Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family == 2 {
                    let d = &(*sa).sa_data;
                    ipv4.push(format!(
                        "{}.{}.{}.{}",
                        d[2] as u8, d[3] as u8, d[4] as u8, d[5] as u8
                    ));
                } else if !sa.is_null()
                    && ua.Address.iSockaddrLength >= 24
                    && (*sa).sa_family == 23
                {
                    // SOCKADDR_IN6: адрес лежит со смещения 8
                    let b = std::slice::from_raw_parts(sa as *const u8, 24);
                    let mut oct = [0u8; 16];
                    oct.copy_from_slice(&b[8..24]);
                    ipv6.push(std::net::Ipv6Addr::from(oct).to_string());
                }
                u = ua.Next;
            }

            out.push(Adapter {
                guid,
                name: wstr(a.FriendlyName),
                description: wstr(a.Description),
                if_index: a.Anonymous1.Anonymous.IfIndex,
                if_index6: a.Ipv6IfIndex,
                if_type: a.IfType,
                ipv4,
                ipv6,
                up: a.OperStatus == IF_OPER_STATUS_UP,
            });
        }
    }
    out.sort_by(|a, b| b.up.cmp(&a.up).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Friendly name of an adapter that is NOT present right now. Windows keeps it in the
/// registry for adapters it has seen before (disabled, unplugged, tethering phones...).
pub fn registry_name(guid: &str) -> Option<String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
    };

    let path = format!(
        r"SYSTEM\CurrentControlSet\Control\Network\{{4D36E972-E325-11CE-BFC1-08002BE10318}}\{guid}\Connection"
    );
    let wpath: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = "Name\0".encode_utf16().collect();

    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, wpath.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != 0 {
            return None;
        }
        let mut size: u32 = 0;
        let mut ty: u32 = 0;
        let r = RegQueryValueExW(key, value.as_ptr(), null(), &mut ty, null_mut(), &mut size);
        let mut result = None;
        if r == 0 && ty == 1 && size >= 2 {
            // ty == 1: REG_SZ
            let mut buf = vec![0u16; (size as usize + 1) / 2];
            let mut sz = (buf.len() * 2) as u32;
            let r2 = RegQueryValueExW(key, value.as_ptr(), null(), null_mut(), buf.as_mut_ptr() as *mut u8, &mut sz);
            if r2 == 0 {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                result = Some(String::from_utf16_lossy(&buf[..n]));
            }
        }
        RegCloseKey(key);
        result.filter(|s| !s.is_empty())
    }
}
