//! UTF-16 для Win32 API.

/// Строка → нуль-терминированный UTF-16.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Копирует строку в фиксированный буфер Win32-структуры (с обрезкой и нулём в конце).
pub fn copy_into(dst: &mut [u16], s: &str) {
    if dst.is_empty() {
        return;
    }
    let mut i = 0;
    for c in s.encode_utf16() {
        if i + 1 >= dst.len() {
            break;
        }
        dst[i] = c;
        i += 1;
    }
    dst[i] = 0;
}

/// Нуль-терминированный UTF-16 по указателю → String.
///
/// # Safety
/// `p` — null или указатель на нуль-терминированную UTF-16 строку.
pub unsafe fn from_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}
