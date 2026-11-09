//! Значок в трее: клик открывает панель с адресами и настройками (ui::flyout),
//! двойной клик — главное окно (ui::window). Здесь же — всплывающие уведомления.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP,
    NIIF_ERROR, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW,
    GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, LoadIconW, LoadImageW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetTimer, TranslateMessage, HICON,
    IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, MSG, SM_CXSMICON, SM_CYSMICON, WM_APP,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

use crate::state::App;
use crate::ui;
use crate::wide::{copy_into, wide};

const CLASS: &str = "sound-stream.tray";
const WM_TRAY: u32 = WM_APP + 1; // события значка
const WM_NOTE: u32 = WM_APP + 2; // в очереди есть уведомление
const WM_POKE: u32 = WM_APP + 3; // запустили второй экземпляр
const ICON_UID: u32 = 1;
const TIMER_TIP: usize = 1;

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
static QUITTING: AtomicBool = AtomicBool::new(false);
static NOTES: Mutex<VecDeque<Note>> = Mutex::new(VecDeque::new());

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

/// Второй запуск exe: просим уже работающий экземпляр показать своё окно.
pub fn poke_running_instance() {
    let cls = wide(CLASS);
    // SAFETY: обычные вызовы WinAPI.
    unsafe {
        let h = FindWindowW(cls.as_ptr(), std::ptr::null());
        if !h.is_null() {
            // Передний план сейчас у нас (exe только что запустили) — отдаём
            // его работающему экземпляру, иначе его окно откроется позади.
            let mut pid = 0;
            GetWindowThreadProcessId(h, &mut pid);
            AllowSetForegroundWindow(pid);
            PostMessageW(h, WM_POKE, 0, 0);
        }
    }
}

/// Цикл сообщений трея (и всех окон интерфейса); возвращается по «Выход»
/// или «Перезапустить».
pub fn run(app: Arc<App>) -> Exit {
    let show_window = app.show_window;
    let _ = APP.set(app);
    // SAFETY: всё окно и значок живут в этом (главном) потоке.
    unsafe {
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
        if show_window {
            ui::window::show();
        }

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

pub fn app() -> &'static App {
    APP.get().expect("tray::run sets APP")
}

/// Скрытое окно значка — владелец буфера обмена и адресат уведомлений.
pub fn hwnd() -> HWND {
    HWND_.load(Ordering::SeqCst) as HWND
}

/// Где на экране значок (для панели, которая открывается рядом с ним).
pub fn icon_rect() -> Option<RECT> {
    let h = hwnd();
    if h.is_null() {
        return None;
    }
    // SAFETY: структуры заполнены, размер проставлен.
    unsafe {
        let mut id: NOTIFYICONIDENTIFIER = std::mem::zeroed();
        id.cbSize = std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32;
        id.hWnd = h;
        id.uID = ICON_UID;
        let mut r: RECT = std::mem::zeroed();
        (Shell_NotifyIconGetRect(&id, &mut r) >= 0).then_some(r)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match lp as u32 {
                WM_LBUTTONUP | WM_RBUTTONUP => ui::flyout::toggle(),
                WM_LBUTTONDBLCLK => {
                    ui::flyout::close();
                    ui::window::show();
                }
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
            ui::window::show();
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
        return format!("Sound Stream — порт {} занят", a.cfg.port);
    }
    if !a.server_up.load(Ordering::SeqCst) {
        return "Sound Stream — запуск…".into();
    }
    match a.listeners() {
        0 => "Sound Stream — работает".into(),
        n => format!("Sound Stream — в эфире, слушателей: {n}"),
    }
}

/// Завершает цикл сообщений: выход или перезапуск.
pub fn quit(exit: Exit) {
    if let Ok(mut e) = EXIT.lock() {
        *e = exit;
    }
    QUITTING.store(true, Ordering::SeqCst);
    // SAFETY: зовётся из потока интерфейса — того, где крутится цикл.
    unsafe { PostQuitMessage(0) };
}

/// Программа уже закрывается (окна гаснут не по воле пользователя).
pub fn quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}
