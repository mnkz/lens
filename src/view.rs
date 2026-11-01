use std::mem::ManuallyDrop;
use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_STYLE,
    GetWindowLongPtrW, MoveWindow, RegisterClassExW, SW_HIDE, SW_SHOW, SetWindowLongPtrW,
    ShowWindow, WM_ERASEBKGND, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebView, WebViewBuilder};

use crate::util::wide;

/// Narrow layout so Google Lens follows the panel instead of a desktop browser.
const AGENT: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";

const FIT: &str = r#"
(() => {
  const fit = () => {
    const head = document.head || document.documentElement;
    let meta = document.querySelector('meta[name="viewport"]');
    if (!meta) {
      meta = document.createElement('meta');
      meta.setAttribute('name', 'viewport');
      head.appendChild(meta);
    }
    meta.setAttribute('content', 'width=device-width, initial-scale=1, viewport-fit=cover');
  };
  fit();
  addEventListener('DOMContentLoaded', fit);
})();
"#;

pub struct Pane {
    view: ManuallyDrop<WebView>,
    host: HWND,
    shown: PathBuf,
    visible: bool,
}

impl Pane {
    pub fn open(
        frame: &eframe::Frame,
        page: &Path,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> Result<Self, String> {
        let url = file_url(page)?;
        let parent = frame_hwnd(frame)?;
        clip_children(parent);
        let host = create_host(parent, x, y, w, h)?;
        let built = WebViewBuilder::new()
            .with_url(&url)
            .with_bounds(fill(w, h))
            .with_focused(false)
            .with_background_color((0, 0, 0, 255))
            .with_visible(true)
            .with_user_agent(AGENT)
            .with_initialization_script(FIT)
            .build_as_child(&HostHwnd(host));
        let view = match built {
            Ok(view) => view,
            Err(err) => {
                unsafe { DestroyWindow(host) };
                return Err(err.to_string());
            }
        };
        Ok(Self {
            view: ManuallyDrop::new(view),
            host,
            shown: page.to_path_buf(),
            visible: true,
        })
    }

    /// Move the search to the current slot. The page fills that slot, so a
    /// resize of Lens resizes the results.
    pub fn sync(&mut self, page: &Path, x: i32, y: i32, w: u32, h: u32) -> Result<(), String> {
        if self.shown != page {
            let url = file_url(page)?;
            self.view.load_url(&url).map_err(|err| err.to_string())?;
            self.shown = page.to_path_buf();
        }
        unsafe {
            MoveWindow(self.host, x, y, w.max(1) as i32, h.max(1) as i32, 1);
            if !self.visible {
                ShowWindow(self.host, SW_SHOW);
            }
        }
        self.view
            .set_bounds(fill(w, h))
            .map_err(|err| err.to_string())?;
        if !self.visible {
            self.view.set_visible(true).map_err(|err| err.to_string())?;
            self.visible = true;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe { ShowWindow(self.host, SW_HIDE) };
            let _ = self.view.set_visible(false);
            self.visible = false;
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.view);
            if !self.host.is_null() {
                DestroyWindow(self.host);
                self.host = std::ptr::null_mut();
            }
        }
    }
}

struct HostHwnd(HWND);

impl HasWindowHandle for HostHwnd {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        let Some(hwnd) = NonZeroIsize::new(self.0 as isize) else {
            return Err(raw_window_handle::HandleError::Unavailable);
        };
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

fn fill(w: u32, h: u32) -> Rect {
    Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(w.max(1), h.max(1)).into(),
    }
}

pub(crate) fn frame_hwnd(frame: &eframe::Frame) -> Result<HWND, String> {
    let handle = frame.window_handle().map_err(|err| err.to_string())?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win) => Ok(win.hwnd.get() as HWND),
        _ => Err("fenêtre Lens introuvable".into()),
    }
}

fn clip_children(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
    }
}

fn create_host(parent: HWND, x: i32, y: i32, w: u32, h: u32) -> Result<HWND, String> {
    register_host();
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            host_class(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            x,
            y,
            w.max(1) as i32,
            h.max(1) as i32,
            parent,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        Err("cadre Lens impossible".into())
    } else {
        Ok(hwnd)
    }
}

fn host_class() -> *const u16 {
    static NAME: OnceLock<Vec<u16>> = OnceLock::new();
    NAME.get_or_init(|| wide("LensSlot")).as_ptr()
}

fn register_host() {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| {
        let klass = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(host_proc),
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            hbrBackground: unsafe { GetStockObject(BLACK_BRUSH) },
            lpszClassName: host_class(),
            ..WNDCLASSEXW::default()
        };
        let _ = unsafe { RegisterClassExW(&klass) };
    });
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if msg == WM_ERASEBKGND {
            return 1;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

fn file_url(path: &Path) -> Result<String, String> {
    let canon = path.canonicalize().map_err(|err| err.to_string())?;
    let text = canon.display().to_string();
    let trimmed = text.trim_start_matches(r"\\?\");
    let mut url = String::from("file:///");
    for byte in trimmed.bytes() {
        match byte {
            b'\\' => url.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                url.push(byte as char);
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    Ok(url)
}
