//! Значок в трее: адреса для телефона, «открыть на этом ПК», автозапуск,
//! журнал, перезапуск, выход. Окна у приложения нет — только этот значок.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{GlobalFree, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR,
    NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics, LoadIconW, LoadImageW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetMenuDefaultItem, SetTimer, TrackPopupMenu, TranslateMessage, HICON, HMENU, IDI_APPLICATION,
    IMAGE_ICON, LR_DEFAULTCOLOR, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON,
    SM_CYSMICON, SW_SHOWNORMAL, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
    WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

use crate::state::App;
use crate::wide::{copy_into, wide};
use crate::{autostart, net};

const CLASS: &str = "sound-stream.tray";
const WM_TRAY: u32 = WM_APP + 1; // события значка
const WM_NOTE: u32 = WM_APP + 2; // в очереди есть уведомление
const WM_POKE: u32 = WM_APP + 3; // запустили второй экземпляр
const ICON_UID: u32 = 1;
const TIMER_TIP: usize = 1;

const ID_COPY_MDNS: u32 = 10;
const ID_OPEN: u32 = 20;
const ID_AUTOSTART: u32 = 21;
const ID_LOG: u32 = 22;
const ID_RESTART: u32 = 23;
const ID_EXIT: u32 = 24;
const ID_COPY_IP: u32 = 100; // 100.. — адреса по IP

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Quit,
    Restart,
}

#[derive(Clone, Copy)]
pub enum Kind {
    Info,
    Warning,
    #[allow(dead_code)]
    Error,
}

struct Note {
    title: String,
    text: String,
    kind: Kind,
}

static APP: OnceLock<Arc<App>> = OnceLock::new();
static HWND_: AtomicUsize = AtomicUsize::new(0);
static ICON: AtomicUsize = AtomicUsize::new(0);
static ICON_ADDED: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static EXIT: Mutex<Exit> = Mutex::new(Exit::Quit);
static NOTES: Mutex<VecDeque<Note>> = Mutex::new(VecDeque::new());
/// Адреса по IP из последнего открытого меню (по индексу пункта).
static MENU_URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Всплывающее уведомление у значка. Можно звать из любого потока, в том
/// числе до появления значка — покажется, как только он будет.
pub fn notify(title: impl Into<String>, text: impl Into<String>, kind: Kind) {
    if let Ok(mut q) = NOTES.lock() {
        q.push_back(Note {
            title: title.into(),
            text: text.into(),
            kind,
        });
    }
    let h = HWND_.load(Ordering::SeqCst);
    if h != 0 {
        // SAFETY: PostMessage потокобезопасен; окно живёт до выхода из run().
        unsafe { PostMessageW(h as HWND, WM_NOTE, 0, 0) };
    }
}

/// Второй запуск exe: просим уже работающий экземпляр напомнить о себе.
pub fn poke_running_instance() {
    let cls = wide(CLASS);
    // SAFETY: обычные вызовы WinAPI.
    unsafe {
        let h = FindWindowW(cls.as_ptr(), std::ptr::null());
        if !h.is_null() {
            PostMessageW(h, WM_POKE, 0, 0);
        }
    }
}

/// Цикл сообщений трея; возвращается по «Выход» или «Перезапустить».
pub fn run(app: Arc<App>) -> Exit {
    let _ = APP.set(app);
    // SAFETY: всё окно и значок живут в этом (главном) потоке.
    unsafe {
        allow_dark_menus();
        let hinst = GetModuleHandleW(std::ptr::null());
        let cls = wide(CLASS);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: cls.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("sound-stream");
        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log!("[tray] не удалось создать окно — работаю без значка");
            loop {
                std::thread::park();
            }
        }
        // Explorer перезапустился (или стартовал позже нас при входе) — значок
        // надо добавить заново.
        let tb = wide("TaskbarCreated");
        TASKBAR_CREATED.store(RegisterWindowMessageW(tb.as_ptr()), Ordering::SeqCst);

        let mut icon = LoadImageW(
            hinst,
            1 as _, // ресурс иконки №1 (build.rs)
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        ) as HICON;
        if icon.is_null() {
            icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        ICON.store(icon as usize, Ordering::SeqCst);

        HWND_.store(hwnd as usize, Ordering::SeqCst);
        add_icon(hwnd);
        SetTimer(hwnd, TIMER_TIP, 2000, None);
        PostMessageW(hwnd, WM_NOTE, 0, 0); // показать то, что накопилось до окна

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        HWND_.store(0, Ordering::SeqCst);
        let nid = base_nid(hwnd);
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
    EXIT.lock().map(|e| *e).unwrap_or(Exit::Quit)
}

fn app() -> &'static App {
    APP.get().expect("tray::run sets APP")
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match lp as u32 {
                WM_RBUTTONUP => show_menu(hwnd),
                WM_LBUTTONDBLCLK => open(&app().cfg.local_url()),
                _ => {}
            }
            0
        }
        WM_NOTE => {
            // Windows показывает одно уведомление за раз — берём самое свежее.
            let last = NOTES.lock().ok().and_then(|mut q| q.drain(..).next_back());
            if let Some(n) = last {
                balloon(hwnd, &n);
            }
            0
        }
        WM_POKE => {
            let a = app();
            balloon(
                hwnd,
                &Note {
                    title: "sound-stream уже запущен".into(),
                    text: format!(
                        "Значок — у часов в трее. Адрес для телефона: {}",
                        a.cfg.mdns_url()
                    ),
                    kind: Kind::Info,
                },
            );
            0
        }
        WM_TIMER => {
            if ICON_ADDED.load(Ordering::SeqCst) {
                let mut nid = base_nid(hwnd);
                nid.uFlags = NIF_TIP;
                copy_into(&mut nid.szTip, &tooltip(app()));
                Shell_NotifyIconW(NIM_MODIFY, &nid);
            } else {
                add_icon(hwnd); // панель задач ещё не готова (ранний автозапуск)
            }
            0
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            add_icon(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn base_nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = ICON_UID;
    nid
}

unsafe fn add_icon(hwnd: HWND) {
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = ICON.load(Ordering::SeqCst) as HICON;
    copy_into(&mut nid.szTip, &tooltip(app()));
    let ok = Shell_NotifyIconW(NIM_ADD, &nid) != 0 || Shell_NotifyIconW(NIM_MODIFY, &nid) != 0;
    ICON_ADDED.store(ok, Ordering::SeqCst);
}

unsafe fn balloon(hwnd: HWND, n: &Note) {
    let mut nid = base_nid(hwnd);
    nid.uFlags = NIF_INFO;
    copy_into(&mut nid.szInfoTitle, &n.title);
    copy_into(&mut nid.szInfo, &n.text);
    nid.dwInfoFlags = match n.kind {
        Kind::Info => NIIF_INFO,
        Kind::Warning => NIIF_WARNING,
        Kind::Error => NIIF_ERROR,
    };
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

fn tooltip(a: &App) -> String {
    if a.port_busy.load(Ordering::SeqCst) {
        return format!("sound-stream — порт {} занят", a.cfg.port);
    }
    if !a.server_up.load(Ordering::SeqCst) {
        return "sound-stream — запуск…".into();
    }
    match a.listeners() {
        0 => "sound-stream — работает".into(),
        n => format!("sound-stream — в эфире, слушателей: {n}"),
    }
}

fn status_line(a: &App) -> String {
    if a.port_busy.load(Ordering::SeqCst) {
        format!("○  порт {} занят — жду освобождения", a.cfg.port)
    } else if !a.server_up.load(Ordering::SeqCst) {
        "○  сервер запускается…".into()
    } else {
        match a.listeners() {
            0 => "●  сервер работает".into(),
            n => format!("●  сервер работает · слушателей: {n}"),
        }
    }
}

fn source_line(a: &App) -> Option<String> {
    let s = a.source()?;
    let mut device: String = s.device.chars().take(40).collect();
    if s.device.chars().count() > 40 {
        device.push('…');
    }
    let khz = if s.sample_rate % 1000 == 0 {
        (s.sample_rate / 1000).to_string()
    } else {
        format!("{:.1}", s.sample_rate as f64 / 1000.0).replace('.', ",")
    };
    Some(format!("     звук: {device} · {khz} кГц"))
}

unsafe fn item(m: HMENU, id: u32, text: &str, flags: u32) {
    let w = wide(text);
    AppendMenuW(m, MF_STRING | flags, id as usize, w.as_ptr());
}

unsafe fn separator(m: HMENU) {
    AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
}

/// Меню собирается заново при каждом открытии — адреса и статус всегда свежие.
unsafe fn show_menu(hwnd: HWND) {
    let a = app();
    let cfg = &a.cfg;
    let m = CreatePopupMenu();

    item(m, 0, "sound-stream", MF_GRAYED);
    item(m, 0, &status_line(a), MF_GRAYED);
    if let Some(line) = source_line(a) {
        item(m, 0, &line, MF_GRAYED);
    }
    separator(m);

    // Постоянный адрес (mDNS) — добавить на телефоне в закладку один раз,
    // он не зависит от смены IP по DHCP. Клик копирует.
    item(
        m,
        0,
        "Постоянный адрес (добавь в закладку на телефоне):",
        MF_GRAYED,
    );
    item(m, ID_COPY_MDNS, &format!("     ★  {}", cfg.mdns_url()), 0);
    item(m, 0, "Или по IP (запасной вариант):", MF_GRAYED);
    let urls: Vec<String> = net::lan_ipv4()
        .into_iter()
        .map(|ip| cfg.ip_url(ip))
        .collect();
    if urls.is_empty() {
        item(m, 0, "     LAN-адрес не найден", MF_GRAYED);
    }
    for (i, url) in urls.iter().enumerate().take(50) {
        item(m, ID_COPY_IP + i as u32, &format!("     {url}"), 0);
    }
    if let Ok(mut u) = MENU_URLS.lock() {
        *u = urls;
    }
    separator(m);

    item(m, ID_OPEN, "Открыть на этом ПК", 0);
    SetMenuDefaultItem(m, ID_OPEN, 0); // жирным: то же, что двойной клик
    let auto = if autostart::is_enabled() {
        MF_CHECKED
    } else {
        0
    };
    item(m, ID_AUTOSTART, "Запускать при входе в Windows", auto);
    item(m, ID_LOG, "Открыть журнал", 0);
    item(m, ID_RESTART, "Перезапустить", 0);
    separator(m);
    item(m, ID_EXIT, "Выход", 0);

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // Без этого меню не закрывается по клику мимо него (известная особенность трея).
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        m,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    ) as u32;
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(m);

    match cmd {
        0 => {}
        ID_COPY_MDNS => copy_url(hwnd, &cfg.mdns_url()),
        ID_OPEN => open(&cfg.local_url()),
        ID_AUTOSTART => {
            let on = !autostart::is_enabled();
            if autostart::set(on) {
                let text = if on {
                    "sound-stream будет запускаться сам при входе в Windows."
                } else {
                    "Автозапуск выключен."
                };
                balloon(hwnd, &note("Автозапуск", text, Kind::Info));
            } else {
                balloon(
                    hwnd,
                    &note(
                        "Автозапуск",
                        "Не удалось изменить настройку.",
                        Kind::Warning,
                    ),
                );
            }
        }
        ID_LOG => open(&crate::log::path().to_string_lossy()),
        ID_RESTART => quit(Exit::Restart),
        ID_EXIT => quit(Exit::Quit),
        id if id >= ID_COPY_IP => {
            let url = MENU_URLS
                .lock()
                .ok()
                .and_then(|u| u.get((id - ID_COPY_IP) as usize).cloned());
            if let Some(url) = url {
                copy_url(hwnd, &url);
            }
        }
        _ => {}
    }
}

fn note(title: &str, text: &str, kind: Kind) -> Note {
    Note {
        title: title.into(),
        text: text.into(),
        kind,
    }
}

unsafe fn quit(exit: Exit) {
    if let Ok(mut e) = EXIT.lock() {
        *e = exit;
    }
    PostQuitMessage(0);
}

unsafe fn copy_url(hwnd: HWND, url: &str) {
    if copy_text(hwnd, url) {
        balloon(hwnd, &note("Адрес скопирован", url, Kind::Info));
    }
}

unsafe fn copy_text(hwnd: HWND, s: &str) -> bool {
    let w = wide(s);
    if OpenClipboard(hwnd) == 0 {
        return false;
    }
    EmptyClipboard();
    let mem = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
    let mut ok = false;
    if !mem.is_null() {
        let p = GlobalLock(mem) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            GlobalUnlock(mem);
            ok = !SetClipboardData(CF_UNICODETEXT as u32, mem).is_null();
        }
        if !ok {
            GlobalFree(mem); // при успехе памятью владеет буфер обмена
        }
    }
    CloseClipboard();
    ok
}

fn open(target: &str) {
    let verb = wide("open");
    let t = wide(target);
    // SAFETY: строки нуль-терминированы.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            t.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Контекстное меню трея следует тёмной теме Windows. Официального API нет —
/// это ordinal 135/136 из uxtheme (Windows 10 1903+); на старых сборках не зовём.
unsafe fn allow_dark_menus() {
    let mut v: OSVERSIONINFOW = std::mem::zeroed();
    v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    if GetVersionExW(&mut v) == 0 || v.dwMajorVersion < 10 || v.dwBuildNumber < 18362 {
        return;
    }
    let name = wide("uxtheme.dll");
    let lib = LoadLibraryW(name.as_ptr());
    if lib.is_null() {
        return;
    }
    // SetPreferredAppMode(AllowDark = 1)
    if let Some(f) = GetProcAddress(lib, 135 as _) {
        let f: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
        f(1);
    }
    // FlushMenuThemes()
    if let Some(f) = GetProcAddress(lib, 136 as _) {
        let f: unsafe extern "system" fn() = std::mem::transmute(f);
        f();
    }
}
