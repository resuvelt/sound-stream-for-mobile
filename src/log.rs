//! Журнал в %LOCALAPPDATA%\sound-stream\server.log — консоли у приложения нет,
//! так что это единственный способ понять, что пошло не так.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

pub fn dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("sound-stream")
}

pub fn path() -> PathBuf {
    dir().join("server.log")
}

/// Каждый запуск начинает журнал с чистого листа.
pub fn init() {
    let _ = std::fs::create_dir_all(dir());
    let file = File::create(path()).ok().map(|mut f| {
        let _ = f.write_all("\u{feff}".as_bytes()); // BOM — Блокнот не ошибётся с кодировкой
        f
    });
    let _ = FILE.set(Mutex::new(file));
}

pub fn write(args: std::fmt::Arguments) {
    let line = format!("{} {}\r\n", timestamp(), args);
    #[cfg(debug_assertions)]
    eprint!("{line}");
    if let Some(m) = FILE.get() {
        if let Ok(mut f) = m.lock() {
            if let Some(f) = f.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
}

fn timestamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime только заполняет переданную структуру.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(format_args!($($t)*)) };
}
