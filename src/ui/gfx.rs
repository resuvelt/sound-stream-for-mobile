//! Рисование через Direct2D + DirectWrite. Координаты — в DIP (1/96 дюйма),
//! поэтому окна одинаково чёткие при любом масштабе экрана.

use std::cell::RefCell;
use std::ffi::c_void;

use qrcodegen::QrCode;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::Vector2;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::wide::wide;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn r(&self) -> f32 {
        self.x + self.w
    }
    pub fn b(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.r() && y >= self.y && y < self.b()
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.r(),
            bottom: self.b(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const CLEAR: Color = Color::hexa(0, 0.0);

    pub const fn hex(rgb: u32) -> Self {
        Self::hexa(rgb, 1.0)
    }
    pub const fn hexa(rgb: u32, a: f32) -> Self {
        Color {
            r: ((rgb >> 16) & 0xff) as f32 / 255.0,
            g: ((rgb >> 8) & 0xff) as f32 / 255.0,
            b: (rgb & 0xff) as f32 / 255.0,
            a,
        }
    }
    pub const fn alpha(self, k: f32) -> Self {
        Color {
            a: self.a * k,
            ..self
        }
    }
    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F {
            r: self.r,
            g: self.g,
            b: self.b,
            a: self.a,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Face {
    Text,
    Display,
    Icon,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: f32,
    pub weight: i32,
    pub face: Face,
    pub align: Align,
    pub wrap: bool,
}

impl Font {
    pub const fn text(size: f32) -> Self {
        Font {
            size,
            weight: 400,
            face: Face::Text,
            align: Align::Left,
            wrap: false,
        }
    }
    pub const fn icon(size: f32) -> Self {
        Font {
            size,
            weight: 400,
            face: Face::Icon,
            align: Align::Center,
            wrap: false,
        }
    }
    pub const fn strong(self) -> Self {
        Font {
            weight: 600,
            ..self
        }
    }
    pub const fn display(self) -> Self {
        Font {
            face: Face::Display,
            ..self
        }
    }
    pub const fn right(self) -> Self {
        Font {
            align: Align::Right,
            ..self
        }
    }
    pub const fn wrap(self) -> Self {
        Font { wrap: true, ..self }
    }
}

/// Фабрики Direct2D/DirectWrite и кэш шрифтов — одни на поток интерфейса.
struct Shared {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    /// Text, Display, Icon — в порядке `Face`.
    families: [Vec<u16>; 3],
    formats: RefCell<Vec<(Font, IDWriteTextFormat)>>,
}

thread_local! {
    static SHARED: Option<Shared> = unsafe { Shared::new() };
}

fn with_shared<R>(f: impl FnOnce(&Shared) -> R) -> Option<R> {
    SHARED.with(|s| s.as_ref().map(f))
}

impl Shared {
    unsafe fn new() -> Option<Self> {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
            .map_err(|e| log!("[ui] Direct2D недоступен: {e}"))
            .ok()?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| log!("[ui] DirectWrite недоступен: {e}"))
            .ok()?;
        let mut fonts = None;
        let _ = dw.GetSystemFontCollection(&mut fonts, false);
        // Шрифты Windows 11, а на Windows 10 — их предшественники.
        let has = |name: &str| {
            let Some(c) = fonts.as_ref() else {
                return false;
            };
            let n = wide(name);
            let (mut i, mut found) = (0u32, BOOL(0));
            c.FindFamilyName(PCWSTR(n.as_ptr()), &mut i, &mut found)
                .is_ok()
                && found.as_bool()
        };
        let pick = |a: &str, b: &str| wide(if has(a) { a } else { b });
        let families = [
            pick("Segoe UI Variable Text", "Segoe UI"),
            pick("Segoe UI Variable Display", "Segoe UI"),
            pick("Segoe Fluent Icons", "Segoe MDL2 Assets"),
        ];
        Some(Shared {
            d2d,
            dw,
            families,
            formats: RefCell::new(Vec::new()),
        })
    }

    unsafe fn format(&self, f: Font) -> Option<IDWriteTextFormat> {
        if let Some((_, tf)) = self.formats.borrow().iter().find(|(k, _)| *k == f) {
            return Some(tf.clone());
        }
        let family = &self.families[f.face as usize];
        let tf = self
            .dw
            .CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT(f.weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                f.size,
                w!("ru-ru"),
            )
            .ok()?;
        let _ = tf.SetTextAlignment(match f.align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        });
        if f.wrap {
            let _ = tf.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = tf.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        } else {
            // Одна строка по центру прямоугольника; не влезла — многоточие.
            let _ = tf.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = tf.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&tf) {
                let t = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = tf.SetTrimming(&t, &sign);
            }
        }
        self.formats.borrow_mut().push((f, tf.clone()));
        Some(tf)
    }

    unsafe fn metrics(&self, s: &str, f: Font, max_w: f32) -> Option<DWRITE_TEXT_METRICS> {
        let tf = self.format(f)?;
        let text: Vec<u16> = s.encode_utf16().collect();
        let layout = self.dw.CreateTextLayout(&text, &tf, max_w, 4096.0).ok()?;
        let mut m = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut m).ok()?;
        Some(m)
    }
}

/// Ширина однострочного текста.
pub fn measure(s: &str, f: Font) -> f32 {
    let f = Font {
        align: Align::Left,
        wrap: false,
        ..f
    };
    with_shared(|sh| unsafe { sh.metrics(s, f, 8192.0) })
        .flatten()
        .map(|m| m.widthIncludingTrailingWhitespace)
        .unwrap_or(s.chars().count() as f32 * f.size * 0.55)
}

/// Холст окна: render target Direct2D, создаётся при первой отрисовке и
/// пересоздаётся, если видеокарта «потерялась» (смена драйвера, RDP).
pub struct Canvas {
    hwnd: *mut c_void,
    rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    round: Option<ID2D1StrokeStyle>,
    dpi: f32,
}

impl Canvas {
    pub fn new(hwnd: *mut c_void, dpi: u32) -> Self {
        Canvas {
            hwnd,
            rt: None,
            brush: None,
            round: None,
            dpi: dpi as f32,
        }
    }

    pub fn dpi(&self) -> f32 {
        self.dpi
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi as f32;
        if let Some(rt) = &self.rt {
            unsafe { rt.SetDpi(self.dpi, self.dpi) };
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if let Some(rt) = &self.rt {
            let _ = unsafe {
                rt.Resize(&D2D_SIZE_U {
                    width: w,
                    height: h,
                })
            };
        }
    }

    unsafe fn create(&mut self, sh: &Shared) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(self.hwnd, &mut rc);
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                // С альфой: сквозь прозрачный фон виден Mica/Acrylic.
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: self.dpi,
            dpiY: self.dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: HWND(self.hwnd),
            pixelSize: D2D_SIZE_U {
                width: (rc.right - rc.left).max(1) as u32,
                height: (rc.bottom - rc.top).max(1) as u32,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = match sh.d2d.CreateHwndRenderTarget(&props, &hwnd_props) {
            Ok(rt) => rt,
            Err(e) => {
                log!("[ui] не удалось создать холст Direct2D: {e}");
                return;
            }
        };
        let Ok(brush) = rt.CreateSolidColorBrush(&Color::CLEAR.d2d(), None) else {
            return;
        };
        if self.round.is_none() {
            let sp = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            self.round = sh.d2d.CreateStrokeStyle(&sp, None).ok();
        }
        self.rt = Some(rt);
        self.brush = Some(brush);
    }

    /// Рисует кадр целиком: фон `clear`, затем `draw`.
    pub fn paint(&mut self, clear: Color, draw: impl FnOnce(&Painter)) {
        with_shared(|sh| unsafe {
            if self.rt.is_none() {
                self.create(sh);
            }
            let lost = {
                let (Some(rt), Some(brush), Some(round)) = (&self.rt, &self.brush, &self.round)
                else {
                    return;
                };
                rt.BeginDraw();
                // ClearType на полупрозрачном фоне невозможен — сглаживание в оттенках серого.
                rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                rt.Clear(Some(&clear.d2d()));
                draw(&Painter {
                    rt,
                    brush,
                    round,
                    sh,
                    dpi: self.dpi,
                });
                matches!(rt.EndDraw(None, None), Err(e) if e.code() == D2DERR_RECREATE_TARGET)
            };
            if lost {
                self.rt = None;
                self.brush = None;
            }
        });
    }
}

pub struct Painter<'a> {
    rt: &'a ID2D1HwndRenderTarget,
    brush: &'a ID2D1SolidColorBrush,
    round: &'a ID2D1StrokeStyle,
    sh: &'a Shared,
    dpi: f32,
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

impl Painter<'_> {
    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c.d2d()) };
        self.brush
    }

    /// Один физический пиксель в DIP — толщина тонких линий.
    pub fn px(&self) -> f32 {
        96.0 / self.dpi
    }

    pub fn fill(&self, r: Rect, radius: f32, c: Color) {
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            if radius > 0.0 {
                let rr = D2D1_ROUNDED_RECT {
                    rect: r.d2d(),
                    radiusX: radius,
                    radiusY: radius,
                };
                self.rt.FillRoundedRectangle(&rr, self.brush(c));
            } else {
                self.rt.FillRectangle(&r.d2d(), self.brush(c));
            }
        }
    }

    pub fn stroke(&self, r: Rect, radius: f32, c: Color, width: f32) {
        let rr = D2D1_ROUNDED_RECT {
            rect: r.inset(width / 2.0).d2d(),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.rt
                .DrawRoundedRectangle(&rr, self.brush(c), width, None)
        };
    }

    /// Горизонтальная линия толщиной в один пиксель.
    pub fn hline(&self, x0: f32, x1: f32, y: f32, c: Color) {
        self.fill(Rect::new(x0, y, x1 - x0, self.px()), 0.0, c);
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let e = D2D1_ELLIPSE {
            point: pt(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.rt.FillEllipse(&e, self.brush(c)) };
    }

    pub fn text(&self, s: &str, r: Rect, f: Font, c: Color) {
        if s.is_empty() {
            return;
        }
        let Some(tf) = (unsafe { self.sh.format(f) }) else {
            return;
        };
        let text: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.rt.DrawText(
                &text,
                &tf,
                &r.d2d(),
                self.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    /// Значок из шрифта Segoe Fluent Icons (на Windows 10 — Segoe MDL2 Assets).
    pub fn icon(&self, glyph: char, r: Rect, size: f32, c: Color) {
        let mut buf = [0u8; 4];
        self.text(glyph.encode_utf8(&mut buf), r, Font::icon(size), c);
    }

    /// QR-код на белой плашке. Модули выровнены по пикселям экрана, иначе
    /// между ними видны тонкие щели и камере сложнее его прочитать.
    pub fn qr(&self, r: Rect, code: &QrCode) {
        self.fill(r, 8.0, Color::hex(0xFFFFFF));
        let s = self.dpi / 96.0;
        let n = code.size();
        let quiet = 3;
        let tile = (r.w * s).floor();
        let m = (tile / (n + 2 * quiet) as f32).floor().max(1.0);
        let off = ((tile - m * n as f32) / 2.0).floor();
        let (x0, y0) = ((r.x * s).round() + off, (r.y * s).round() + off);
        let brush = self.brush(Color::hex(0x000000));
        unsafe {
            self.rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            for y in 0..n {
                let mut x = 0;
                while x < n {
                    if !code.get_module(x, y) {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < n && code.get_module(x, y) {
                        x += 1;
                    }
                    let rect = D2D_RECT_F {
                        left: (x0 + start as f32 * m) / s,
                        top: (y0 + y as f32 * m) / s,
                        right: (x0 + x as f32 * m) / s,
                        bottom: (y0 + (y + 1) as f32 * m) / s,
                    };
                    self.rt.FillRectangle(&rect, brush);
                }
            }
            self.rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
    }

    /// Значок приложения (как assets/sound-stream.ico): динамик и две
    /// зелёные волны на тёмной плашке. Координаты — из макета 512×512.
    pub fn logo(&self, r: Rect) {
        let k = r.w / 512.0;
        let p = |x: f32, y: f32| pt(r.x + x * k, r.y + y * k);
        self.fill(r, r.w * 0.22, Color::hex(0x1C1C1E));
        self.stroke(r, r.w * 0.22, Color::hexa(0xFFFFFF, 0.08), self.px());
        unsafe {
            let Ok(speaker) = self.sh.d2d.CreatePathGeometry() else {
                return;
            };
            if let Ok(sink) = speaker.Open() {
                sink.BeginFigure(p(136.0, 216.0), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[
                    p(188.0, 216.0),
                    p(252.0, 154.0),
                    p(252.0, 358.0),
                    p(188.0, 296.0),
                    p(136.0, 296.0),
                ]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                let _ = sink.Close();
            }
            self.rt
                .FillGeometry(&speaker, self.brush(Color::hex(0xFFFFFF)), None);

            let Ok(waves) = self.sh.d2d.CreatePathGeometry() else {
                return;
            };
            if let Ok(sink) = waves.Open() {
                for (x, y0, y1, rad) in [(279.0, 208.0, 298.0, 52.0), (313.0, 176.0, 334.0, 98.0)] {
                    sink.BeginFigure(p(x, y0), D2D1_FIGURE_BEGIN_HOLLOW);
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: p(x, y1),
                        size: D2D_SIZE_F {
                            width: rad * k,
                            height: rad * k,
                        },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: D2D1_ARC_SIZE_SMALL,
                    });
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
                let _ = sink.Close();
            }
            self.rt.DrawGeometry(
                &waves,
                self.brush(Color::hex(0x30D158)),
                22.0 * k,
                self.round,
            );
        }
    }
}
