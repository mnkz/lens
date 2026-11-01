use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, Ordering};

use eframe::egui;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SNAPSHOT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_ALTDOWN, LLKHF_UP, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

use crate::util::log_line;

static HOOK: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static REQUEST: AtomicU8 = AtomicU8::new(0);
static TARGET: AtomicIsize = AtomicIsize::new(0);
static INSIDE: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<egui::Context> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Freeze,
    Window,
}

pub fn install(ctx: egui::Context) {
    let _ = WAKE.set(ctx);
    unsafe {
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(proc),
            GetModuleHandleW(std::ptr::null()),
            0,
        );
        if hook.is_null() {
            log_line("hook Impr écran indisponible");
            return;
        }
        HOOK.store(hook, Ordering::Relaxed);
        log_line("hook Impr écran installé");
    }
}

pub fn remove() {
    let hook = HOOK.swap(std::ptr::null_mut(), Ordering::Relaxed);
    if !hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(hook as HHOOK);
        }
    }
}

pub fn peek() -> bool {
    REQUEST.load(Ordering::SeqCst) != 0
}

pub fn take() -> Option<Kind> {
    match REQUEST.swap(0, Ordering::SeqCst) {
        1 => Some(Kind::Freeze),
        2 => Some(Kind::Window),
        _ => None,
    }
}

pub fn take_target() -> HWND {
    TARGET.swap(0, Ordering::SeqCst) as usize as HWND
}

pub fn set_inside(inside: bool) {
    INSIDE.store(inside, Ordering::SeqCst);
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        }
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        if kb.vkCode != VK_SNAPSHOT as u32 {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        }
        let down = wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize;
        let up = kb.flags & LLKHF_UP != 0;
        if key_down(VK_LWIN) || key_down(VK_RWIN) || key_down(VK_CONTROL) {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        }
        if !down || up {
            return 1;
        }
        let alt = kb.flags & LLKHF_ALTDOWN != 0 || key_down(VK_MENU);
        if alt && !INSIDE.load(Ordering::SeqCst) {
            let hwnd = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid != 0 && pid != GetCurrentProcessId() {
                TARGET.store(hwnd as isize, Ordering::SeqCst);
                REQUEST.store(2, Ordering::SeqCst);
            } else {
                REQUEST.store(1, Ordering::SeqCst);
            }
        } else {
            REQUEST.store(1, Ordering::SeqCst);
        }
        if let Some(ctx) = WAKE.get() {
            ctx.request_repaint();
        }
        1
    }
}

fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}
