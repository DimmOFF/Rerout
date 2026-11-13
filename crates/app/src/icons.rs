//! Icons of executables (like in Explorer), cached as egui textures.

use eframe::egui;
use rerout_common::norm_path;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// RGBA pixels of the file's large icon.
fn extract(path: &str) -> Option<(Vec<u8>, usize, usize)> {
    unsafe {
        let mut sfi: SHFILEINFOW = std::mem::zeroed();
        let r = SHGetFileInfoW(
            wide(path).as_ptr(),
            0,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if r == 0 || sfi.hIcon.is_null() {
            return None;
        }
        let hicon = sfi.hIcon;
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut ii) == 0 {
            DestroyIcon(hicon);
            return None;
        }

        let result = (|| -> Option<(Vec<u8>, usize, usize)> {
            let mut bm: BITMAP = std::mem::zeroed();
            let got = GetObjectW(
                ii.hbmColor,
                std::mem::size_of::<BITMAP>() as i32,
                &mut bm as *mut BITMAP as *mut c_void,
            );
            if got == 0 {
                return None;
            }
            let (w, h) = (bm.bmWidth, bm.bmHeight);
            if w <= 0 || h <= 0 || w > 256 || h > 256 {
                return None;
            }

            let hdc = CreateCompatibleDC(null_mut());
            let read = |bmp: *mut c_void| -> Option<Vec<u8>> {
                let mut bi: BITMAPINFO = std::mem::zeroed();
                bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bi.bmiHeader.biWidth = w;
                bi.bmiHeader.biHeight = -h; // top-down
                bi.bmiHeader.biPlanes = 1;
                bi.bmiHeader.biBitCount = 32;
                bi.bmiHeader.biCompression = BI_RGB;
                let mut buf = vec![0u8; (w * h * 4) as usize];
                let n = GetDIBits(hdc, bmp, 0, h as u32, buf.as_mut_ptr() as *mut c_void, &mut bi, DIB_RGB_COLORS);
                if n == 0 {
                    None
                } else {
                    Some(buf)
                }
            };
            let color = read(ii.hbmColor);
            let mask = if ii.hbmMask.is_null() { None } else { read(ii.hbmMask) };
            DeleteDC(hdc);

            let mut px = color?;
            // Old icons have no alpha channel: take transparency from the mask
            let has_alpha = px.chunks(4).any(|c| c[3] != 0);
            for (i, c) in px.chunks_mut(4).enumerate() {
                c.swap(0, 2); // BGRA -> RGBA
                if !has_alpha {
                    let opaque = mask
                        .as_ref()
                        .map(|m| m[i * 4] == 0 && m[i * 4 + 1] == 0 && m[i * 4 + 2] == 0)
                        .unwrap_or(true);
                    c[3] = if opaque { 255 } else { 0 };
                }
            }
            Some((px, w as usize, h as usize))
        })();

        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        DestroyIcon(hicon);
        result
    }
}

#[derive(Default)]
pub struct IconCache {
    map: HashMap<String, Option<egui::TextureHandle>>,
}

impl IconCache {
    fn texture(&mut self, ctx: &egui::Context, path: &str) -> Option<&egui::TextureHandle> {
        let key = norm_path(path);
        if !self.map.contains_key(&key) {
            let tex = extract(path).map(|(rgba, w, h)| {
                ctx.load_texture(
                    format!("icon:{key}"),
                    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba),
                    egui::TextureOptions::LINEAR,
                )
            });
            self.map.insert(key.clone(), tex);
        }
        self.map.get(&key).and_then(|o| o.as_ref())
    }

    /// Draw the icon of `path` (or a neutral placeholder) at `size` x `size`.
    pub fn show(&mut self, ui: &mut egui::Ui, path: &str, size: f32) {
        let ctx = ui.ctx().clone();
        match self.texture(&ctx, path) {
            Some(t) => {
                ui.add(egui::Image::new(egui::load::SizedTexture::new(t.id(), egui::vec2(size, size))));
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                ui.painter().rect_filled(rect, 6.0, egui::Color32::from_rgb(55, 60, 75));
            }
        }
    }
}
