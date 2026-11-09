//! Цвета Windows 11 (Fluent) и элементы интерфейса: карточки, кнопки,
//! переключатель. Плюс общий для окон разбор мыши: наведение и нажатие.

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

use super::gfx::{measure, Color, Font, Painter, Rect};
use super::Act;

// Значки Segoe Fluent Icons / Segoe MDL2 Assets.
pub const G_COPY: char = '\u{E8C8}';
pub const G_CHECK: char = '\u{E73E}';
pub const G_PLAY: char = '\u{E768}';
pub const G_WINDOW: char = '\u{E8A7}';
pub const G_DOC: char = '\u{E8A5}';
pub const G_REFRESH: char = '\u{E72C}';
pub const G_POWER: char = '\u{E7E8}';
pub const G_SPEAKER: char = '\u{E7F5}';

/// Палитра под тему Windows. `backdrop` — фон окна рисует сама система
/// (Mica/Acrylic), тогда наш фон прозрачный.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub backdrop: bool,
}

impl Theme {
    fn pick(self, dark: Color, light: Color) -> Color {
        if self.dark {
            dark
        } else {
            light
        }
    }
    pub fn text(self) -> Color {
        self.pick(Color::hex(0xFFFFFF), Color::hexa(0x000000, 0.894))
    }
    pub fn text2(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.786), Color::hexa(0x000000, 0.62))
    }
    pub fn text3(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.544), Color::hexa(0x000000, 0.447))
    }
    pub fn card(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.0512), Color::hexa(0xFFFFFF, 0.7))
    }
    pub fn card_stroke(self) -> Color {
        self.pick(Color::hexa(0x000000, 0.1), Color::hexa(0x000000, 0.0578))
    }
    pub fn divider(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.0837), Color::hexa(0x000000, 0.0803))
    }
    pub fn ctl(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.061), Color::hexa(0xFFFFFF, 0.7))
    }
    pub fn ctl_hover(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.084), Color::hexa(0xF9F9F9, 0.5))
    }
    pub fn ctl_press(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.033), Color::hexa(0xF9F9F9, 0.3))
    }
    pub fn ctl_stroke(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.07), Color::hexa(0x000000, 0.0578))
    }
    pub fn subtle_hover(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.061), Color::hexa(0x000000, 0.037))
    }
    pub fn subtle_press(self) -> Color {
        self.pick(Color::hexa(0xFFFFFF, 0.042), Color::hexa(0x000000, 0.024))
    }
    /// Фирменный зелёный — как у волн на значке и у плеера на телефоне.
    pub fn accent(self) -> Color {
        self.pick(Color::hex(0x30D158), Color::hex(0x128A3A))
    }
    pub fn on_accent(self) -> Color {
        self.pick(Color::hex(0x000000), Color::hex(0xFFFFFF))
    }
    pub fn warn(self) -> Color {
        self.pick(Color::hex(0xFFD60A), Color::hex(0x9D5D00))
    }
    pub fn bad(self) -> Color {
        self.pick(Color::hex(0xFF453A), Color::hex(0xC42B1C))
    }
    /// Фон главного окна, если Mica недоступна (Windows 10).
    pub fn window_bg(self) -> Color {
        if self.backdrop {
            Color::CLEAR
        } else {
            self.pick(Color::hex(0x202020), Color::hex(0xF3F3F3))
        }
    }
    /// Фон панели у значка, если Acrylic недоступен.
    pub fn flyout_bg(self) -> Color {
        if self.backdrop {
            Color::CLEAR
        } else {
            self.pick(Color::hex(0x2C2C2C), Color::hex(0xF9F9F9))
        }
    }
    /// Нижняя полоса панели — темнее основной части, как в «Быстрых настройках».
    pub fn footer(self) -> Color {
        self.pick(Color::hexa(0x000000, 0.22), Color::hexa(0x000000, 0.035))
    }
}

/// Кликабельные области последнего кадра + что под мышью и что нажато.
#[derive(Default)]
pub struct Hits {
    list: Vec<(Rect, Act)>,
    hot: Option<Act>,
    down: Option<Act>,
    tracking: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum St {
    Rest,
    Hover,
    Down,
}

impl Hits {
    pub fn reset(&mut self) {
        self.list.clear();
    }
    pub fn add(&mut self, r: Rect, a: Act) {
        self.list.push((r, a));
    }
    /// Добавленное позже лежит сверху (кнопка внутри строки важнее строки).
    fn at(&self, x: f32, y: f32) -> Option<Act> {
        self.list
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, a)| *a)
    }
    pub fn st(&self, a: Act) -> St {
        match (self.hot == Some(a), self.down) {
            (true, Some(d)) if d == a => St::Down,
            (true, None) => St::Hover,
            _ => St::Rest,
        }
    }

    /// Разбирает сообщение мыши. Возвращает действие, если по нему кликнули
    /// (нажали и отпустили над одним и тем же элементом).
    pub unsafe fn mouse(&mut self, hwnd: HWND, msg: u32, lp: LPARAM, dpi: f32) -> Option<Act> {
        let k = 96.0 / dpi;
        let x = (lp & 0xFFFF) as i16 as f32 * k;
        let y = ((lp >> 16) & 0xFFFF) as i16 as f32 * k;
        let before = (self.hot, self.down);
        let mut click = None;
        match msg {
            WM_MOUSEMOVE => {
                self.hot = self.at(x, y);
                if !self.tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    self.tracking = TrackMouseEvent(&mut t) != 0;
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hot = None;
            }
            WM_LBUTTONDOWN => {
                self.hot = self.at(x, y);
                self.down = self.hot;
                if self.down.is_some() {
                    SetCapture(hwnd);
                }
            }
            WM_LBUTTONUP => {
                let a = self.at(x, y);
                if self.down.is_some() && self.down == a {
                    click = a;
                }
                self.down = None;
                self.hot = a;
                ReleaseCapture();
            }
            WM_CAPTURECHANGED => self.down = None,
            _ => {}
        }
        if (self.hot, self.down) != before {
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
        click
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Обычная кнопка с подложкой и рамкой.
    Standard,
    /// Главное действие — фирменным цветом.
    Accent,
    /// Без подложки, только подсветка при наведении.
    Subtle,
}

const BUTTON_FONT: Font = Font::text(14.0);

/// Ширина кнопки под подпись (и значок).
pub fn button_w(label: &str, icon: bool) -> f32 {
    measure(label, BUTTON_FONT).ceil() + if icon { 24.0 } else { 0.0 } + 24.0
}

/// Один кадр: на чём рисуем, в какой теме и куда записывать кликабельные места.
pub struct Ui<'a, 'p> {
    pub p: &'a Painter<'p>,
    pub t: Theme,
    pub hits: &'a mut Hits,
}

impl Ui<'_, '_> {
    pub fn card(&self, r: Rect) {
        self.p.fill(r, 8.0, self.t.card());
        self.p.stroke(r, 8.0, self.t.card_stroke(), self.p.px());
    }

    pub fn button(&mut self, r: Rect, label: &str, icon: Option<char>, act: Act, kind: Kind) {
        self.hits.add(r, act);
        let st = self.hits.st(act);
        let t = self.t;
        let (bg, fg) = match kind {
            Kind::Accent => {
                let k = match st {
                    St::Rest => 1.0,
                    St::Hover => 0.9,
                    St::Down => 0.8,
                };
                (t.accent().alpha(k), t.on_accent())
            }
            Kind::Standard => match st {
                St::Rest => (t.ctl(), t.text()),
                St::Hover => (t.ctl_hover(), t.text()),
                St::Down => (t.ctl_press(), t.text2()),
            },
            Kind::Subtle => match st {
                St::Rest => (Color::CLEAR, t.text()),
                St::Hover => (t.subtle_hover(), t.text()),
                St::Down => (t.subtle_press(), t.text2()),
            },
        };
        self.p.fill(r, 4.0, bg);
        if kind == Kind::Standard {
            self.p.stroke(r, 4.0, t.ctl_stroke(), self.p.px());
        }
        let lw = measure(label, BUTTON_FONT).ceil();
        let iw = if icon.is_some() { 24.0 } else { 0.0 };
        let x = (r.cx() - (lw + iw) / 2.0).max(r.x + 8.0);
        if let Some(g) = icon {
            self.p.icon(g, Rect::new(x, r.y, 16.0, r.h), 16.0, fg);
        }
        let tw = (lw + 4.0).min(r.r() - 8.0 - x - iw);
        self.p
            .text(label, Rect::new(x + iw, r.y, tw, r.h), BUTTON_FONT, fg);
    }

    /// Квадратная кнопка-значок без подложки.
    pub fn icon_button(&mut self, r: Rect, glyph: char, act: Act, color: Color) {
        self.hits.add(r, act);
        let st = self.hits.st(act);
        let bg = match st {
            St::Rest => Color::CLEAR,
            St::Hover => self.t.subtle_hover(),
            St::Down => self.t.subtle_press(),
        };
        self.p.fill(r, 4.0, bg);
        let fg = if st == St::Down {
            color.alpha(0.75)
        } else {
            color
        };
        self.p.icon(glyph, r, 16.0, fg);
    }

    /// Переключатель Windows 11 у правого края `row`; кликается вся строка.
    pub fn toggle(&mut self, row: Rect, on: bool, act: Act) {
        self.hits.add(row, act);
        let st = self.hits.st(act);
        let t = self.t;
        let sw = Rect::new(row.r() - 40.0, row.cy() - 10.0, 40.0, 20.0);
        let knob = match st {
            St::Rest => 6.0,
            _ => 7.0,
        };
        if on {
            let k = match st {
                St::Rest => 1.0,
                St::Hover => 0.9,
                St::Down => 0.8,
            };
            self.p.fill(sw, 10.0, t.accent().alpha(k));
            self.p.circle(sw.r() - 10.0, sw.cy(), knob, t.on_accent());
        } else {
            let bg = match st {
                St::Rest => Color::CLEAR,
                St::Hover => t.subtle_hover(),
                St::Down => t.subtle_press(),
            };
            self.p.fill(sw, 10.0, bg);
            self.p.stroke(sw, 10.0, t.text3(), 1.0);
            self.p.circle(sw.x + 10.0, sw.cy(), knob - 1.0, t.text2());
        }
    }

    /// Цветная точка состояния; в эфире — с мягким ореолом.
    pub fn status_dot(&self, cx: f32, cy: f32, c: Color, glow: bool) {
        if glow {
            self.p.circle(cx, cy, 7.0, c.alpha(0.25));
        }
        self.p.circle(cx, cy, 4.0, c);
    }
}
