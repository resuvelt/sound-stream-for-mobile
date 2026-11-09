//! Панель у значка в трее — как «Быстрые настройки» Windows 11: появляется
//! над панелью задач по клику на значок и прячется, как только уходит фокус.

use std::cell::{Cell, RefCell};
use std::sync::Once;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DWMSBT_TRANSIENTWINDOW;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetForegroundWindow, KillTimer,
    LoadCursorW, RegisterClassExW, SetForegroundWindow, SetTimer, SetWindowPos, HWND_TOPMOST,
    IDC_ARROW, SWP_SHOWWINDOW, WA_INACTIVE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_DESTROY,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SIZE,
    WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::gfx::{Canvas, Font, Rect};
use super::kit::{
    button_w, Hits, Kind, Theme, Ui, G_CHECK, G_COPY, G_DOC, G_PLAY, G_POWER, G_REFRESH, G_SPEAKER,
    G_WINDOW,
};
use super::{copied, Act, Info, Status};
use crate::tray;
use crate::wide::wide;

const CLASS: &str = "sound-stream.flyout";
const TIMER: usize = 1;

// Раскладка (DIP).
const W: f32 = 360.0;
const PAD: f32 = 16.0;
const ADDR_TOP: f32 = 68.0;
const ADDR_ROW: f32 = 34.0;
const FOOT_H: f32 = 52.0;

struct Fly {
    hwnd: HWND,
    canvas: Canvas,
    hits: Hits,
    theme: Theme,
    /// Когда показали: первые мгновения не проверяем фокус.
    shown: Instant,
    closing: bool,
}

thread_local! {
    static FLY: RefCell<Option<Fly>> = const { RefCell::new(None) };
    static CLOSED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Fly) -> R) -> Option<R> {
    FLY.with(|w| {
        let mut w = w.try_borrow_mut().ok()?;
        w.as_mut().map(f)
    })
}

/// Клик по значку: открыть панель или закрыть уже открытую.
pub fn toggle() {
    if let Some(h) = with(|f| f.hwnd) {
        // SAFETY: окно этого потока.
        unsafe { dismiss(h) };
        return;
    }
    // Тот же клик по значку уже закрыл панель (она потеряла фокус раньше,
    // чем пришло сообщение от значка) — значит, её хотели спрятать.
    let just_closed = CLOSED_AT.with(|c| {
        c.get()
            .is_some_and(|t| t.elapsed() < Duration::from_millis(400))
    });
    if !just_closed {
        // SAFETY: всё в потоке интерфейса.
        unsafe { open() };
    }
}

pub fn close() {
    if let Some(h) = with(|f| f.hwnd) {
        // SAFETY: окно этого потока.
        unsafe { dismiss(h) };
    }
}

/// Закрывает панель — один раз: DestroyWindow сам присылает WM_ACTIVATE
/// «неактивна», и без флага панель попыталась бы закрыться повторно.
unsafe fn dismiss(hwnd: HWND) {
    if with(|f| !std::mem::replace(&mut f.closing, true)).unwrap_or(false) {
        DestroyWindow(hwnd);
    }
}

fn height(n: usize) -> f32 {
    ADDR_TOP + addr_h(n) + 10.0 + 24.0 + 10.0 + 32.0 + 8.0 + 40.0 + 8.0 + FOOT_H
}

fn addr_h(n: usize) -> f32 {
    28.0 + n as f32 * ADDR_ROW + 6.0
}

unsafe fn open() {
    let hinst = GetModuleHandleW(std::ptr::null());
    let cls = wide(CLASS);
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            lpszClassName: cls.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
    });

    // Где значок: панель появится рядом с ним, на том же мониторе.
    let anchor = tray::icon_rect().unwrap_or_else(|| {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        RECT {
            left: p.x,
            top: p.y,
            right: p.x,
            bottom: p.y,
        }
    });
    let center = POINT {
        x: (anchor.left + anchor.right) / 2,
        y: (anchor.top + anchor.bottom) / 2,
    };
    let title = wide("Sound Stream");
    // Сначала крошечное окно у значка — чтобы узнать DPI именно этого монитора.
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        cls.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        center.x,
        center.y,
        1,
        1,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinst,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        log!("[ui] не удалось создать панель трея");
        return;
    }
    let dpi = GetDpiForWindow(hwnd);
    let dark = super::dark_mode(false);
    let backdrop = super::decorate(hwnd, dark, DWMSBT_TRANSIENTWINDOW, true);
    FLY.with(|f| {
        *f.borrow_mut() = Some(Fly {
            hwnd,
            canvas: Canvas::new(hwnd, dpi),
            hits: Hits::default(),
            theme: Theme { dark, backdrop },
            shown: Instant::now(),
            closing: false,
        })
    });

    super::refresh_ips();
    let (w, h) = (
        super::px(W, dpi),
        super::px(height(super::info().urls.len()), dpi),
    );
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST), &mut mi);
    let (x, y) = place(center, w, h, mi.rcMonitor, mi.rcWork, super::px(12.0, dpi));
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    SetForegroundWindow(hwnd);
    SetTimer(hwnd, TIMER, 500, None);
}

/// Над панелью задач (или рядом с ней, если она сбоку/сверху), напротив значка.
fn place(icon: POINT, w: i32, h: i32, mon: RECT, work: RECT, m: i32) -> (i32, i32) {
    let fit_x = |x: i32| x.clamp(work.left + m, (work.right - m - w).max(work.left + m));
    let fit_y = |y: i32| y.clamp(work.top + m, (work.bottom - m - h).max(work.top + m));
    if work.bottom < mon.bottom {
        (fit_x(icon.x - w / 2), work.bottom - m - h)
    } else if work.top > mon.top {
        (fit_x(icon.x - w / 2), work.top + m)
    } else if work.left > mon.left {
        (work.left + m, fit_y(icon.y - h / 2))
    } else if work.right < mon.right {
        (work.right - m - w, fit_y(icon.y - h / 2))
    } else {
        // Панель задач скрывается автоматически: просто у значка.
        let below = icon.y < (mon.top + mon.bottom) / 2;
        (
            fit_x(icon.x - w / 2),
            if below {
                work.top + m
            } else {
                work.bottom - m - h
            },
        )
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            with(|f| {
                f.canvas
                    .resize((lp & 0xFFFF) as u32, ((lp >> 16) & 0xFFFF) as u32)
            });
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) as u32 == WA_INACTIVE {
                dismiss(hwnd);
            }
            0
        }
        WM_TIMER => {
            // Фокус так и не пришёл (Windows не дала выйти на передний план) —
            // без него панель не узнает о клике мимо, поэтому прячем её сами.
            let stale = with(|f| f.shown.elapsed() > Duration::from_millis(400)).unwrap_or(false);
            if stale && GetForegroundWindow() != hwnd {
                dismiss(hwnd);
            } else {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_CAPTURECHANGED => {
            let click = with(|f| {
                let dpi = f.canvas.dpi();
                f.hits.mouse(hwnd, msg, lp, dpi)
            })
            .flatten();
            match click {
                // Эти остаются в панели: видно результат.
                Some(act @ (Act::Copy(_) | Act::Autostart)) => {
                    super::perform(act);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                Some(act) => {
                    dismiss(hwnd);
                    super::perform(act);
                }
                None => {}
            }
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            dismiss(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER);
            FLY.with(|f| f.borrow_mut().take());
            CLOSED_AT.with(|c| c.set(Some(Instant::now())));
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    BeginPaint(hwnd, &mut ps);
    let info = super::info();
    with(|f| {
        let Fly {
            canvas,
            hits,
            theme,
            ..
        } = f;
        let t = *theme;
        hits.reset();
        canvas.paint(t.flyout_bg(), |p| {
            let mut ui = Ui { p, t, hits };
            draw(&mut ui, &info);
        });
    });
    EndPaint(hwnd, &ps);
}

fn draw(ui: &mut Ui, info: &Info) {
    let (p, t) = (ui.p, ui.t);

    // Шапка: значок, название, состояние; справа — «открыть окно».
    p.logo(Rect::new(PAD, 16.0, 40.0, 40.0));
    p.text(
        "Sound Stream",
        Rect::new(68.0, 14.0, W - 68.0 - 56.0, 22.0),
        Font::text(15.0).strong(),
        t.text(),
    );
    ui.status_dot(
        72.0,
        46.0,
        info.status.color(t),
        matches!(info.status, Status::Live(_)),
    );
    p.text(
        &info.status.label(),
        Rect::new(82.0, 37.0, W - 82.0 - 56.0, 18.0),
        Font::text(12.0),
        t.text2(),
    );
    ui.icon_button(
        Rect::new(W - PAD - 32.0, 20.0, 32.0, 32.0),
        G_WINDOW,
        Act::Window,
        t.text(),
    );

    // Адреса для телефона.
    let card = Rect::new(12.0, ADDR_TOP, W - 24.0, addr_h(info.urls.len()));
    ui.card(card);
    p.text(
        "Адрес для телефона",
        Rect::new(card.x + 12.0, card.y + 8.0, card.w - 24.0, 18.0),
        Font::text(12.0),
        t.text3(),
    );
    let mut y = card.y + 28.0;
    for (i, url) in info.urls.iter().enumerate() {
        let r = Rect::new(card.x + 12.0, y, card.w - 12.0 - 44.0, 32.0);
        let (f, c) = if i == 0 {
            (Font::text(14.0).strong(), t.text())
        } else {
            (Font::text(13.0), t.text2())
        };
        p.text(url, r, f, c);
        let act = Act::Copy(i as u8);
        let (glyph, gc) = if copied(act) {
            (G_CHECK, t.accent())
        } else {
            (G_COPY, t.text2())
        };
        ui.icon_button(Rect::new(card.r() - 40.0, y, 32.0, 32.0), glyph, act, gc);
        y += ADDR_ROW;
    }

    // Что захватывается.
    let y = card.b() + 10.0;
    let live = info.source.is_some();
    p.icon(
        G_SPEAKER,
        Rect::new(PAD, y, 20.0, 24.0),
        14.0,
        if live { t.accent() } else { t.text3() },
    );
    let line = match (&info.status, &info.source) {
        (Status::PortBusy, _) => format!("Порт {} занят другой программой", info.port),
        (_, Some(s)) => format!("{} · {} кГц", s.device, super::khz(s.sample_rate)),
        _ => "Звук захватывается, пока подключён телефон".into(),
    };
    p.text(
        &line,
        Rect::new(PAD + 26.0, y, W - 2.0 * PAD - 26.0, 24.0),
        Font::text(12.0),
        t.text2(),
    );

    // Кнопки.
    let y = y + 24.0 + 10.0;
    let bw = (W - 2.0 * PAD - 8.0) / 2.0;
    ui.button(
        Rect::new(PAD, y, bw, 32.0),
        "Плеер на ПК",
        Some(G_PLAY),
        Act::Player,
        Kind::Accent,
    );
    ui.button(
        Rect::new(PAD + bw + 8.0, y, bw, 32.0),
        "Перезапустить",
        Some(G_REFRESH),
        Act::Restart,
        Kind::Standard,
    );

    // Автозапуск.
    let y = y + 32.0 + 8.0;
    let row = Rect::new(PAD, y, W - 2.0 * PAD, 40.0);
    p.text(
        "Запускать при входе в Windows",
        Rect::new(row.x, row.y, row.w - 52.0, row.h),
        Font::text(14.0),
        t.text(),
    );
    ui.toggle(row, info.autostart, Act::Autostart);

    // Подвал.
    let y = row.b() + 8.0;
    let foot = Rect::new(0.0, y, W, FOOT_H);
    p.fill(foot, 0.0, t.footer());
    p.hline(0.0, W, y, t.divider());
    let log = "Журнал";
    ui.button(
        Rect::new(PAD - 6.0, y + 10.0, button_w(log, true), 32.0),
        log,
        Some(G_DOC),
        Act::Log,
        Kind::Subtle,
    );
    let quit = "Выход";
    let qw = button_w(quit, true);
    ui.button(
        Rect::new(W - PAD + 6.0 - qw, y + 10.0, qw, 32.0),
        quit,
        Some(G_POWER),
        Act::Exit,
        Kind::Subtle,
    );
}
