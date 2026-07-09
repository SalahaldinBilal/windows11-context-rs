//! Generated-icon support: extracts a config's icon into a local multi-size
//! .ico (a stable snapshot), optionally compositing the system UAC shield into
//! the bottom-right corner. Only ctxmenu-setup calls this (never the DLL — it
//! runs in Explorer).
#![cfg(windows)]

use crate::config::Corner;
use crate::setup::parse_icon_spec;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
};
use windows::Win32::UI::Shell::{
    SHDefExtractIconW, SHGetStockIconInfo, SHGSI_ICONLOCATION, SHSTOCKICONINFO, SIID_SHIELD,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfoExW, HICON, ICONINFOEXW};

const SIZES: [i32; 6] = [16, 20, 24, 32, 48, 64];

struct Image {
    size: i32,
    /// Top-down straight-alpha BGRA.
    px: Vec<u8>,
}

fn shield_location() -> Option<(String, i32)> {
    let mut info = SHSTOCKICONINFO {
        cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe { SHGetStockIconInfo(SIID_SHIELD, SHGSI_ICONLOCATION, &mut info) }.ok()?;
    let len = info.szPath.iter().position(|&c| c == 0).unwrap_or(0);
    let path = String::from_utf16_lossy(&info.szPath[..len]);
    if path.is_empty() {
        None
    } else {
        Some((path, info.iIcon))
    }
}

fn extract_icon(path: &str, index: i32, size: i32) -> Option<HICON> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hicon = HICON::default();
    let hr = unsafe {
        SHDefExtractIconW(PCWSTR(wide.as_ptr()), index, 0, Some(&mut hicon), None, size as u32)
    };
    if hr.is_ok() && !hicon.is_invalid() {
        Some(hicon)
    } else {
        None
    }
}

fn icon_to_bgra(hicon: HICON, size: i32) -> Option<Vec<u8>> {
    unsafe {
        let mut ii = ICONINFOEXW {
            cbSize: std::mem::size_of::<ICONINFOEXW>() as u32,
            ..Default::default()
        };
        if !GetIconInfoExW(hicon, &mut ii).as_bool() {
            return None;
        }
        let hdc = GetDC(None);
        let read = |hbm| -> Option<Vec<u8>> {
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (size * size * 4) as usize];
            let n = GetDIBits(
                hdc,
                hbm,
                0,
                size as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            (n > 0).then_some(buf)
        };
        let color = (!ii.hbmColor.is_invalid()).then(|| read(ii.hbmColor)).flatten();
        let mask = (!ii.hbmMask.is_invalid()).then(|| read(ii.hbmMask)).flatten();
        ReleaseDC(None, hdc);
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask.into());
        }

        let mut px = color?;
        // Icons without an alpha channel come back all-zero: derive it from the
        // monochrome AND mask (white = transparent) or treat as opaque.
        if px.chunks_exact(4).all(|c| c[3] == 0) {
            match &mask {
                Some(m) => {
                    for (c, mk) in px.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
                        c[3] = if mk[0] == 0 { 255 } else { 0 };
                    }
                }
                None => px.chunks_exact_mut(4).for_each(|c| c[3] = 255),
            }
        }
        Some(px)
    }
}

fn extract_bgra(path: &str, index: i32, size: i32) -> Option<Vec<u8>> {
    let hicon = extract_icon(path, index, size)?;
    let px = icon_to_bgra(hicon, size);
    let _ = unsafe { DestroyIcon(hicon) };
    px
}

/// Straight-alpha "over" of `src` onto one corner of `dst`.
fn over_at(dst: &mut [u8], dst_size: i32, src: &[u8], src_size: i32, corner: Corner) {
    let off = dst_size - src_size;
    let (ox, oy) = match corner {
        Corner::TopLeft => (0, 0),
        Corner::TopRight => (off, 0),
        Corner::BottomLeft => (0, off),
        Corner::BottomRight => (off, off),
    };
    for y in 0..src_size {
        for x in 0..src_size {
            let s = &src[((y * src_size + x) * 4) as usize..][..4];
            let sa = s[3] as u32;
            if sa == 0 {
                continue;
            }
            let di = (((y + oy) * dst_size + (x + ox)) * 4) as usize;
            let d = &mut dst[di..di + 4];
            if sa == 255 || d[3] == 0 {
                d.copy_from_slice(s);
                continue;
            }
            for i in 0..3 {
                d[i] = ((s[i] as u32 * sa + d[i] as u32 * (255 - sa)) / 255) as u8;
            }
            d[3] = (sa + d[3] as u32 * (255 - sa) / 255) as u8;
        }
    }
}

/// 32bpp BMP-encoded .ico (BITMAPINFOHEADER + bottom-up BGRA + AND mask).
fn write_ico(images: &[Image], dest: &Path) -> std::io::Result<()> {
    let mut entries: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut offset = 6 + 16 * images.len();
    for img in images {
        let s = img.size as usize;
        let mask_row = s.div_ceil(32) * 4;
        let mut d = Vec::with_capacity(40 + s * s * 4 + mask_row * s);
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&img.size.to_le_bytes());
        d.extend_from_slice(&(img.size * 2).to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&((s * s * 4 + mask_row * s) as u32).to_le_bytes());
        d.extend_from_slice(&[0u8; 16]);
        for y in (0..s).rev() {
            d.extend_from_slice(&img.px[y * s * 4..(y + 1) * s * 4]);
        }
        for y in (0..s).rev() {
            let mut row = vec![0u8; mask_row];
            for x in 0..s {
                if img.px[(y * s + x) * 4 + 3] < 128 {
                    row[x / 8] |= 0x80 >> (x % 8);
                }
            }
            d.extend_from_slice(&row);
        }
        entries.push(s as u8);
        entries.push(s as u8);
        entries.extend_from_slice(&[0, 0]);
        entries.extend_from_slice(&1u16.to_le_bytes());
        entries.extend_from_slice(&32u16.to_le_bytes());
        entries.extend_from_slice(&(d.len() as u32).to_le_bytes());
        entries.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += d.len();
        data.extend_from_slice(&d);
    }
    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    out.extend_from_slice(&entries);
    out.extend_from_slice(&data);
    std::fs::write(dest, out)
}

/// Write `dest` as a multi-size .ico of `base_spec` ("path,index", already
/// env-expanded). `badge` composites a half-size icon into the given corner —
/// spec "uac" resolves to the system UAC shield; when the base is empty or
/// can't be loaded the badge renders alone at full size. Without a badge a
/// plain local copy is made, and an unloadable base returns false.
pub fn generate_icon(base_spec: &str, dest: &Path, badge: Option<(&str, Corner)>) -> bool {
    let badge_loc = match badge {
        Some((spec, corner)) => {
            let loc = if spec.eq_ignore_ascii_case("uac") {
                shield_location()
            } else {
                Some(parse_icon_spec(spec))
            };
            let Some((path, index)) = loc else {
                return false;
            };
            Some((path, index, corner))
        }
        None => None,
    };
    let base = {
        let spec = base_spec.trim();
        (!spec.is_empty()).then(|| parse_icon_spec(spec))
    };
    if base.is_none() && badge_loc.is_none() {
        return false;
    }
    let mut images = Vec::new();
    for &size in &SIZES {
        let canvas = base
            .as_ref()
            .and_then(|(path, index)| extract_bgra(path, *index, size));
        let (mut px, badge_size) = match canvas {
            Some(px) => (px, (size / 2).max(8)),
            None if badge_loc.is_some() => (vec![0u8; (size * size * 4) as usize], size),
            None => continue,
        };
        if let Some((path, index, corner)) = &badge_loc {
            match extract_bgra(path, *index, badge_size) {
                Some(b) => over_at(&mut px, size, &b, badge_size, *corner),
                None if base.is_none() => continue,
                None => {}
            }
        }
        images.push(Image { size, px });
    }
    !images.is_empty() && write_ico(&images, dest).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Corner;

    #[test]
    #[ignore = "needs a desktop session with GDI; run manually via: cargo test -- --ignored"]
    fn generates_badged_icons() {
        let dir = std::env::temp_dir();
        let base = crate::exec::expand_env(r"%SystemRoot%\system32\notepad.exe,0");
        assert!(generate_icon(&base, &dir.join("ctx_copy.ico"), None));
        assert!(generate_icon(&base, &dir.join("ctx_uac_tl.ico"), Some(("uac", Corner::TopLeft))));
        let folder = crate::exec::expand_env(r"%SystemRoot%\system32\shell32.dll,3");
        assert!(generate_icon(
            &base,
            &dir.join("ctx_badge_bl.ico"),
            Some((folder.as_str(), Corner::BottomLeft))
        ));
        assert!(generate_icon("", &dir.join("ctx_uac_only.ico"), Some(("uac", Corner::BottomRight))));
        assert!(!generate_icon(r"C:\missing\nope.exe,0", &dir.join("ctx_none.ico"), None));
    }
}
