//! The lights section and the clock's card.

use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::{ColorSetting, LightMode, ModeChoice, Rgb, SleepSetting};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::ID2D1GradientStopCollection;

use super::gfx::{BORDER, DIM, Draw, MUTED, R, SILVER, TEXT, TRACK_DISABLED, alpha};
use super::{App, Hit, Role};
use crate::i18n::t;
use crate::layout::{self, effect_chips, split_row};
use crate::state::{LightControls, brightness_text};

fn color_of(rgb: Rgb) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(rgb.r) / 255.0,
        g: f32::from(rgb.g) / 255.0,
        b: f32::from(rgb.b) / 255.0,
        a: 1.0,
    }
}

impl App {
    pub fn paint_lights(&self, d: &Draw, rainbow: &ID2D1GradientStopCollection) {
        let c = R::from(self.frame.lights);
        self.card(d, c, t().lights, "\u{E706}");
        let rows = layout::light_rows(self.frame.lights);
        let controls = self.state.light_controls();
        self.paint_effects(d, rows.effects.into(), &controls);
        let colors = R::from(rows.colors);
        let sliders = R::from(rows.sliders);
        if controls.effect.is_some() {
            self.paint_sliders(d, sliders, &controls);
            self.paint_colors(d, colors, &controls, rainbow);
        } else {
            d.text(
                self.state.light_note(),
                &self.fonts.note,
                R::new(sliders.x0, sliders.y0 + 8.0, sliders.x1, colors.y1),
                MUTED,
            );
        }
        self.paint_sleep(d, rows.sleep.into(), &controls);
    }

    fn paint_effects(&self, d: &Draw, area: R, controls: &LightControls) {
        let Some(model) = self.state.model() else {
            return;
        };
        let effects = model.lighting.effects;
        let chips = effect_chips((area.x0, area.y0, area.x1, area.y1), effects.len() + 1);
        let enabled = controls.enabled;
        let off = Hit::Mode(ModeChoice::Off);
        let off_on = controls.mode == Some(LightMode::Off);
        d.chip(
            chips[0].into(),
            t().off,
            &self.fonts.chip,
            self.look(off, off_on, enabled),
        );
        self.item(
            chips[0].into(),
            off,
            t().labelled(t().effect, t().off),
            Role::Choice,
            off_on,
            enabled,
        );
        for (effect, chip) in effects.iter().zip(&chips[1..]) {
            let hit = Hit::Mode(ModeChoice::Effect(effect.id));
            let on = controls.mode == Some(LightMode::Effect(effect.id));
            let name = t().effect_name(effect);
            d.chip(
                (*chip).into(),
                name,
                &self.fonts.chip,
                self.look(hit, on, enabled),
            );
            self.item(
                (*chip).into(),
                hit,
                t().labelled(t().effect, name),
                Role::Choice,
                on,
                enabled,
            );
        }
    }

    fn paint_sliders(&self, d: &Draw, area: R, controls: &LightControls) {
        d.line(area.x0, area.y0 - 5.0, area.x1, area.y0 - 5.0, BORDER, 1.0);
        let parts = split_row((area.x0, area.y0, area.x1, area.y1), 2, 24.0);
        if let Some((step, steps)) = controls.brightness {
            self.paint_slider(
                d,
                parts[0].into(),
                Hit::Brightness,
                t().brightness,
                brightness_text(step, steps),
                (step, steps),
                controls.enabled,
            );
        }
        if let Some((value, min, max)) = controls.speed {
            self.paint_slider(
                d,
                parts[1].into(),
                Hit::Speed,
                t().speed,
                value.to_string(),
                (usize::from(value - min), usize::from(max - min) + 1),
                controls.enabled,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_slider(
        &self,
        d: &Draw,
        r: R,
        hit: Hit,
        name: &str,
        value: String,
        (step, steps): (usize, usize),
        enabled: bool,
    ) {
        let (label, shown) = if enabled { (SILVER, TEXT) } else { (DIM, DIM) };
        d.text(
            name,
            &self.fonts.small,
            R::new(r.x0, r.y0, r.x1 - 50.0, r.y0 + 17.0),
            label,
        );
        d.text(
            &value,
            &self.fonts.small_r,
            R::new(r.x1 - 50.0, r.y0, r.x1, r.y0 + 17.0),
            shown,
        );
        let (x0, x1) = self.slider_track(hit);
        let y = r.y1 - 11.0;
        d.slider(x0, x1, y, step, steps, enabled);
        let rect = R::new(r.x0, y - 12.0, r.x1, y + 12.0);
        self.slider_item(rect, hit, name, value, enabled);
    }

    fn paint_colors(
        &self,
        d: &Draw,
        area: R,
        controls: &LightControls,
        rainbow: &ID2D1GradientStopCollection,
    ) {
        d.line(area.x0, area.y0 - 5.0, area.x1, area.y0 - 5.0, BORDER, 1.0);
        let enabled = controls.enabled;
        d.text(
            t().color,
            &self.fonts.small,
            R::new(area.x0, area.y0, area.x1, area.y0 + 17.0),
            if enabled { SILVER } else { DIM },
        );
        if !controls.colors && !controls.rainbow {
            d.text(
                t().own_colors,
                &self.fonts.small,
                R::new(area.x0, area.y0 + 18.0, area.x1, area.y1),
                MUTED,
            );
            return;
        }
        let y = area.y1 - 15.0;
        let radius = 12.0;
        let step = 32.0;
        let mut x = area.x0 + radius;
        let dim = |color: D2D1_COLOR_F| if enabled { color } else { alpha(color, 0.3) };
        if controls.colors {
            let colors = self.state.slot_colors(&self.settings);
            for (slot, rgb) in colors.iter().enumerate() {
                let slot = slot as u8;
                let hit = Hit::Slot(slot);
                let on = controls.color == Some(ColorSetting::Slot(slot));
                d.circle(x, y, radius, dim(color_of(*rgb)));
                self.swatch_ring(d, x, y, radius, hit, on, enabled);
                self.item(
                    swatch_rect(x, y, radius),
                    hit,
                    t().color_slot(slot),
                    Role::Choice,
                    on,
                    enabled,
                );
                x += step;
            }
        }
        if controls.rainbow {
            let on = controls.color == Some(ColorSetting::Rainbow);
            let drawn = d.rainbow_circle(rainbow, x, y, radius);
            if drawn.is_err() {
                d.circle(x, y, radius, TRACK_DISABLED);
            }
            if !enabled {
                d.circle(x, y, radius, alpha(TRACK_DISABLED, 0.8));
            }
            self.swatch_ring(d, x, y, radius, Hit::Rainbow, on, enabled);
            self.item(
                swatch_rect(x, y, radius),
                Hit::Rainbow,
                t().labelled(t().color, t().rainbow),
                Role::Choice,
                on,
                enabled,
            );
            x += step;
        }
        if controls.colors {
            // A custom colour goes into the first slot, so that's the one that shows it.
            let hovered = self.hover == Some(Hit::CustomColor);
            let edge = if !enabled {
                DIM
            } else if hovered {
                TEXT
            } else {
                MUTED
            };
            d.circle_stroke(x, y, radius - 0.5, edge, 1.0);
            d.text(
                "+",
                &self.fonts.chip,
                R::new(x - radius, y - radius - 1.0, x + radius, y + radius - 1.0),
                edge,
            );
            self.item(
                swatch_rect(x, y, radius),
                Hit::CustomColor,
                t().custom_color,
                Role::Button,
                false,
                enabled,
            );
            d.text(
                t().custom_color,
                &self.fonts.small,
                R::new(x + radius + 6.0, y - 10.0, area.x1, y + 10.0),
                if enabled { MUTED } else { DIM },
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn swatch_ring(
        &self,
        d: &Draw,
        x: f32,
        y: f32,
        radius: f32,
        hit: Hit,
        on: bool,
        enabled: bool,
    ) {
        if on {
            let color = if enabled { TEXT } else { SILVER };
            d.circle_stroke(x, y, radius + 2.5, color, 1.5);
        } else if enabled && self.hover == Some(hit) {
            d.circle_stroke(x, y, radius + 2.0, MUTED, 1.0);
        }
    }

    fn paint_sleep(&self, d: &Draw, area: R, controls: &LightControls) {
        d.line(area.x0, area.y0 - 5.0, area.x1, area.y0 - 5.0, BORDER, 1.0);
        let Some(sleep) = controls.sleep else {
            return;
        };
        let enabled = controls.enabled;
        d.text(
            t().sleep,
            &self.fonts.small,
            R::new(area.x0, area.y0, area.x0 + 120.0, area.y0 + 17.0),
            if enabled { SILVER } else { DIM },
        );
        if let SleepSetting::Unknown(raw) = sleep {
            d.text(
                &t().sleep_other(raw),
                &self.fonts.small_r,
                R::new(area.x0 + 120.0, area.y0, area.x1, area.y0 + 17.0),
                MUTED,
            );
        }
        let chips = (area.x0, area.y0 + 20.0, area.x1, area.y1);
        for (choice, part) in SleepTime::ALL
            .iter()
            .zip(split_row(chips, SleepTime::ALL.len(), 6.0))
        {
            let hit = Hit::Sleep(*choice);
            let on = sleep == SleepSetting::Known(*choice);
            let label = t().sleep_label(*choice);
            d.chip(
                part.into(),
                label,
                &self.fonts.chip,
                self.look(hit, on, enabled),
            );
            self.item(
                part.into(),
                hit,
                t().labelled(t().sleep, label),
                Role::Choice,
                on,
                enabled,
            );
        }
    }

    pub fn paint_clock(&self, d: &Draw) {
        let c = R::from(self.frame.clock);
        self.card(d, c, t().clock, "\u{E823}");
        let width = (d.text_width(t().sync_clock, &self.fonts.button) + 28.0).max(120.0);
        let button = R::new(c.x1 - 18.0 - width, c.y0 + 11.0, c.x1 - 18.0, c.y0 + 38.0);
        d.button(
            button,
            t().sync_clock,
            &self.fonts.button,
            false,
            self.look(Hit::SyncClock, false, true),
        );
        self.item(
            button,
            Hit::SyncClock,
            t().sync_clock,
            Role::Button,
            false,
            true,
        );

        let row = R::new(c.x0 + 18.0, c.y0 + 42.0, c.x1 - 18.0, c.y1 - 6.0);
        d.text(
            t().auto_clock,
            &self.fonts.body,
            R::new(row.x0, row.y0, row.x1 - 56.0, row.y0 + 19.0),
            TEXT,
        );
        d.text(
            t().auto_clock_more,
            &self.fonts.small,
            R::new(row.x0, row.y0 + 18.0, row.x1 - 56.0, row.y1),
            MUTED,
        );
        d.toggle(row.x1 - 44.0, row.y0 + 6.0, self.a_auto_clock.value, true);
        self.item(
            row,
            Hit::AutoClock,
            t().auto_clock_name,
            Role::Toggle,
            self.settings.auto_clock,
            true,
        );
    }
}

fn swatch_rect(x: f32, y: f32, radius: f32) -> R {
    R::new(x - radius, y - radius, x + radius, y + radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swatch_can_be_clicked_where_it_shows() {
        let r = swatch_rect(50.0, 40.0, 12.0);
        assert!(r.contains(50.0, 40.0));
        assert!(r.contains(38.0, 28.0));
        assert!(!r.contains(37.0, 40.0));
    }
}
