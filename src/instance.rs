//! Один экземпляр на сеанс пользователя.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

use crate::wide::wide;

pub struct Instance(#[allow(dead_code)] HANDLE);

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

/// Запускает новый экземпляр, который дождётся выхода текущего.
pub fn spawn_restart() {
    if let Ok(exe) = std::env::current_exe() {
        if let Err(e) = std::process::Command::new(exe).arg("--restart").spawn() {
            log!("не удалось перезапуститься: {e}");
        }
    }
}
