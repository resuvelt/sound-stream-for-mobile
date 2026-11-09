//! Главное окно: QR-код и адреса для телефона, что сейчас захватывается,
//! настройки. Закрытие окна не останавливает сервер — он остаётся в трее.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use qrcodegen::{QrCode, QrCodeEcc};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DWMSBT_MAINWINDOW;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, IsIconic, KillTimer,
    LoadCursorW, LoadImageW, PostMessageW, RegisterClassExW, SetForegroundWindow, SetTimer,
    SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HICON, IDC_ARROW, IMAGE_ICON,
    LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, SW_RESTORE, SW_SHOW, SW_SHOWNORMAL, WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_SETTINGCHANGE, WM_SIZE, WM_THEMECHANGED, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU,
};

use super::gfx::{measure, Font, Rect};
use super::kit::{
    button_w, Hits, Kind, St, Theme, Ui, G_CHECK, G_COPY, G_PLAY, G_POWER, G_SPEAKER,
};
use super::{copied, Act, Info, Status};
use crate::tray;
use crate::wide::wide;

const CLASS: &str = "sound-stream.window";
const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
const TIMER: usize = 1;

// Раскладка (DIP).
const W: f32 = 760.0;
const PAD: f32 = 20.0;
const TOP: f32 = 84.0;
const QR: f32 = 152.0;
const ADDR_ROW: f32 = 52.0;
const LEFT_W: f32 = 400.0;
const GAP: f32 = 16.0;
const SRC_H: f32 = 116.0;
const SET_ROW: f32 = 60.0;

struct Win {
    hwnd: HWND,
    canvas: super::gfx::Canvas,
    hits: Hits,
    theme: Theme,
    /// Какой адрес сейчас в QR-коде.
    sel: usize,
    qr: Option<(String, QrCode)>,
    height: f32,
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
}

/// Подсказка «окно закрыто, но всё работает» — один раз за запуск.
static TOLD: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Win) -> R) -> Option<R> {
    WIN.with(|w| {
        let mut w = w.try_borrow_mut().ok()?;
        w.as_mut().map(f)
    })
}

pub fn is_open() -> bool {
    WIN.with(|w| w.try_borrow().map(|w| w.is_some()).unwrap_or(true))
}

/// Показывает окно (создаёт, если его нет) и выводит на передний план.
pub fn show() {
    let existing = with(|w| w.hwnd);
    // SAFETY: окно живёт в этом потоке; дескриптор проверен выше.
    unsafe {
        match existing {
            Some(h) => {
                ShowWindow(
                    h,
                    if IsIconic(h) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                SetForegroundWindow(h);
            }
            None => create(),
        }
    }
}

unsafe fn create() {
    let hinst = GetModuleHandleW(std::ptr::null());
    let cls = wide(CLASS);
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let icon = |w, h| {
            LoadImageW(
                hinst,
                1 as _, // ресурс иконки №1 (build.rs)
                IMAGE_ICON,
                GetSystemMetrics(w),
                GetSystemMetrics(h),
                LR_DEFAULTCOLOR,
            ) as HICON
        };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: icon(SM_CXICON, SM_CYICON),
            hIconSm: icon(SM_CXSMICON, SM_CYSMICON),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            lpszClassName: cls.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
    });
    let title = wide("Sound Stream");
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        title.as_ptr(),
        STYLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinst,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        log!("[ui] не удалось создать окно");
        return;
    }
    let dark = super::dark_mode(true);
    let backdrop = super::decorate(hwnd, dark, DWMSBT_MAINWINDOW, false);
    WIN.with(|w| {
        *w.borrow_mut() = Some(Win {
            hwnd,
            canvas: super::gfx::Canvas::new(hwnd, GetDpiForWindow(hwnd)),
            hits: Hits::default(),
            theme: Theme { dark, backdrop },
            sel: 0,
            qr: None,
            height: 0.0,
        })
    });
    super::refresh_ips();
    fit(hwnd, true);
    ShowWindow(hwnd, SW_SHOWNORMAL);
    SetForegroundWindow(hwnd);
    SetTimer(hwnd, TIMER, 500, None);
}

/// Высота клиентской области под `n` адресов.
fn height(n: usize) -> f32 {
    TOP + content_h(n) + GAP + 32.0 + PAD
}

fn content_h(n: usize) -> f32 {
    let left = 16.0 + QR + 12.0 + n as f32 * ADDR_ROW + 8.0;
    let right = SRC_H + 12.0 + 3.0 * SET_ROW;
    left.max(right)
}

/// Подгоняет размер окна под содержимое (и центрирует при `center`).
unsafe fn fit(hwnd: HWND, center: bool) {
    let h = height(super::info().urls.len());
    with(|w| w.height = h);
    let dpi = GetDpiForWindow(hwnd);
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: super::px(W, dpi),
        bottom: super::px(h, dpi),
    };
    AdjustWindowRectExForDpi(&mut rc, STYLE, 0, 0, dpi);
    let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
    if !center {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            ww,
            wh,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
        );
        return;
    }
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
    let wa = mi.rcWork;
    let x = wa.left + ((wa.right - wa.left) - ww) / 2;
    let y = (wa.top + ((wa.bottom - wa.top) - wh) / 2).max(wa.top);
    SetWindowPos(
        hwnd,
        std::ptr::null_mut(),
        x,
        y,
        ww,
        wh,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            with(|w| {
                w.canvas
                    .resize((lp & 0xFFFF) as u32, ((lp >> 16) & 0xFFFF) as u32)
            });
            0
        }
        WM_DPICHANGED => {
            with(|w| w.canvas.set_dpi((wp & 0xFFFF) as u32));
            let r = &*(lp as *const RECT);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            fit(hwnd, false);
            0
        }
        WM_TIMER => {
            // Число адресов поменялось (подключили кабель, VPN) — меняем высоту.
            let h = height(super::info().urls.len());
            if with(|w| (w.height - h).abs() > 0.5).unwrap_or(false) {
                fit(hwnd, false);
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
            0
        }
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_CAPTURECHANGED => {
            let click = with(|w| {
                let dpi = w.canvas.dpi();
                w.hits.mouse(hwnd, msg, lp, dpi)
            })
            .flatten();
            if let Some(act) = click {
                match act {
                    Act::Select(i) => {
                        with(|w| w.sel = i as usize);
                    }
                    _ => super::perform(act),
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            // Переключили светлую/тёмную тему Windows.
            let dark = super::dark_mode(true);
            if with(|w| w.theme.dark != dark).unwrap_or(false) {
                let backdrop = super::decorate(hwnd, dark, DWMSBT_MAINWINDOW, false);
                with(|w| w.theme = Theme { dark, backdrop });
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER);
            WIN.with(|w| w.borrow_mut().take());
            if !TOLD.swap(true, Ordering::SeqCst) && !tray::quitting() {
                tray::notify(
                    "Sound Stream работает в трее",
                    "Окно закрыто, звук на телефон идёт дальше. Значок — у часов.",
                    tray::Kind::Info,
                );
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    BeginPaint(hwnd, &mut ps);
    let info = super::info();
    with(|w| {
        let Win {
            canvas,
            hits,
            theme,
            sel,
            qr,
            ..
        } = w;
        *sel = (*sel).min(info.urls.len() - 1);
        let url = &info.urls[*sel];
        if qr.as_ref().is_none_or(|(u, _)| u != url) {
            *qr = QrCode::encode_text(url, QrCodeEcc::Medium)
                .ok()
                .map(|c| (url.clone(), c));
        }
        let t = *theme;
        hits.reset();
        canvas.paint(t.window_bg(), |p| {
            let mut ui = Ui { p, t, hits };
            draw(&mut ui, &info, *sel, qr.as_ref().map(|(_, c)| c));
        });
    });
    EndPaint(hwnd, &ps);
}

fn draw(ui: &mut Ui, info: &Info, sel: usize, qr: Option<&QrCode>) {
    let (p, t) = (ui.p, ui.t);

    // Заголовок: значок, название, состояние справа.
    p.logo(Rect::new(PAD, 20.0, 44.0, 44.0));
    p.text(
        "Sound Stream",
        Rect::new(76.0, 17.0, 360.0, 28.0),
        Font::text(20.0).strong().display(),
        t.text(),
    );
    p.text(
        "Звук этого компьютера — в браузере телефона",
        Rect::new(76.0, 44.0, 400.0, 20.0),
        Font::text(13.0),
        t.text3(),
    );
    let label = info.status.label();
    let f = Font::text(13.0);
    let pw = measure(&label, f).ceil() + 44.0;
    let pill = Rect::new(W - PAD - pw, 26.0, pw, 32.0);
    p.fill(pill, 16.0, t.card());
    p.stroke(pill, 16.0, t.card_stroke(), p.px());
    ui.status_dot(
        pill.x + 18.0,
        pill.cy(),
        info.status.color(t),
        matches!(info.status, Status::Live(_)),
    );
    p.text(
        &label,
        Rect::new(pill.x + 30.0, pill.y, pw - 30.0, pill.h),
        f,
        t.text(),
    );

    let h = content_h(info.urls.len());

    // Слева — телефон: QR-код и адреса.
    let card = Rect::new(PAD, TOP, LEFT_W, h);
    ui.card(card);
    if let Some(code) = qr {
        p.qr(Rect::new(card.x + 16.0, card.y + 16.0, QR, QR), code);
    }
    let tx = card.x + 16.0 + QR + 20.0;
    let tw = card.r() - 16.0 - tx;
    p.text(
        "Слушать на телефоне",
        Rect::new(tx, card.y + 18.0, tw, 22.0),
        Font::text(14.0).strong(),
        t.text(),
    );
    p.text(
        "Наведи камеру телефона на QR-код или открой адрес в браузере. \
         Телефон и компьютер должны быть в одной Wi-Fi сети.",
        Rect::new(tx, card.y + 46.0, tw, QR - 30.0),
        Font::text(13.0).wrap(),
        t.text2(),
    );
    let mut y = card.y + 16.0 + QR + 12.0;
    for (i, url) in info.urls.iter().enumerate() {
        address_row(
            ui,
            Rect::new(card.x + 8.0, y, card.w - 16.0, ADDR_ROW),
            i,
            url,
            i == sel,
        );
        y += ADDR_ROW;
    }

    // Справа — что захватывается и настройки.
    let rx = card.r() + GAP;
    let rw = W - PAD - rx;
    source_card(ui, Rect::new(rx, TOP, rw, SRC_H), info);
    settings_card(
        ui,
        Rect::new(rx, TOP + SRC_H + 12.0, rw, h - SRC_H - 12.0),
        info,
    );

    // Низ: главное действие слева, выход справа.
    let fy = TOP + h + GAP;
    let play = "Открыть плеер на этом ПК";
    let bw = button_w(play, true);
    ui.button(
        Rect::new(PAD, fy, bw, 32.0),
        play,
        Some(G_PLAY),
        Act::Player,
        Kind::Accent,
    );
    let quit = "Выйти";
    let qw = button_w(quit, true);
    ui.button(
        Rect::new(W - PAD - qw, fy, qw, 32.0),
        quit,
        Some(G_POWER),
        Act::Exit,
        Kind::Standard,
    );
}

/// Строка адреса: клик показывает его в QR-коде, справа — «скопировать».
fn address_row(ui: &mut Ui, r: Rect, i: usize, url: &str, selected: bool) {
    let t = ui.t;
    let act = Act::Select(i as u8);
    ui.hits.add(r, act);
    let bg = match (selected, ui.hits.st(act)) {
        (true, _) => t.ctl(),
        (false, St::Hover) => t.subtle_hover(),
        (false, St::Down) => t.subtle_press(),
        (false, St::Rest) => super::gfx::Color::CLEAR,
    };
    ui.p.fill(r, 4.0, bg);
    if selected {
        ui.p.fill(Rect::new(r.x, r.cy() - 8.0, 3.0, 16.0), 1.5, t.accent());
    }
    let tx = r.x + 14.0;
    let tw = r.w - 14.0 - 48.0;
    ui.p.text(
        url,
        Rect::new(tx, r.y + 7.0, tw, 20.0),
        Font::text(14.0).strong(),
        t.text(),
    );
    let copy = Act::Copy(i as u8);
    let (note, color) = if copied(copy) {
        ("Скопировано в буфер обмена", t.accent())
    } else if i == 0 {
        ("Постоянный — добавь его в закладки", t.text3())
    } else {
        ("По IP — если адрес .local не открывается", t.text3())
    };
    ui.p.text(
        note,
        Rect::new(tx, r.y + 27.0, tw, 18.0),
        Font::text(12.0),
        color,
    );
    let (glyph, gc) = if copied(copy) {
        (G_CHECK, t.accent())
    } else {
        (G_COPY, t.text())
    };
    ui.icon_button(
        Rect::new(r.r() - 40.0, r.cy() - 16.0, 32.0, 32.0),
        glyph,
        copy,
        gc,
    );
}

fn source_card(ui: &mut Ui, r: Rect, info: &Info) {
    let (p, t) = (ui.p, ui.t);
    ui.card(r);
    let tile = Rect::new(r.x + 16.0, r.y + 16.0, 36.0, 36.0);
    p.fill(tile, 6.0, t.ctl());
    let live = info.source.is_some();
    p.icon(
        G_SPEAKER,
        tile,
        18.0,
        if live { t.accent() } else { t.text3() },
    );
    let (title, detail) = match (&info.status, &info.source) {
        (Status::PortBusy, _) => (
            format!("Порт {} занят", info.port),
            "Жду, пока он освободится".to_string(),
        ),
        (_, Some(s)) => (
            s.device.clone(),
            format!("{} кГц · моно", super::khz(s.sample_rate)),
        ),
        (Status::Starting, None) => ("Сервер запускается…".into(), String::new()),
        (_, None) => (
            "Звук пока не захватывается".into(),
            "Начнётся с первым слушателем".into(),
        ),
    };
    let tx = tile.r() + 12.0;
    let tw = r.r() - 16.0 - tx;
    p.text(
        &title,
        Rect::new(tx, r.y + 14.0, tw, 20.0),
        Font::text(14.0).strong(),
        t.text(),
    );
    p.text(
        &detail,
        Rect::new(tx, r.y + 34.0, tw, 18.0),
        Font::text(12.0),
        t.text2(),
    );
    p.hline(r.x + 16.0, r.r() - 16.0, r.y + 68.0, t.divider());
    let row = Rect::new(r.x + 16.0, r.y + 68.0, r.w - 32.0, r.h - 68.0);
    p.text("Слушатели", row, Font::text(14.0), t.text2());
    let n = match info.status {
        Status::Live(n) => n.to_string(),
        _ => "нет".into(),
    };
    p.text(&n, row, Font::text(14.0).strong().right(), t.text());
}

fn settings_card(ui: &mut Ui, r: Rect, info: &Info) {
    ui.card(r);
    let t = ui.t;
    let row = |i: usize| Rect::new(r.x + 16.0, r.y + i as f32 * SET_ROW, r.w - 32.0, SET_ROW);
    let caption = |ui: &Ui, rr: Rect, title: &str, note: &str, right: f32| {
        let w = rr.w - right;
        ui.p.text(
            title,
            Rect::new(rr.x, rr.y + 11.0, w, 20.0),
            Font::text(14.0),
            t.text(),
        );
        ui.p.text(
            note,
            Rect::new(rr.x, rr.y + 31.0, w, 18.0),
            Font::text(12.0),
            t.text2(),
        );
    };

    let r0 = row(0);
    caption(ui, r0, "Автозапуск", "При входе в Windows, без окна", 56.0);
    ui.toggle(r0, info.autostart, Act::Autostart);
    ui.p.hline(r.x + 16.0, r.r() - 16.0, r0.b(), t.divider());

    let r1 = row(1);
    let open = "Открыть";
    let ow = button_w(open, false);
    caption(ui, r1, "Журнал", "Если что-то не работает", ow + 12.0);
    ui.button(
        Rect::new(r1.r() - ow, r1.cy() - 16.0, ow, 32.0),
        open,
        None,
        Act::Log,
        Kind::Standard,
    );
    ui.p.hline(r.x + 16.0, r.r() - 16.0, r1.b(), t.divider());

    let r2 = row(2);
    let restart = "Перезапустить";
    let rw = button_w(restart, false);
    caption(ui, r2, "Сервер", &format!("Порт {}", info.port), rw + 12.0);
    ui.button(
        Rect::new(r2.r() - rw, r2.cy() - 16.0, rw, 32.0),
        restart,
        None,
        Act::Restart,
        Kind::Standard,
    );
}
