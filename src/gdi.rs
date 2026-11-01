use std::ffi::c_void;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};

pub struct Dib {
    pub dc: HDC,
    pub bmp: HBITMAP,
    pub bits: *mut u8,
    pub w: i32,
    pub h: i32,
    old: HGDIOBJ,
}

impl Dib {
    pub fn new(screen: HDC, bgra: &[u8], w: i32, h: i32) -> Result<Self, String> {
        if w < 1 || h < 1 || bgra.len() < (w as usize) * (h as usize) * 4 {
            return Err("image vide".into());
        }
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..BITMAPINFOHEADER::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(
                screen,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bmp.is_null() || bits.is_null() {
                return Err("DIB".into());
            }
            let bits = bits as *mut u8;
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits, (w * h * 4) as usize);
            let dc = CreateCompatibleDC(screen);
            if dc.is_null() {
                DeleteObject(bmp);
                return Err("DC".into());
            }
            let old = SelectObject(dc, bmp);
            Ok(Self {
                dc,
                bmp,
                bits,
                w,
                h,
                old,
            })
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_null() {
                SelectObject(self.dc, self.old);
                DeleteDC(self.dc);
                self.dc = std::ptr::null_mut();
            }
            if !self.bmp.is_null() {
                DeleteObject(self.bmp);
                self.bmp = std::ptr::null_mut();
            }
        }
    }
}

pub fn bgr(r: u8, g: u8, b: u8) -> u32 {
    (b as u32) | ((g as u32) << 8) | ((r as u32) << 16)
}
