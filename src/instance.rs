//! Один экземпляр на сеанс пользователя.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

use crate::wide::wide;

pub struct Instance(#[allow(dead_code)] HANDLE);

/// Аргумент перезапуска: дождаться выхода прежнего процесса.
pub const RESTART: &str = "--restart";
/// Аргумент «сразу показать окно» (перезапуск из открытого окна).
pub const SHOW: &str = "--show";

/// Захватывает блокировку экземпляра. `wait` — подождать, пока предыдущий
/// процесс завершится (перезапуск); без него при занятой блокировке → None.
pub fn acquire(wait: bool) -> Option<Instance> {
    let name = wide("Local\\sound-stream");
    // SAFETY: обычные вызовы WinAPI; имя — нуль-терминированная строка.
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            return Some(Instance(h)); // не смогли проверить — не мешаем запуску
        }
        let r = WaitForSingleObject(h, if wait { 15_000 } else { 0 });
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
            Some(Instance(h))
        } else {
            CloseHandle(h);
            None
        }
    }
}

/// Запускает новый экземпляр, который дождётся выхода текущего; `show` —
/// он откроет окно (перезапускали из окна).
pub fn spawn_restart(show: bool) {
    if let Ok(exe) = std::env::current_exe() {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg(RESTART);
        if show {
            cmd.arg(SHOW);
        }
        if let Err(e) = cmd.spawn() {
            log!("не удалось перезапуститься: {e}");
        }
    }
}
