//! Colours, rectangles, fonts and the drawing shortcuts everything else paints with.

use std::sync::Arc;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_GAMMA_2_2, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT, ID2D1Bitmap, ID2D1Factory,
    ID2D1GradientStopCollection, ID2D1HwndRenderTarget, ID2D1RadialGradientBrush,
    ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;
use windows::core::{Interface, PCWSTR, Result, w};
use windows_numerics::Vector2;

use unkludge_protocol::screen::Animation;

use crate::layout::Rect;

const fn rgb(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a,
    }
}

pub const BG: D2D1_COLOR_F = rgb(0x0B0B0F, 1.0);
pub const CARD: D2D1_COLOR_F = rgb(0x15151B, 1.0);
pub const WELL: D2D1_COLOR_F = rgb(0x0D0D12, 1.0);
pub const SURFACE: D2D1_COLOR_F = rgb(0x1F1F27, 1.0);
pub const SURFACE_HI: D2D1_COLOR_F = rgb(0x2A2A35, 1.0);
pub const BORDER: D2D1_COLOR_F = rgb(0x282831, 1.0);
pub const BORDER_HI: D2D1_COLOR_F = rgb(0x44444F, 1.0);
pub const TRACK: D2D1_COLOR_F = rgb(0x2E2E38, 1.0);
pub const TEXT: D2D1_COLOR_F = rgb(0xF4F4F5, 1.0);
pub const SILVER: D2D1_COLOR_F = rgb(0xC9CDD6, 1.0);
pub const MUTED: D2D1_COLOR_F = rgb(0x9696A3, 1.0);
pub const DIM: D2D1_COLOR_F = rgb(0x5E5E6A, 1.0);
pub const ACCENT: D2D1_COLOR_F = rgb(0x6E56F8, 1.0);
pub const ACCENT_DEEP: D2D1_COLOR_F = rgb(0x3D2BB8, 1.0);
pub const ACCENT_HI: D2D1_COLOR_F = rgb(0xA796FF, 1.0);
pub const SCREEN_DARK: D2D1_COLOR_F = rgb(0x121022, 1.0);
pub const LUME: D2D1_COLOR_F = rgb(0xE9E4FF, 1.0);
pub const GREEN: D2D1_COLOR_F = rgb(0x2FD27A, 1.0);
pub const AMBER: D2D1_COLOR_F = rgb(0xF5A524, 1.0);
pub const RED: D2D1_COLOR_F = rgb(0xFF5468, 1.0);
pub const CYAN: D2D1_COLOR_F = rgb(0x38BDF8, 1.0);

pub const fn alpha(c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

pub fn lerp(a: D2D1_COLOR_F, b: D2D1_COLOR_F, t: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn v(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl R {
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> R {
        R { x0, y0, x1, y1 }
    }

    pub fn d2d(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x0,
            top: self.y0,
            right: self.x1,
            bottom: self.y1,
        }
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn w(self) -> f32 {
        self.x1 - self.x0
    }

    pub fn h(self) -> f32 {
        self.y1 - self.y0
    }

    pub fn mid_y(self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }

    pub fn inset(self, by: f32) -> R {
        R::new(self.x0 + by, self.y0 + by, self.x1 - by, self.y1 - by)
    }
}

impl From<Rect> for R {
    fn from((x0, y0, x1, y1): Rect) -> R {
        R { x0, y0, x1, y1 }
    }
}

pub struct Fonts {
    pub title: IDWriteTextFormat,
    pub sub: IDWriteTextFormat,
    pub label: IDWriteTextFormat,
    pub headline: IDWriteTextFormat,
    pub body: IDWriteTextFormat,
    pub body_c: IDWriteTextFormat,
    pub small: IDWriteTextFormat,
    pub small_r: IDWriteTextFormat,
    pub small_c: IDWriteTextFormat,
    pub note: IDWriteTextFormat,
    pub message: IDWriteTextFormat,
    pub chip: IDWriteTextFormat,
    pub button: IDWriteTextFormat,
    pub badge: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
}

struct FontSpec {
    family: PCWSTR,
    weight: DWRITE_FONT_WEIGHT,
    size: f32,
    align: DWRITE_TEXT_ALIGNMENT,
    vertical: DWRITE_PARAGRAPH_ALIGNMENT,
    wrap: DWRITE_WORD_WRAPPING,
}

fn text_format(dw: &IDWriteFactory, locale: PCWSTR, spec: FontSpec) -> Result<IDWriteTextFormat> {
    // SAFETY: the strings are NUL-terminated and last for the whole program.
    unsafe {
        let format = dw.CreateTextFormat(
            spec.family,
            None,
            spec.weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            spec.size,
            locale,
        )?;
        format.SetTextAlignment(spec.align)?;
        format.SetParagraphAlignment(spec.vertical)?;
        format.SetWordWrapping(spec.wrap)?;
        Ok(format)
    }
}

impl Fonts {
    pub fn new(dw: &IDWriteFactory, spanish: bool) -> Result<Fonts> {
        let locale = if spanish { w!("es-es") } else { w!("en-us") };
        let (bahn, segoe) = (w!("Bahnschrift"), w!("Segoe UI"));
        let (norm, semi) = (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD);
        let (lead, trail, center) = (
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_TEXT_ALIGNMENT_TRAILING,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        );
        let middle = DWRITE_PARAGRAPH_ALIGNMENT_CENTER;
        let one_line = DWRITE_WORD_WRAPPING_NO_WRAP;
        let line = |family, weight, size, align| FontSpec {
            family,
            weight,
            size,
            align,
            vertical: middle,
            wrap: one_line,
        };
        let make = |spec| text_format(dw, locale, spec);
        let message = make(FontSpec {
            family: segoe,
            weight: norm,
            size: 11.5,
            align: lead,
            vertical: middle,
            wrap: DWRITE_WORD_WRAPPING_WRAP,
        })?;
        // A message from a file or a device can be long, and it mustn't run out of its box.
        let trim = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        // SAFETY: both arguments are valid for the call, and the format keeps its own reference.
        unsafe {
            let sign = dw.CreateEllipsisTrimmingSign(&message)?;
            message.SetTrimming(&trim, &sign)?;
        }
        Ok(Fonts {
            title: make(line(bahn, semi, 22.0, lead))?,
            sub: make(line(bahn, norm, 11.0, lead))?,
            label: make(line(bahn, semi, 11.5, lead))?,
            headline: make(line(bahn, semi, 19.0, center))?,
            body: make(line(segoe, norm, 13.0, lead))?,
            body_c: make(line(segoe, norm, 13.0, center))?,
            small: make(line(segoe, norm, 11.0, lead))?,
            small_r: make(line(segoe, norm, 11.0, trail))?,
            small_c: make(line(segoe, norm, 11.0, center))?,
            note: make(FontSpec {
                family: segoe,
                weight: norm,
                size: 11.0,
                align: lead,
                vertical: DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                wrap: DWRITE_WORD_WRAPPING_WRAP,
            })?,
            message,
            chip: make(line(segoe, semi, 12.0, center))?,
            button: make(line(bahn, semi, 12.5, center))?,
            badge: make(line(bahn, semi, 10.5, center))?,
            icon: make(line(w!("Segoe MDL2 Assets"), norm, 15.0, center))?,
        })
    }
}

/// What's drawn with, per render target, which the system can take away at any time.
pub struct Gfx {
    pub rt: ID2D1RenderTarget,
    /// The same target, when it's a window's, for resizing it.
    pub window: Option<ID2D1HwndRenderTarget>,
    pub brush: ID2D1SolidColorBrush,
    pub glow: ID2D1RadialGradientBrush,
    pub rainbow: ID2D1GradientStopCollection,
    pub size: (u32, u32),
    /// The preview, its size, and which frame of which animation it holds.
    pub preview: Option<PreviewBitmap>,
}

pub struct PreviewBitmap {
    pub bitmap: ID2D1Bitmap,
    pub size: (u32, u32),
    /// Which frame of which animation it holds now.
    pub holds: Option<(Arc<Animation>, usize)>,
}

impl Gfx {
    /// For drawing on a window.
    pub fn for_window(
        d2d: &ID2D1Factory,
        target: D2D1_HWND_RENDER_TARGET_PROPERTIES,
    ) -> Result<Gfx> {
        // SAFETY: the properties are valid for the call, and what comes back owns its references.
        let window = unsafe {
            d2d.CreateHwndRenderTarget(
                // Plenty for a window like this, and it doesn't load the GPU's driver, which takes
                // far more memory than the rest of the app.
                &D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                    ..Default::default()
                },
                &target,
            )?
        };
        let size = (target.pixelSize.width, target.pixelSize.height);
        Gfx::from_target(window.cast()?, Some(window), size)
    }

    /// For drawing on a part of a device context, for a picture rather than the desktop.
    pub fn for_dc(d2d: &ID2D1Factory, dc: HDC, part: &RECT) -> Result<Gfx> {
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            ..Default::default()
        };
        // SAFETY: the properties and rectangle are valid for the calls, and the caller keeps the
        // device context for as long as this draws.
        unsafe {
            let target = d2d.CreateDCRenderTarget(&properties)?;
            target.BindDC(dc, part)?;
            let size = (
                (part.right - part.left) as u32,
                (part.bottom - part.top) as u32,
            );
            Gfx::from_target(target.cast()?, None, size)
        }
    }

    fn from_target(
        rt: ID2D1RenderTarget,
        window: Option<ID2D1HwndRenderTarget>,
        size: (u32, u32),
    ) -> Result<Gfx> {
        // SAFETY: the properties are valid for the calls, and what comes back owns its references.
        unsafe {
            let brush = rt.CreateSolidColorBrush(&TEXT, None)?;
            let glow_stops = rt.CreateGradientStopCollection(
                &[
                    D2D1_GRADIENT_STOP {
                        position: 0.0,
                        color: alpha(ACCENT, 0.24),
                    },
                    D2D1_GRADIENT_STOP {
                        position: 1.0,
                        color: alpha(ACCENT, 0.0),
                    },
                ],
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let glow = rt.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: v(60.0, 10.0),
                    gradientOriginOffset: v(0.0, 0.0),
                    radiusX: 460.0,
                    radiusY: 260.0,
                },
                None,
                &glow_stops,
            )?;
            let hues = [
                0xFF4D4D, 0xFFB03B, 0xF6F05C, 0x45E06F, 0x3BC8FF, 0x7A6BFF, 0xFF5CC8,
            ];
            let stops: Vec<D2D1_GRADIENT_STOP> = hues
                .iter()
                .enumerate()
                .map(|(i, hex)| D2D1_GRADIENT_STOP {
                    position: i as f32 / (hues.len() - 1) as f32,
                    color: rgb(*hex, 1.0),
                })
                .collect();
            let rainbow =
                rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            Ok(Gfx {
                rt,
                window,
                brush,
                glow,
                rainbow,
                size,
                preview: None,
            })
        }
    }

    /// The preview's bitmap, made if there's none of this size yet.
    pub fn preview_bitmap(&mut self, size: (u32, u32)) -> Result<&mut PreviewBitmap> {
        if self.preview.as_ref().is_none_or(|p| p.size != size) {
            let properties = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let pixels = D2D_SIZE_U {
                width: size.0,
                height: size.1,
            };
            // SAFETY: the properties are valid for the call, with no starting pixels to read.
            let bitmap = unsafe { self.rt.CreateBitmap(pixels, None, 0, &properties)? };
            self.preview = Some(PreviewBitmap {
                bitmap,
                size,
                holds: None,
            });
        }
        // Just made, if it wasn't there.
        Ok(self.preview.as_mut().expect("the preview was just made"))
    }
}

/// How a chip or button looks right now.
#[derive(Clone, Copy, Default)]
pub struct Look {
    pub selected: bool,
    pub enabled: bool,
    pub hovered: bool,
}

/// Drawing shortcuts on the render target, with a single solid brush that gets reused.
pub struct Draw<'a> {
    pub rt: &'a ID2D1RenderTarget,
    pub brush: &'a ID2D1SolidColorBrush,
    pub dw: &'a IDWriteFactory,
}

impl Draw<'_> {
    fn color(&self, c: D2D1_COLOR_F) -> &ID2D1SolidColorBrush {
        // SAFETY: the colour is valid for the call.
        unsafe { self.brush.SetColor(&c) };
        self.brush
    }

    pub fn round(&self, r: R, radius: f32, c: D2D1_COLOR_F) {
        let rr = D2D1_ROUNDED_RECT {
            rect: r.d2d(),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: inside the frame's drawing, with arguments valid for the call. As for all below.
        unsafe { self.rt.FillRoundedRectangle(&rr, self.color(c)) }
    }

    pub fn round_stroke(&self, r: R, radius: f32, c: D2D1_COLOR_F, width: f32) {
        let rr = D2D1_ROUNDED_RECT {
            rect: r.inset(0.5).d2d(),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: as in `round`.
        unsafe {
            self.rt
                .DrawRoundedRectangle(&rr, self.color(c), width, None::<&ID2D1StrokeStyle>)
        }
    }

    /// A soft glow around a rounded rectangle, from layers of faint colour.
    pub fn glow_round(&self, r: R, radius: f32, c: D2D1_COLOR_F, strength: f32) {
        for i in (1..=6).rev() {
            let e = i as f32 * 2.0;
            self.round(
                R::new(r.x0 - e, r.y0 - e, r.x1 + e, r.y1 + e),
                radius + e,
                alpha(c, strength * 0.12 / i as f32),
            );
        }
    }

    pub fn circle(&self, x: f32, y: f32, radius: f32, c: D2D1_COLOR_F) {
        let e = D2D1_ELLIPSE {
            point: v(x, y),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: as in `round`.
        unsafe { self.rt.FillEllipse(&e, self.color(c)) }
    }

    pub fn circle_stroke(&self, x: f32, y: f32, radius: f32, c: D2D1_COLOR_F, width: f32) {
        let e = D2D1_ELLIPSE {
            point: v(x, y),
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: as in `round`.
        unsafe {
            self.rt
                .DrawEllipse(&e, self.color(c), width, None::<&ID2D1StrokeStyle>)
        }
    }

    pub fn glow_circle(&self, x: f32, y: f32, radius: f32, c: D2D1_COLOR_F) {
        for i in 1..=6 {
            self.circle(x, y, radius * i as f32 / 6.0, alpha(c, 0.05));
        }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: D2D1_COLOR_F, width: f32) {
        // SAFETY: as in `round`.
        unsafe {
            self.rt.DrawLine(
                v(x0, y0),
                v(x1, y1),
                self.color(c),
                width,
                None::<&ID2D1StrokeStyle>,
            )
        }
    }

    pub fn text(&self, s: &str, f: &IDWriteTextFormat, r: R, c: D2D1_COLOR_F) {
        #[cfg(debug_assertions)]
        self.check_fit(s, f, r);
        let text = wide(s);
        // SAFETY: as in `round`.
        unsafe {
            self.rt.DrawText(
                &text,
                f,
                &r.d2d(),
                self.color(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        }
    }

    /// How wide a line of text is.
    pub fn text_width(&self, s: &str, f: &IDWriteTextFormat) -> f32 {
        text_width(self.dw, s, f)
    }

    /// In a debug build, says when a text doesn't fit where it's drawn, once per text and place.
    /// That's how clipped text gets found across all the states and languages.
    #[cfg(debug_assertions)]
    fn check_fit(&self, s: &str, f: &IDWriteTextFormat, r: R) {
        use std::cell::RefCell;
        use std::collections::HashSet;
        thread_local! {
            static SAID: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
        }
        let text = wide(s);
        // SAFETY: valid arguments, and the metrics are written by the call.
        let metrics = unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&text, f, r.w(), r.h()) else {
                return;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return;
            }
            metrics
        };
        let wide_by = metrics.width - r.w();
        let tall_by = metrics.height - r.h();
        if wide_by > 0.5 || tall_by > 0.5 {
            let line = format!("clipped by {wide_by:.1} wide, {tall_by:.1} tall: {s}");
            let new = SAID.with(|said| said.borrow_mut().insert(line.clone()));
            if new {
                eprintln!("{line}");
            }
        }
    }

    /// Just the preview's picture, on what's already there. A full frame costs far more than this,
    /// and an animation would pay for it many times a second.
    pub fn just_bitmap(&self, bitmap: &ID2D1Bitmap, to: R, sharp: bool) -> Result<()> {
        // SAFETY: the target and bitmap are valid, and these calls pair up.
        unsafe {
            self.rt.BeginDraw();
            self.bitmap(bitmap, to, sharp);
            self.rt.EndDraw(None, None)
        }
    }

    /// A chip: a choice among several, or a small button.
    pub fn chip(&self, r: R, label: &str, font: &IDWriteTextFormat, look: Look) {
        let radius = (r.h() / 2.0).min(9.0);
        if look.selected && look.enabled {
            self.round(r, radius, ACCENT);
        } else if look.selected {
            self.round(r, radius, TRACK);
        } else if look.enabled && look.hovered {
            self.round(r, radius, SURFACE_HI);
            self.round_stroke(r, radius, BORDER_HI, 1.0);
        } else {
            self.round(r, radius, SURFACE);
            self.round_stroke(r, radius, BORDER, 1.0);
        }
        let color = if !look.enabled {
            DIM
        } else if look.selected {
            TEXT
        } else {
            SILVER
        };
        self.text(label, font, r, color);
    }

    /// A button: a filled one for what the section is for, or an outlined one.
    pub fn button(&self, r: R, label: &str, font: &IDWriteTextFormat, primary: bool, look: Look) {
        let radius = 9.0;
        let color = if !look.enabled {
            self.round(r, radius, SURFACE);
            self.round_stroke(r, radius, BORDER, 1.0);
            DIM
        } else if primary {
            self.glow_round(r, radius, ACCENT, 0.3);
            self.round(r, radius, if look.hovered { ACCENT_HI } else { ACCENT });
            if look.hovered { BG } else { TEXT }
        } else {
            self.round(r, radius, if look.hovered { SURFACE_HI } else { SURFACE });
            let border = if look.hovered { BORDER_HI } else { BORDER };
            self.round_stroke(r, radius, border, 1.0);
            TEXT
        };
        self.text(label, font, r, color);
    }

    /// An animated toggle, whose `t` goes from 0 (off) to 1 (on).
    pub fn toggle(&self, x: f32, y: f32, t: f32, enabled: bool) {
        let track = R::new(x, y, x + 44.0, y + 24.0);
        let on = if enabled {
            lerp(TRACK, ACCENT, t)
        } else {
            SURFACE
        };
        if enabled && t > 0.05 {
            self.glow_round(track, 12.0, ACCENT, 0.5 * t);
        }
        self.round(track, 12.0, on);
        self.circle(
            x + 12.0 + t * 20.0,
            y + 12.0,
            9.0,
            if enabled { TEXT } else { DIM },
        );
    }

    /// A slider with a mark at each of its steps. `step` is where it is now.
    pub fn slider(&self, x0: f32, x1: f32, y: f32, step: usize, steps: usize, enabled: bool) {
        let at = crate::layout::step_x(step, x0, x1, steps);
        self.round(R::new(x0, y - 3.0, x1, y + 3.0), 3.0, TRACK);
        if at > x0 {
            let fill = if enabled { ACCENT_HI } else { DIM };
            self.round(R::new(x0, y - 3.0, at, y + 3.0), 3.0, fill);
        }
        for mark in 0..steps {
            let x = crate::layout::step_x(mark, x0, x1, steps);
            self.circle(x, y, 1.5, alpha(BG, 0.7));
        }
        if enabled {
            self.glow_circle(at, y, 14.0, ACCENT);
        }
        self.circle(at, y, 8.0, if enabled { TEXT } else { DIM });
        self.circle(at, y, 3.0, if enabled { ACCENT } else { TRACK });
    }

    /// The preview's picture. At 100 % every pixel lands on one of the screen's, and stays sharp.
    pub fn bitmap(&self, bitmap: &ID2D1Bitmap, to: R, sharp: bool) {
        let mode = if sharp {
            D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR
        } else {
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR
        };
        // SAFETY: as in `round`.
        unsafe {
            self.rt.DrawBitmap(bitmap, Some(&to.d2d()), 1.0, mode, None);
        }
    }

    /// A circle filled with all the colours, for the rainbow option.
    pub fn rainbow_circle(
        &self,
        stops: &ID2D1GradientStopCollection,
        x: f32,
        y: f32,
        radius: f32,
    ) -> Result<()> {
        let e = D2D1_ELLIPSE {
            point: v(x, y),
            radiusX: radius,
            radiusY: radius,
        };
        let properties = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: v(x - radius, y - radius),
            endPoint: v(x + radius, y + radius),
        };
        // SAFETY: as in `round`, and the brush lasts through the call that uses it.
        unsafe {
            let brush = self
                .rt
                .CreateLinearGradientBrush(&properties, None, stops)?;
            self.rt.FillEllipse(&e, &brush);
        }
        Ok(())
    }

    /// The app's mark: the same key with a screen as its icon.
    pub fn logo(&self, r: R) {
        let at = |x0: f32, y0: f32, x1: f32, y1: f32| {
            R::new(
                r.x0 + x0 * r.w(),
                r.y0 + y0 * r.h(),
                r.x0 + x1 * r.w(),
                r.y0 + y1 * r.h(),
            )
        };
        let scale = r.w();
        self.glow_round(at(0.04, 0.06, 0.96, 0.96), 0.2 * scale, ACCENT, 0.35);
        self.round(at(0.04, 0.06, 0.96, 0.96), 0.2 * scale, ACCENT_DEEP);
        self.round(at(0.13, 0.1, 0.87, 0.76), 0.13 * scale, ACCENT);
        self.round(at(0.24, 0.22, 0.76, 0.6), 0.05 * scale, SCREEN_DARK);
        self.round(at(0.32, 0.36, 0.68, 0.47), 0.02 * scale, LUME);
    }
}

/// How wide a line of text is.
pub fn text_width(dw: &IDWriteFactory, s: &str, f: &IDWriteTextFormat) -> f32 {
    let text = wide(s);
    // SAFETY: valid arguments, and the metrics are written by the call.
    unsafe {
        let Ok(layout) = dw.CreateTextLayout(&text, f, 4000.0, 100.0) else {
            return 0.0;
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if layout.GetMetrics(&mut metrics).is_err() {
            return 0.0;
        }
        metrics.widthIncludingTrailingWhitespace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_knows_what_it_holds() {
        let r = R::new(10.0, 20.0, 30.0, 60.0);
        assert!(r.contains(10.0, 20.0));
        assert!(r.contains(30.0, 60.0));
        assert!(!r.contains(9.0, 20.0));
        assert!(!r.contains(10.0, 61.0));
        assert_eq!((r.w(), r.h(), r.mid_y()), (20.0, 40.0, 40.0));
        assert_eq!(r.inset(1.0), R::new(11.0, 21.0, 29.0, 59.0));
    }

    #[test]
    fn colours_mix_from_one_to_the_other() {
        let mid = lerp(alpha(BG, 0.0), alpha(BG, 1.0), 0.5);
        assert_eq!(mid.a, 0.5);
        assert_eq!(lerp(ACCENT, GREEN, 0.0).r, ACCENT.r);
        assert_eq!(lerp(ACCENT, GREEN, 1.0).g, GREEN.g);
    }

    #[test]
    fn the_logo_uses_the_icons_colours() {
        assert_eq!(rgb(0x6E56F8, 1.0).g, 86.0 / 255.0);
        assert_eq!(ACCENT.b, 248.0 / 255.0);
    }
}
