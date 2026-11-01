use std::path::{Path, PathBuf};

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::util::{data_dir, wide};

pub fn open(target: &str) -> Result<(), String> {
    let file = wide(target);
    let op = wide("open");
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (rc as isize) <= 32 {
        Err(format!("ouverture impossible · {target}"))
    } else {
        Ok(())
    }
}

pub fn open_path(path: &Path) -> Result<(), String> {
    open(&path.display().to_string())
}

pub fn google_text(query: &str) -> Result<(), String> {
    let url = format!("https://www.google.com/search?q={}", urlencode(query));
    open(&url)
}

pub fn lens_page(png: &[u8]) -> Result<PathBuf, String> {
    let dir = data_dir();
    prune_pages(&dir);
    let name = format!(
        "lens-{}.html",
        chrono::Local::now().format("%Y%m%d%H%M%S%f")
    );
    let path = dir.join(name);
    let page = format!(
        r#"<!DOCTYPE html>
<meta charset="utf-8">
<title>Lens</title>
<body style="margin:0;background:#000;color:#8a8a88;font-family:Segoe UI,sans-serif">
<form id="f" action="https://lens.google.com/v3/upload" method="POST" enctype="multipart/form-data">
<input id="file" type="file" name="encoded_image" style="display:none">
</form>
<script>
const bin = atob("{b64}");
const bytes = new Uint8Array(bin.length);
for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
const file = new File([bytes], "lens.png", {{type:"image/png"}});
const data = new DataTransfer();
data.items.add(file);
document.getElementById("file").files = data.files;
document.getElementById("f").submit();
</script>
</body>
"#,
        b64 = base64(png)
    );
    std::fs::write(&path, page).map_err(|err| err.to_string())?;
    Ok(path)
}

fn prune_pages(dir: &Path) {
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lens-") && name.ends_with(".html"))
        })
        .collect();
    pages.sort();
    let keep = 4;
    if pages.len() > keep {
        for path in &pages[..pages.len() - keep] {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_carries_the_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0, 1, 2, 3];
        let path = lens_page(&png).expect("page");
        let html = std::fs::read_to_string(&path).expect("read");
        assert!(html.contains("name=\"encoded_image\""));
        assert!(html.contains(&base64(&png)));
        assert!(html.contains("https://lens.google.com/v3/upload"));
        let _ = std::fs::remove_file(path);
    }
}
