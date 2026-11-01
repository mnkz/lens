use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, GetWindowRect, IsIconic, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::pixels::{self, PxRect};
use crate::util::log_line;

pub struct Shot {
    pub bgra: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub origin_x: i32,
    pub origin_y: i32,
}

pub fn ensure_dpi() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn capture() -> Result<Shot, String> {
    ensure_dpi();
    unsafe {
        let origin_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let origin_y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width < 1 || height < 1 || width > 20000 || height > 20000 {
            return Err("taille d'écran illisible".into());
        }
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("GetDC".into());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
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
        let mem = CreateCompatibleDC(screen);
        if bmp.is_null() || mem.is_null() || bits.is_null() {
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err("DIB".into());
        }
        let old = SelectObject(mem, bmp);
        let ok = BitBlt(
            mem,
            0,
            0,
            width,
            height,
            screen,
            origin_x,
            origin_y,
            SRCCOPY | CAPTUREBLT,
        );
        let len = (width as usize) * (height as usize) * 4;
        let mut bgra = vec![0u8; len];
        std::ptr::copy_nonoverlapping(bits as *const u8, bgra.as_mut_ptr(), len);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if ok == 0 {
            return Err("BitBlt".into());
        }
        pixels::force_opaque(&mut bgra);
        log_line(&format!("capture {width}×{height} @ {origin_x},{origin_y}"));
        Ok(Shot {
            bgra,
            width,
            height,
            origin_x,
            origin_y,
        })
    }
}

pub fn window_image_rect(hwnd: HWND, shot: &Shot) -> Option<PxRect> {
    if hwnd.is_null() {
        return None;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            return None;
        }
        let mut rect = RECT::default();
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut rect as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr != 0 && GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        PxRect {
            x: rect.left - shot.origin_x,
            y: rect.top - shot.origin_y,
            w: rect.right - rect.left,
            h: rect.bottom - rect.top,
        }
        .clamp_bounds(shot.width, shot.height)
    }
}

pub fn monitor_rects(origin_x: i32, origin_y: i32) -> Vec<PxRect> {
    let mut raw: Vec<RECT> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_mon),
            &mut raw as *mut Vec<RECT> as isize,
        );
    }
    raw.into_iter()
        .filter_map(|rect| {
            PxRect {
                x: rect.left - origin_x,
                y: rect.top - origin_y,
                w: rect.right - rect.left,
                h: rect.bottom - rect.top,
            }
            .clamp_bounds(100_000, 100_000)
        })
        .collect()
}

unsafe extern "system" fn enum_mon(
    _monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
    _dc: HDC,
    rect: *mut RECT,
    data: isize,
) -> i32 {
    unsafe {
        if rect.is_null() || data == 0 {
            return 1;
        }
        let list = &mut *(data as *mut Vec<RECT>);
        list.push(*rect);
    }
    1
}

pub fn cursor() -> (i32, i32) {
    let mut pt = windows_sys::Win32::Foundation::POINT::default();
    unsafe {
        GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

pub fn work_on_monitor(screen_x: i32, screen_y: i32) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromPoint(
            windows_sys::Win32::Foundation::POINT {
                x: screen_x,
                y: screen_y,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        if monitor.is_null() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        Some(info.rcWork)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_covers_the_virtual_screen() {
        let shot = capture().expect("capture");
        assert!(shot.width >= 100 && shot.height >= 100);
        assert_eq!(shot.bgra.len(), (shot.width * shot.height * 4) as usize);
        assert_eq!(shot.bgra[3], 255);
        assert!(!monitor_rects(shot.origin_x, shot.origin_y).is_empty());
    }
}
