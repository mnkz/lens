use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, EndPaint, FillRect, FrameRect, GetDC, HALFTONE, ReleaseDC,
    SRCCOPY, SetStretchBltMode, StretchBlt,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, HTCAPTION, HTCLIENT, LoadCursorW,
    RegisterClassExW, SW_SHOW, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WM_DESTROY, WM_KEYDOWN, WM_LBUTTONUP, WM_NCCREATE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_PAINT,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::capture::work_on_monitor;
use crate::clipboard;
use crate::gdi::{self, Dib};
use crate::pixels;
use crate::util::{log_line, wide};

struct Pin {
    hwnd: HWND,
    bgra: Vec<u8>,
    src_w: i32,
    src_h: i32,
    view_w: i32,
    view_h: i32,
    dib: Dib,
}

static PINS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

pub fn open(bgra: &[u8], w: i32, h: i32, screen_x: i32, screen_y: i32) {
    if w < 2 || h < 2 {
        return;
    }
    let max_w = 520;
    let (view_w, view_h) = if w > max_w {
        let view_h = ((h as i64) * (max_w as i64) / (w as i64)).clamp(1, 4000) as i32;
        (max_w, view_h)
    } else {
        (w, h)
    };
    let screen = unsafe { GetDC(std::ptr::null_mut()) };
    if screen.is_null() {
        return;
    }
    let dib = match Dib::new(screen, bgra, w, h) {
        Ok(dib) => dib,
        Err(err) => {
            unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
            log_line(&err);
            return;
        }
    };
    unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
    register_class();
    let pin = Box::new(Pin {
        hwnd: std::ptr::null_mut(),
        bgra: bgra.to_vec(),
        src_w: w,
        src_h: h,
        view_w,
        view_h,
        dib,
    });
    let state = Box::into_raw(pin);
    let (x, y) = place(screen_x, screen_y, view_w + 2, view_h + 2);
    let title = wide("Lens");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            view_w + 2,
            view_h + 2,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            state as *mut _,
        )
    };
    if hwnd.is_null() {
        drop(unsafe { Box::from_raw(state) });
        log_line("épingle impossible");
        return;
    }
    if let Ok(mut pins) = PINS.lock() {
        pins.push(state as usize);
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetWindowPos(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_SHOWWINDOW
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
        );
    }
}

pub fn close_all() {
    let hwnds: Vec<HWND> = PINS
        .lock()
        .map(|pins| {
            pins.iter()
                .map(|raw| unsafe { &*(*raw as *const Pin) }.hwnd)
                .collect()
        })
        .unwrap_or_default();
    for hwnd in hwnds {
        if !hwnd.is_null() {
            unsafe { DestroyWindow(hwnd) };
        }
    }
}

fn place(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    let Some(work) = work_on_monitor(x, y) else {
        return (x, y);
    };
    let max_x = (work.right - w).max(work.left);
    let max_y = (work.bottom - h).max(work.top);
    (x.clamp(work.left, max_x), y.clamp(work.top, max_y))
}

fn class_name() -> *const u16 {
    static NAME: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| wide("LensPin")).as_ptr()
}

fn register_class() {
    static READY: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    READY.get_or_init(|| {
        let klass = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            hCursor: unsafe {
                LoadCursorW(
                    std::ptr::null_mut(),
                    windows_sys::Win32::UI::WindowsAndMessaging::IDC_ARROW,
                )
            },
            lpszClassName: class_name(),
            ..WNDCLASSEXW::default()
        };
        unsafe { RegisterClassExW(&klass) };
    });
}

fn forget(pin: *mut Pin) {
    if let Ok(mut pins) = PINS.lock() {
        pins.retain(|raw| *raw != pin as usize);
    }
    drop(unsafe { Box::from_raw(pin) });
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let created = &*(lparam as *const CREATESTRUCTW);
            let state = created.lpCreateParams as *mut Pin;
            if !state.is_null() {
                (*state).hwnd = hwnd;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Pin;
        if raw.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_DESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            forget(raw);
            return 0;
        }
        let pin = &mut *raw;
        match msg {
            WM_NCHITTEST => {
                let x = (lparam as i16) as i32;
                let y = ((lparam >> 16) as i16) as i32;
                let mut pt = POINT { x, y };
                windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
                if pt.x >= pin.view_w - 18 && pt.y <= 20 {
                    HTCLIENT as LRESULT
                } else {
                    HTCAPTION as LRESULT
                }
            }
            WM_LBUTTONUP => {
                DestroyWindow(hwnd);
                0
            }
            WM_NCLBUTTONDBLCLK => {
                if let Ok(png) = pixels::encode_png(&pin.bgra, pin.src_w, pin.src_h) {
                    let _ = clipboard::copy_image(&pin.bgra, pin.src_w, pin.src_h, &png);
                }
                0
            }
            WM_KEYDOWN => {
                if wparam as u16 == VK_ESCAPE {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_PAINT => {
                let mut paint = windows_sys::Win32::Graphics::Gdi::PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut paint);
                if !hdc.is_null() {
                    let mut client = RECT::default();
                    GetClientRect(hwnd, &mut client);
                    let black = CreateSolidBrush(gdi::bgr(0, 0, 0));
                    let line = CreateSolidBrush(gdi::bgr(244, 244, 242));
                    FillRect(hdc, &client, black);
                    SetStretchBltMode(hdc, HALFTONE);
                    StretchBlt(
                        hdc, 1, 1, pin.view_w, pin.view_h, pin.dib.dc, 0, 0, pin.src_w, pin.src_h,
                        SRCCOPY,
                    );
                    FrameRect(hdc, &client, line);
                    let close = RECT {
                        left: client.right - 22,
                        top: 1,
                        right: client.right - 1,
                        bottom: 20,
                    };
                    FillRect(hdc, &close, black);
                    FrameRect(hdc, &close, line);
                    windows_sys::Win32::Graphics::Gdi::DeleteObject(black);
                    windows_sys::Win32::Graphics::Gdi::DeleteObject(line);
                    EndPaint(hwnd, &paint);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
