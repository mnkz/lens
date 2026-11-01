use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};

use crate::pixels;
use crate::util::wide;

pub fn copy_image(bgra: &[u8], w: i32, h: i32, png: &[u8]) -> Result<(), String> {
    let dib = pixels::dib_bytes(bgra, w, h);
    let png_fmt = register("PNG")?;
    let mime = register("image/png")?;
    with_clipboard(|| {
        set_bytes(CF_DIB as u32, &dib)?;
        set_bytes(png_fmt, png)?;
        set_bytes(mime, png)?;
        Ok(())
    })
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let mut bytes: Vec<u8> = text
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    bytes.extend_from_slice(&[0, 0]);
    with_clipboard(|| set_bytes(CF_UNICODETEXT as u32, &bytes))
}

fn register(name: &str) -> Result<u32, String> {
    let wide = wide(name);
    let fmt = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
    if fmt == 0 {
        Err(format!("format {name}"))
    } else {
        Ok(fmt)
    }
}

fn with_clipboard(body: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let mut opened = false;
    for _ in 0..12 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    if !opened {
        return Err("presse-papiers occupé".into());
    }
    let cleared = unsafe { EmptyClipboard() } != 0;
    let result = if cleared {
        body()
    } else {
        Err("presse-papiers refusé".into())
    };
    unsafe {
        CloseClipboard();
    }
    result
}

fn set_bytes(format: u32, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if mem.is_null() {
            return Err("mémoire".into());
        }
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            GlobalFree(mem);
            return Err("mémoire".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        GlobalUnlock(mem);
        let placed = SetClipboardData(format, mem as HANDLE);
        if placed.is_null() {
            GlobalFree(mem);
            return Err("SetClipboardData".into());
        }
        let _ = placed;
        Ok(())
    }
}
