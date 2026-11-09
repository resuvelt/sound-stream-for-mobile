//! Автозапуск при входе в Windows: значение в
//! HKCU\Software\Microsoft\Windows\CurrentVersion\Run (права администратора не нужны).

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_Startup, SHGetKnownFolderPath};

use crate::wide::{from_ptr, wide};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "sound-stream";
/// С этим аргументом exe запускается при входе: только значок, без окна.
pub const FLAG: &str = "--autostart";

/// Команда, записанная в автозапуск (если есть).
fn current() -> Option<String> {
    let key = wide(RUN);
    let name = wide(NAME);
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: буфер и его размер в байтах переданы согласованно.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1); // без завершающего нуля
    Some(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}

pub fn is_enabled() -> bool {
    current().is_some()
}

pub fn set(enabled: bool) -> bool {
    if enabled {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        write(Some(&exe))
    } else {
        write(None)
    }
}

/// Записывает автозапуск `exe` (или удаляет его, если `None`).
fn write(exe: Option<&Path>) -> bool {
    let key = wide(RUN);
    let name = wide(NAME);
    let enabled = exe.is_some();
    // SAFETY: строки нуль-терминированы, размер данных — в байтах.
    let r = unsafe {
        if let Some(exe) = exe {
            let cmd = wide(&format!("\"{}\" {FLAG}", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                cmd.as_ptr().cast(),
                (cmd.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                r => r,
            }
        }
    };
    if r == ERROR_SUCCESS {
        log!(
            "автозапуск {}",
            if enabled {
                "включён"
            } else {
                "выключен"
            }
        );
    } else {
        log!("не удалось изменить автозапуск (код {r})");
    }
    r == ERROR_SUCCESS
}

/// exe перенесли в другую папку — автозапуск указывал бы в пустоту. Чиним.
/// Заодно дописываем `--autostart` в запись прежней версии, иначе при каждом
/// входе в Windows открывалось бы окно.
pub fn refresh_path() {
    let Some(cmd) = current() else {
        return;
    };
    let path = PathBuf::from(exe_of(&cmd));
    if !path.exists() {
        log!(
            "автозапуск указывал на несуществующий {} — обновляю путь",
            path.display()
        );
        set(true);
    } else if !cmd.split_whitespace().any(|a| a == FLAG) {
        write(Some(&path));
    }
}

/// Путь к exe из команды автозапуска (в кавычках или без).
fn exe_of(cmd: &str) -> &str {
    let c = cmd.trim();
    match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => c.split_whitespace().next().unwrap_or(c),
    }
}

/// Старая версия (Node + PowerShell) ставила ярлык в «Автозагрузку», который
/// запускал tray\launch-hidden.vbs. Меняем его на автозапуск exe.
pub fn migrate_legacy() -> bool {
    let Some(lnk) = startup_dir().map(|d| d.join("sound-stream.lnk")) else {
        return false;
    };
    if !lnk.exists() {
        return false;
    }
    match std::fs::remove_file(&lnk) {
        Ok(()) => {
            log!("старый ярлык автозапуска удалён: {}", lnk.display());
            set(true)
        }
        Err(e) => {
            log!("не удалось удалить старый ярлык {}: {e}", lnk.display());
            false
        }
    }
}

fn startup_dir() -> Option<PathBuf> {
    // SAFETY: строку от SHGetKnownFolderPath освобождаем CoTaskMemFree.
    unsafe {
        let mut p = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_Startup, 0, std::ptr::null_mut(), &mut p);
        let dir = (hr >= 0).then(|| PathBuf::from(from_ptr(p)));
        CoTaskMemFree(p as *const _);
        dir
    }
}
