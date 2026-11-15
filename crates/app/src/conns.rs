//! TCP/UDP socket tables (IPv4 and IPv6) by PID; the interface of a socket is
//! derived from its local address.

use rerout_common::adapters::Adapter;
use std::collections::HashMap;
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, GetExtendedUdpTable, SetTcpEntry};

pub struct RawConn {
    pub pid: u32,
    pub proto: &'static str,
    pub local: IpAddr,
    pub lport: u16,
    pub remote: Option<(IpAddr, u16)>,
    pub state: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// socket is on an allowed interface
    Allowed,
    /// socket is on an interface that is not allowed
    Denied,
    /// cannot tell (0.0.0.0 / ::)
    Unbound,
    /// loopback
    Local,
}

/// Endpoints of an IPv4 TCP connection (what SetTcpEntry needs to identify it).
#[derive(Clone, Copy)]
pub struct Target {
    pub local: Ipv4Addr,
    pub lport: u16,
    pub remote: Ipv4Addr,
    pub rport: u16,
}

#[derive(Clone)]
pub struct ConnRow {
    pub proc: String,
    pub proto: &'static str,
    pub local: String,
    pub remote: String,
    pub state: String,
    pub iface: String,
    pub verdict: Verdict,
    /// LISTEN and loopback rows are hidden by default
    pub hideable: bool,
    /// set for IPv4 TCP connections that can be closed
    pub target: Option<Target>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The port is stored in the low 16 bits in network byte order.
fn port(v: u32) -> u16 {
    u16::from_be((v & 0xFFFF) as u16)
}

fn v4(b: &[u8], o: usize) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(b[o], b[o + 1], b[o + 2], b[o + 3]))
}

fn v6(b: &[u8], o: usize) -> IpAddr {
    let mut oct = [0u8; 16];
    oct.copy_from_slice(&b[o..o + 16]);
    IpAddr::V6(Ipv6Addr::from(oct))
}

fn read_table(call: impl Fn(*mut c_void, *mut u32) -> u32) -> Vec<u8> {
    let mut size: u32 = 0;
    call(std::ptr::null_mut(), &mut size); // learn the required size
    for _ in 0..4 {
        let mut buf = vec![0u8; size as usize + 4096];
        let mut sz = buf.len() as u32;
        let ret = call(buf.as_mut_ptr() as *mut c_void, &mut sz);
        if ret == 0 {
            return buf;
        }
        if ret != 122 {
            break; // not ERROR_INSUFFICIENT_BUFFER
        }
        size = sz;
    }
    Vec::new()
}

fn tcp_state(s: u32) -> &'static str {
    match s {
        1 => "CLOSED",
        2 => "LISTEN",
        3 => "SYN_SENT",
        4 => "SYN_RCVD",
        5 => "ESTABLISHED",
        6 => "FIN_WAIT1",
        7 => "FIN_WAIT2",
        8 => "CLOSE_WAIT",
        9 => "CLOSING",
        10 => "LAST_ACK",
        11 => "TIME_WAIT",
        _ => "?",
    }
}

pub fn fmt_addr(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(a) => format!("{a}:{port}"),
        IpAddr::V6(a) => format!("[{a}]:{port}"),
    }
}

/// All TCP/UDP sockets of the system with their owner PID.
pub fn raw_all() -> Vec<RawConn> {
    let mut out: Vec<RawConn> = Vec::new();

    // TCP IPv4: 24-byte rows
    let b = read_table(|p, s| unsafe { GetExtendedTcpTable(p, s, 0, 2, 5, 0) });
    if b.len() >= 4 {
        for i in 0..u32_at(&b, 0) as usize {
            let o = 4 + i * 24;
            if o + 24 > b.len() {
                break;
            }
            out.push(RawConn {
                pid: u32_at(&b, o + 20),
                proto: "TCP",
                local: v4(&b, o + 4),
                lport: port(u32_at(&b, o + 8)),
                remote: Some((v4(&b, o + 12), port(u32_at(&b, o + 16)))),
                state: tcp_state(u32_at(&b, o)),
            });
        }
    }

    // TCP IPv6: 56-byte rows
    let b = read_table(|p, s| unsafe { GetExtendedTcpTable(p, s, 0, 23, 5, 0) });
    if b.len() >= 4 {
        for i in 0..u32_at(&b, 0) as usize {
            let o = 4 + i * 56;
            if o + 56 > b.len() {
                break;
            }
            out.push(RawConn {
                pid: u32_at(&b, o + 52),
                proto: "TCP6",
                local: v6(&b, o),
                lport: port(u32_at(&b, o + 20)),
                remote: Some((v6(&b, o + 24), port(u32_at(&b, o + 44)))),
                state: tcp_state(u32_at(&b, o + 48)),
            });
        }
    }

    // UDP IPv4: 12-byte rows
    let b = read_table(|p, s| unsafe { GetExtendedUdpTable(p, s, 0, 2, 1, 0) });
    if b.len() >= 4 {
        for i in 0..u32_at(&b, 0) as usize {
            let o = 4 + i * 12;
            if o + 12 > b.len() {
                break;
            }
            out.push(RawConn {
                pid: u32_at(&b, o + 8),
                proto: "UDP",
                local: v4(&b, o),
                lport: port(u32_at(&b, o + 4)),
                remote: None,
                state: "-",
            });
        }
    }

    // UDP IPv6: 28-byte rows
    let b = read_table(|p, s| unsafe { GetExtendedUdpTable(p, s, 0, 23, 1, 0) });
    if b.len() >= 4 {
        for i in 0..u32_at(&b, 0) as usize {
            let o = 4 + i * 28;
            if o + 28 > b.len() {
                break;
            }
            out.push(RawConn {
                pid: u32_at(&b, o + 24),
                proto: "UDP6",
                local: v6(&b, o),
                lport: port(u32_at(&b, o + 20)),
                remote: None,
                state: "-",
            });
        }
    }
    out
}

/// The adapter that owns this local address.
pub fn adapter_for<'a>(ip: IpAddr, adapters: &'a [Adapter]) -> Option<&'a Adapter> {
    let s = ip.to_string();
    adapters
        .iter()
        .find(|a| a.ipv4.iter().any(|x| *x == s) || a.ipv6.iter().any(|x| *x == s))
}

/// Rows for the given processes. `allowed`: GUIDs the rule allows, or None when
/// the rule is not enforced (disabled / not configured): then nothing is "denied".
pub fn collect(
    pids: &HashMap<u32, String>,
    adapters: &[Adapter],
    allowed: Option<&[String]>,
) -> Vec<ConnRow> {
    let mut rows: Vec<ConnRow> = Vec::new();

    for c in raw_all() {
        let Some(name) = pids.get(&c.pid) else { continue };

        let (iface, verdict) = if c.local.is_unspecified() {
            ("not determined".to_string(), Verdict::Unbound)
        } else if c.local.is_loopback() {
            ("loopback".to_string(), Verdict::Local)
        } else {
            match adapter_for(c.local, adapters) {
                Some(a) => {
                    let ok = match allowed {
                        None => true,
                        Some(list) => list.iter().any(|g| g.eq_ignore_ascii_case(&a.guid)),
                    };
                    (a.name.clone(), if ok { Verdict::Allowed } else { Verdict::Denied })
                }
                None => ("unknown".to_string(), Verdict::Denied),
            }
        };

        let target = match (c.proto, c.remote, c.local) {
            ("TCP", Some((IpAddr::V4(r), rport)), IpAddr::V4(l)) if closable_state(c.state) => {
                Some(Target { local: l, lport: c.lport, remote: r, rport })
            }
            _ => None,
        };

        rows.push(ConnRow {
            proc: format!("{name} ({})", c.pid),
            proto: c.proto,
            local: fmt_addr(c.local, c.lport),
            remote: c
                .remote
                .map(|(a, p)| fmt_addr(a, p))
                .unwrap_or_else(|| "-".into()),
            hideable: c.state == "LISTEN" || verdict == Verdict::Local,
            state: c.state.to_string(),
            iface,
            verdict,
            target,
        });
    }

    // Problems first
    let rank = |v: Verdict| match v {
        Verdict::Denied => 0,
        Verdict::Allowed => 1,
        Verdict::Unbound => 2,
        Verdict::Local => 3,
    };
    rows.sort_by(|a, b| {
        rank(a.verdict)
            .cmp(&rank(b.verdict))
            .then_with(|| a.proc.cmp(&b.proc))
            .then_with(|| a.remote.cmp(&b.remote))
    });
    rows
}

fn closable_state(s: &str) -> bool {
    matches!(
        s,
        "ESTABLISHED" | "SYN_SENT" | "SYN_RCVD" | "CLOSE_WAIT" | "FIN_WAIT1" | "FIN_WAIT2" | "LAST_ACK" | "CLOSING"
    )
}

/// Abort an IPv4 TCP connection (SetTcpEntry with the DELETE_TCB state). The application
/// sees a reset and normally reconnects; the new connection follows the current rule.
/// Windows only offers this for IPv4 and only with administrator rights.
pub fn close_tcp4(t: Target) -> Result<(), String> {
    #[repr(C)]
    struct Row {
        state: u32,
        local_addr: u32,
        local_port: u32,
        remote_addr: u32,
        remote_port: u32,
    }
    let row = Row {
        state: 12, // MIB_TCP_STATE_DELETE_TCB
        local_addr: u32::from_le_bytes(t.local.octets()),
        local_port: t.lport.swap_bytes() as u32,
        remote_addr: u32::from_le_bytes(t.remote.octets()),
        remote_port: t.rport.swap_bytes() as u32,
    };
    let r = unsafe { SetTcpEntry(&row as *const Row as *const _) };
    match r {
        0 => Ok(()),
        5 => Err("access denied - run Rerout as administrator".into()),
        e => Err(format!("Windows error {e} (the connection may already be gone)")),
    }
}
