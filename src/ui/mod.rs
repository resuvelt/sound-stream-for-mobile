//! Окно приложения и панель у значка в трее — в стиле Windows 11: Mica и
//! Acrylic, скруглённые углы, тёмная и светлая тема. Рисуется через Direct2D,
//! без библиотек интерфейса и рантаймов: exe остаётся одним файлом.
//!
//! Всё, что здесь, живёт в главном потоке — том же, где цикл сообщений трея.

pub mod flyout;
mod gfx;
mod kit;
pub mod window;

use std::cell::{Cell, RefCell};
use std::net::Ipv4Addr;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GlobalFree, HWND};
use windows_sys::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::state::Source;
use crate::tray::{self, Exit, Kind};
use crate::wide::wide;
use crate::{autostart, net};

use gfx::Color;
use kit::Theme;

/// Что можно сделать кликом в окне или в панели.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Показать этот адрес в QR-коде (0 — постоянный, дальше — по IP).
    Select(u8),
    /// Скопировать адрес (нумерация как у `Select`).
    Copy(u8),
    /// Открыть плеер на этом ПК.
    Player,
    /// Открыть главное окно.
    Window,
    Autostart,
    Log,
    Restart,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Starting,
    PortBusy,
    Ready,
    Live(usize),
}

impl Status {
    fn color(self, t: Theme) -> Color {
        match self {
            Status::Starting => t.warn(),
            Status::PortBusy => t.bad(),
            Status::Ready | Status::Live(_) => t.accent(),
        }
    }

    fn label(self) -> String {
        match self {
            Status::Starting => "Запуск…".into(),
            Status::PortBusy => "Порт занят".into(),
            Status::Ready => "Готов · ждёт телефон".into(),
            Status::Live(n) => format!("В эфире · {}", listeners(n)),
        }
    }
}

/// Снимок состояния для одного кадра.
pub struct Info {
    pub status: Status,
    pub port: u16,
    /// [0] — постоянный адрес `.local`, дальше — по IP.
    pub urls: Vec<String>,
    pub source: Option<Source>,
    pub autostart: bool,
}

fn info() -> Info {
    let a = tray::app();
    let status = if a.port_busy.load(Ordering::SeqCst) {
        Status::PortBusy
    } else if !a.server_up.load(Ordering::SeqCst) {
        Status::Starting
    } else {
        match a.listeners() {
            0 => Status::Ready,
            n => Status::Live(n),
        }
    };
    let mut urls = vec![a.cfg.mdns_url()];
    urls.extend(lan_ips().into_iter().map(|ip| a.cfg.ip_url(ip)));
    Info {
        status,
        port: a.cfg.port,
        urls,
        source: a.source(),
        autostart: autostart::is_enabled(),
    }
}

thread_local! {
    static IPS: RefCell<Option<(Instant, Vec<Ipv4Addr>)>> = const { RefCell::new(None) };
    static COPIED: Cell<Option<(Act, Instant)>> = const { Cell::new(None) };
}

/// Адреса по IP: опрос адаптеров не на каждый кадр, а раз в несколько секунд.
fn lan_ips() -> Vec<Ipv4Addr> {
    IPS.with(|c| {
        let mut c = c.borrow_mut();
        match c.as_ref() {
            Some((at, ips)) if at.elapsed() < Duration::from_secs(5) => ips.clone(),
            _ => {
                let ips = net::lan_ipv4();
                *c = Some((Instant::now(), ips.clone()));
                ips
            }
        }
    })
}

/// Окно открылось — адреса должны быть свежими, а не пятисекундной давности.
fn refresh_ips() {
    IPS.with(|c| *c.borrow_mut() = None);
}

/// Кнопка «скопировать» ещё полторы секунды показывает галочку.
fn copied(act: Act) -> bool {
    COPIED.with(|c| {
        c.get()
            .is_some_and(|(a, at)| a == act && at.elapsed() < Duration::from_millis(1500))
    })
}

fn perform(act: Act) {
    let a = tray::app();
    match act {
        Act::Select(_) => {}
        Act::Copy(i) => {
            if let Some(url) = info().urls.get(i as usize) {
                if copy_text(tray::hwnd(), url) {
                    COPIED.with(|c| c.set(Some((act, Instant::now()))));
                }
            }
        }
        Act::Player => open(&a.cfg.local_url()),
        Act::Window => window::show(),
        Act::Autostart => {
            if !autostart::set(!autostart::is_enabled()) {
                tray::notify(
                    "Автозапуск",
                    "Не удалось изменить настройку.",
                    Kind::Warning,
                );
            }
        }
        Act::Log => open(&crate::log::path().to_string_lossy()),
        Act::Restart => tray::quit(Exit::Restart),
        Act::Exit => tray::quit(Exit::Quit),
    }
}

/// «1 слушатель», «2 слушателя», «5 слушателей».
fn listeners(n: usize) -> String {
    let word = match (n % 10, n % 100) {
        (1, r) if r != 11 => "слушатель",
        (2..=4, r) if !(12..=14).contains(&r) => "слушателя",
        _ => "слушателей",
    };
    format!("{n} {word}")
}

fn khz(rate: u32) -> String {
    if rate % 1000 == 0 {
        (rate / 1000).to_string()
    } else {
        format!("{:.1}", rate as f64 / 1000.0).replace('.', ",")
    }
}

/// Тёмная тема Windows: `apps` — для окон приложений, иначе — для панели
/// задач и её всплывающих панелей (в Windows их настраивают отдельно).
fn dark_mode(apps: bool) -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide(if apps {
        "AppsUseLightTheme"
    } else {
        "SystemUsesLightTheme"
    });
    let mut v: u32 = 1;
    let mut size = 4u32;
    // SAFETY: буфер — один DWORD, размер передан.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut size,
        )
    };
    r == 0 && v == 0
}

fn os_build() -> u32 {
    // SAFETY: структура с проставленным размером. Манифест объявляет поддержку
    // Windows 10, поэтому GetVersionExW возвращает настоящий номер сборки.
    unsafe {
        let mut v: OSVERSIONINFOW = std::mem::zeroed();
        v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if GetVersionExW(&mut v) == 0 {
            return 0;
        }
        v.dwBuildNumber
    }
}

/// Оформление окна средствами DWM: тёмный заголовок, скругление (Windows 11)
/// и системный фон `backdrop` (Mica = 2, Acrylic = 3; Windows 11 22H2+).
/// Возвращает true, если фон рисует система и наш должен быть прозрачным.
unsafe fn decorate(hwnd: HWND, dark: bool, backdrop: i32, round: bool) -> bool {
    let set = |attr: i32, v: i32| {
        DwmSetWindowAttribute(hwnd, attr as u32, (&v as *const i32).cast(), 4) >= 0
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark as i32);
    let build = os_build();
    if round && build >= 22000 {
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
    }
    if build < 22621 {
        return false;
    }
    let all = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    DwmExtendFrameIntoClientArea(hwnd, &all) >= 0 && set(DWMWA_SYSTEMBACKDROP_TYPE, backdrop)
}

fn copy_text(hwnd: HWND, s: &str) -> bool {
    let w = wide(s);
    // SAFETY: обычная работа с буфером обмена; при успехе памятью владеет он.
    unsafe {
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
                GlobalFree(mem);
            }
        }
        CloseClipboard();
        ok
    }
}

/// Открывает адрес в браузере или файл в связанной программе.
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

/// Размер в физических пикселях для `dip` при данном DPI.
fn px(dip: f32, dpi: u32) -> i32 {
    (dip * dpi as f32 / 96.0).ceil() as i32
}
