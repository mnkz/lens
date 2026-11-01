use std::io::Write;
use std::path::PathBuf;

use chrono::Local;

pub fn data_dir() -> PathBuf {
    let dir = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Lens");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn log_line(text: &str) {
    let path = data_dir().join("lens.log");
    let line = format!("{} {text}\n", Local::now().format("%Y-%m-%d %H:%M:%S"));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown".to_string()
    }
}
