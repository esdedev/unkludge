//! The parts around the sections: the header, the banners, the foot of the window, and what's shown
//! when there's no keyboard.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

use super::gfx::{
    ACCENT, ACCENT_HI, AMBER, BORDER, CARD, CYAN, Draw, GREEN, KEY_ART, LUME, Look, MUTED, R, RED,
    SCREEN_DARK, SILVER, SURFACE, TEXT, alpha,
};
use super::{App, Hit, Item, Role, TextItem};
use crate::i18n::t;
use crate::layout::WIDTH;
use crate::state::Tone;

pub fn tone_color(tone: Tone) -> D2D1_COLOR_F {
    match tone {
        Tone::Neutral => MUTED,
        Tone::Good => GREEN,
        Tone::Warning => AMBER,
        Tone::Bad => RED,
    }
}

/// The longest start of a text that fits, with an ellipsis if some of it had to go.
pub fn fitted_text(
    d: &Draw,
    text: &str,
    font: &windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    width: f32,
) -> String {
    if d.text_width(text, font) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let middle = (low + high).div_ceil(2);
        let candidate: String = chars[..middle].iter().chain(['…'].iter()).collect();
        if d.text_width(&candidate, font) <= width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    chars[..low].iter().chain(['…'].iter()).collect()
}

impl App {
    /// Adds an item to the list of what can be used, as of this paint.
    pub fn item(
        &self,
        rect: R,
        hit: Hit,
        name: impl Into<String>,
        role: Role,
        on: bool,
        enabled: bool,
    ) {
        self.shared.borrow_mut().items.push(Item {
            rect,
            hit,
            name: name.into(),
            value: None,
            role,
            on,
            enabled,
        });
    }

    /// Adds a slider to the list, which reads as its value.
    pub fn slider_item(&self, rect: R, hit: Hit, name: &str, value: String, enabled: bool) {
        self.shared.borrow_mut().items.push(Item {
            rect,
            hit,
            name: name.to_string(),
            value: Some(value),
            role: Role::Slider,
            on: false,
            enabled,
        });
    }

    /// Adds text that says how things stand, for a screen reader to find.
    pub fn text_item(&self, rect: R, which: TextItem, name: &str, value: Option<String>) {
        self.shared.borrow_mut().items.push(Item {
            rect,
            hit: Hit::Text(which),
            name: name.to_string(),
            value,
            role: Role::Text,
            on: false,
            enabled: true,
        });
    }

    /// How a chip or button looks, given where the mouse is.
    pub fn look(&self, hit: Hit, selected: bool, enabled: bool) -> Look {
        Look {
            selected,
            enabled,
            hovered: self.hover == Some(hit),
        }
    }

    pub fn card(&self, d: &Draw, r: R, title: &str, icon: &str) {
        d.round(r, 14.0, CARD);
        d.round_stroke(r, 14.0, BORDER, 1.0);
        d.round(
            R::new(r.x0 + 18.0, r.y0 + 17.0, r.x0 + 21.0, r.y0 + 31.0),
            1.5,
            ACCENT,
        );
        d.text(
            icon,
            &self.fonts.icon,
            R::new(r.x0 + 26.0, r.y0 + 14.0, r.x0 + 46.0, r.y0 + 34.0),
            MUTED,
        );
        d.text(
            title,
            &self.fonts.label,
            R::new(r.x0 + 50.0, r.y0 + 14.0, r.x1 - 16.0, r.y0 + 34.0),
            TEXT,
        );
    }

    pub fn paint_header(&self, d: &Draw) {
        d.logo(R::new(20.0, 18.0, 62.0, 60.0));
        let model = self.state.model();
        let title = model.map_or_else(|| "UNKLUDGE".to_string(), |m| m.name.to_uppercase());
        let title_width = d.text_width(&title, &self.fonts.title);
        d.text(
            &title,
            &self.fonts.title,
            R::new(78.0, 16.0, 78.0 + title_width + 2.0, 44.0),
            TEXT,
        );
        let sub = match model {
            Some(m) => format!("Unkludge  ·  PID {:04X}", m.pid),
            None => t().tagline.to_string(),
        };
        d.text(
            &sub,
            &self.fonts.sub,
            R::new(79.0, 42.0, 620.0, 60.0),
            MUTED,
        );

        let mut x = 78.0 + title_width + 14.0;
        if self.state.is_experimental() {
            let badge_w = d.text_width(t().experimental, &self.fonts.badge) + 20.0;
            let badge = R::new(x, 21.0, x + badge_w, 41.0);
            d.round(badge, 10.0, alpha(AMBER, 0.12));
            d.round_stroke(badge, 10.0, alpha(AMBER, 0.5), 1.0);
            d.text(t().experimental, &self.fonts.badge, badge, AMBER);
            x = badge.x1 + 10.0;
        }
        if self.state.offers_report() {
            let link_w = d.text_width(t().send_report, &self.fonts.small) + 2.0;
            let link = R::new(x, 21.0, x + link_w, 41.0);
            let hovered = self.hover == Some(Hit::Report);
            let color = if hovered { TEXT } else { ACCENT_HI };
            d.text(t().send_report, &self.fonts.small, link, color);
            d.line(link.x0, 37.0, link.x1, 37.0, alpha(color, 0.5), 1.0);
            self.item(link, Hit::Report, t().send_report, Role::Link, false, true);
        }

        let status = self.state.status();
        let color = tone_color(status.tone);
        let pill_w = d.text_width(&status.text, &self.fonts.badge) + 46.0;
        let pill = R::new(WIDTH - 20.0 - pill_w, 25.0, WIDTH - 20.0, 53.0);
        d.round(pill, 14.0, alpha(color, 0.10));
        d.round_stroke(pill, 14.0, alpha(color, 0.35), 1.0);
        d.circle(pill.x0 + 18.0, pill.mid_y(), 4.0, color);
        d.glow_circle(pill.x0 + 18.0, pill.mid_y(), 9.0, color);
        d.text(
            &status.text,
            &self.fonts.badge,
            R::new(pill.x0 + 30.0, pill.y0, pill.x1 - 12.0, pill.y1),
            TEXT,
        );
        self.text_item(pill, TextItem::Status, &status.text, None);

        if self.is_demo() {
            let width = d.text_width(t().demo, &self.fonts.badge) + 24.0;
            let chip = R::new(pill.x0 - 10.0 - width, 28.0, pill.x0 - 10.0, 50.0);
            d.round(chip, 11.0, alpha(CYAN, 0.15));
            d.round_stroke(chip, 11.0, alpha(CYAN, 0.7), 1.0);
            d.text(t().demo, &self.fonts.badge, chip, CYAN);
        }

        // A line under the header, fading out.
        for i in 0..40 {
            let share = i as f32 / 40.0;
            let x0 = 20.0 + share * (WIDTH - 40.0);
            let x1 = 20.0 + (share + 1.0 / 40.0) * (WIDTH - 40.0);
            d.line(x0, 68.0, x1, 68.0, alpha(ACCENT, 0.6 * (1.0 - share)), 1.0);
        }
    }

    pub fn paint_banners(&self, d: &Draw) {
        let banners = self.state.banners();
        for (index, (banner, rect)) in banners.iter().zip(&self.frame.banners).enumerate() {
            let r = R::from(*rect);
            self.text_item(r, TextItem::Banner(index), &banner.text, None);
            let color = tone_color(banner.tone);
            d.round(r, 10.0, alpha(color, 0.10));
            d.round_stroke(r, 10.0, alpha(color, 0.45), 1.0);
            d.text(
                "\u{E7BA}",
                &self.fonts.icon,
                R::new(r.x0 + 10.0, r.y0, r.x0 + 38.0, r.y1),
                color,
            );
            d.text(
                &banner.text,
                &self.fonts.message,
                R::new(r.x0 + 42.0, r.y0 + 2.0, r.x1 - 14.0, r.y1 - 2.0),
                TEXT,
            );
        }
    }

    pub fn paint_footer(&self, d: &Draw) {
        let f = R::from(self.frame.footer);
        let mid = f.mid_y();
        d.round(f, 12.0, CARD);
        d.round_stroke(f, 12.0, BORDER, 1.0);

        let row = R::new(f.x1 - 234.0, f.y0, f.x1 - 10.0, f.y1);
        d.line(row.x0, f.y0 + 10.0, row.x0, f.y1 - 10.0, BORDER, 1.0);
        d.text(
            t().start_with_system,
            &self.fonts.small_r,
            R::new(row.x0 + 6.0, f.y0, row.x1 - 54.0, f.y1),
            SILVER,
        );
        d.toggle(row.x1 - 44.0, mid - 12.0, self.a_autostart.value, true);
        self.item(
            row,
            Hit::Autostart,
            t().start_with_system,
            Role::Toggle,
            self.autostart,
            true,
        );

        let text_area = R::new(f.x0 + 32.0, f.y0 + 2.0, row.x0 - 12.0, f.y1 - 2.0);
        match &self.state.message {
            Some(message) => {
                d.circle(f.x0 + 18.0, mid, 3.5, tone_color(message.tone));
                d.text(&message.text, &self.fonts.message, text_area, SILVER);
                self.text_item(text_area, TextItem::Footer, &message.text, None);
            }
            None if self.is_demo() => {
                d.circle(f.x0 + 18.0, mid, 3.5, CYAN);
                d.text(t().demo_name, &self.fonts.message, text_area, MUTED);
                self.text_item(text_area, TextItem::Footer, t().demo_name, None);
            }
            None => {}
        }
    }

    /// With no keyboard there's nothing to set, so this takes the sections' place.
    pub fn paint_empty(&self, d: &Draw) {
        let c = R::from(self.frame.empty);
        d.round(c, 14.0, CARD);
        d.round_stroke(c, 14.0, BORDER, 1.0);

        let mid_x = (c.x0 + c.x1) / 2.0;
        let art_w = 500.0;
        let art_h = 176.0;
        let art_top = c.y0 + (c.h() - art_h - 104.0) / 2.0;
        keyboard_art(
            d,
            R::new(
                mid_x - art_w / 2.0,
                art_top,
                mid_x + art_w / 2.0,
                art_top + art_h,
            ),
        );

        let text_top = art_top + art_h + 28.0;
        let view = self.state.empty_view();
        d.text(
            &view.headline,
            &self.fonts.headline,
            R::new(c.x0 + 20.0, text_top, c.x1 - 20.0, text_top + 30.0),
            TEXT,
        );
        let color = match view.tone {
            Tone::Neutral | Tone::Good => MUTED,
            tone => tone_color(tone),
        };
        self.text_item(
            c,
            TextItem::Empty,
            &view.headline,
            Some(view.detail.clone()),
        );
        let detail = fitted_text(d, &view.detail, &self.fonts.body_c, c.w() - 60.0);
        d.text(
            &detail,
            &self.fonts.body_c,
            R::new(c.x0 + 20.0, text_top + 32.0, c.x1 - 20.0, text_top + 56.0),
            color,
        );
    }
}

/// A keyboard with a little screen, waiting.
fn keyboard_art(d: &Draw, r: R) {
    d.round(r, 16.0, SURFACE);
    d.round_stroke(r, 16.0, BORDER, 1.0);
    let pad = 14.0;
    let gap = 5.0;
    let columns = 15;
    let rows = 5;
    let key = (r.w() - 2.0 * pad - gap * (columns as f32 - 1.0)) / columns as f32;
    let row_h = (r.h() - 2.0 * pad - gap * (rows as f32 - 1.0)) / rows as f32;
    for row in 0..rows {
        for column in 0..columns {
            // The screen takes the keyboard's top right corner.
            if row < 2 && column >= columns - 3 {
                continue;
            }
            let x = r.x0 + pad + column as f32 * (key + gap);
            let y = r.y0 + pad + row as f32 * (row_h + gap);
            d.round(R::new(x, y, x + key, y + row_h), 4.0, KEY_ART);
        }
    }
    let x = r.x0 + pad + (columns - 3) as f32 * (key + gap);
    let screen = R::new(x, r.y0 + pad, r.x1 - pad, r.y0 + pad + 2.0 * row_h + gap);
    d.round(screen, 5.0, SCREEN_DARK);
    d.round_stroke(screen, 5.0, alpha(ACCENT, 0.7), 1.0);
    let line_y = screen.mid_y();
    d.round(
        R::new(
            screen.x0 + 16.0,
            line_y - 3.0,
            screen.x1 - 16.0,
            line_y + 3.0,
        ),
        2.0,
        alpha(LUME, 0.7),
    );
}
