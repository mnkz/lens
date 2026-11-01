use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Local;
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_Screenshots, SHGetKnownFolderPath};

use crate::pixels;

pub struct Thumb {
    pub path: PathBuf,
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub fn shots_dir() -> PathBuf {
    let dir = crate::util::data_dir().join("shots");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn keep(png: &[u8]) -> PathBuf {
    let dir = shots_dir();
    let stamp = Local::now();
    let name = format!(
        "{}-{:03}.png",
        stamp.format("%Y-%m-%d-%H%M%S"),
        stamp.timestamp_subsec_millis()
    );
    let path = dir.join(name);
    let _ = std::fs::write(&path, png);
    prune(&dir, 30);
    path
}

pub fn save_screenshots(png: &[u8]) -> Result<PathBuf, String> {
    let dir = screenshots_dir();
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let name = format!("Lens {}.png", Local::now().format("%Y-%m-%d %H%M%S"));
    let path = dir.join(name);
    std::fs::write(&path, png).map_err(|err| err.to_string())?;
    Ok(path)
}

pub fn screenshots_dir() -> PathBuf {
    unsafe {
        let mut ptr = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_Screenshots, 0, std::ptr::null_mut(), &mut ptr);
        if hr == 0 && !ptr.is_null() {
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            CoTaskMemFree(ptr as *const _);
            if !text.is_empty() {
                return PathBuf::from(text);
            }
        }
    }
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Pictures").join("Screenshots")
}

pub fn load_thumbs(limit: usize) -> Vec<Thumb> {
    let mut files = list_shots();
    if files.len() > limit {
        files = files.split_off(files.len() - limit);
    }
    files.reverse();
    files
        .into_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let (bgra, w, h) = pixels::decode_png(&bytes).ok()?;
            let (rgba, tw, th) = pixels::thumb_rgba(&bgra, w, h, 160);
            Some(Thumb {
                path,
                rgba,
                w: tw,
                h: th,
            })
        })
        .collect()
}

pub fn list_shots() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(shots_dir())
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        })
        .collect();
    files.sort();
    files
}

fn prune(dir: &Path, keep_n: usize) {
    let mut files = list_shots();
    if files.len() <= keep_n {
        return;
    }
    let extra = files.len() - keep_n;
    files.truncate(extra);
    for path in files {
        if path.starts_with(dir) {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn startup_lnk() -> PathBuf {
    let root = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    root.join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("Lens.lnk")
}

pub fn startup_enabled() -> bool {
    startup_lnk().is_file()
}

pub fn set_startup(on: bool) -> Result<(), String> {
    let path = startup_lnk();
    if !on {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|err| err.to_string())?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath = '{}'; $s.Arguments = '--tray'; $s.WorkingDirectory = '{}'; $s.WindowStyle = 7; $s.Description = 'Lens'; $s.Save()",
        escape(&path),
        escape(&exe),
        escape(dir),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|err| err.to_string())?;
    if status.success() && path.is_file() {
        Ok(())
    } else {
        Err("raccourci de démarrage impossible".into())
    }
}

fn escape(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}
