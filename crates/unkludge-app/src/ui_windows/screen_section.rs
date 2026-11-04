//! The screen section: the preview, the file, how it fits, and the upload.

use std::sync::Arc;

use unkludge_protocol::model::ScreenSpec;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;

use super::gfx::{
    ACCENT, ACCENT_HI, BORDER, BORDER_HI, CYAN, Draw, MUTED, R, SCREEN_DARK, SILVER, TEXT, TRACK,
    WELL, alpha,
};
use super::header::{fitted_text, tone_color};
use super::{App, Hit, Role, TextItem};
use crate::i18n::t;
use crate::layout::{self, centered, split_row};
use crate::state::{FITS, Upload, WellHint, preview_frame};
use crate::worker::rgb_from_565;

/// `R G B` pixels as the `B G R A` a bitmap holds.
fn bgra_from_rgb(rgb: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgb.as_chunks::<3>();
    pixels
        .iter()
        .flat_map(|&[r, g, b]| [b, g, r, 255])
        .collect()
}

impl App {
    /// Puts the frame that's due into the preview's bitmap, unless it's there already. Says whether
    /// it changed.
    pub fn update_preview(&mut self) -> bool {
        let Some(animation) = self.state.animation().cloned() else {
            return false;
        };
        let frames = animation.frames();
        let elapsed = self.preview_since.elapsed().as_millis();
        let at = preview_frame(elapsed, frames.len(), animation.delay_ms());
        let Some(frame) = frames.get(at) else {
            return false;
        };
        let size = (u32::from(animation.width()), u32::from(animation.height()));
        let Some(gfx) = self.gfx.as_mut() else {
            return false;
        };
        let Ok(preview) = gfx.preview_bitmap(size) else {
            return false;
        };
        let already = preview
            .holds
            .as_ref()
            .is_some_and(|(held, shown)| Arc::ptr_eq(held, &animation) && *shown == at);
        if already {
            return false;
        }
        let pixels = bgra_from_rgb(&rgb_from_565(frame));
        // SAFETY: `pixels` has a full frame at this pitch, and is read only during the call.
        let copied = unsafe {
            preview
                .bitmap
                .CopyFromMemory(None, pixels.as_ptr().cast(), size.0 * 4)
        };
        if copied.is_err() {
            return false;
        }
        preview.holds = Some((animation, at));
        true
    }

    /// Where the picture goes, for this model's screen.
    fn preview_pane(&self, size: (u16, u16)) -> R {
        let rows = layout::screen_rows(self.frame.screen);
        centered(rows.well, f32::from(size.0), f32::from(size.1)).into()
    }

    /// The next frame of an animation, leaving the rest of the window as it is.
    pub fn paint_preview_frame(&mut self) {
        let Some(animation) = self.state.animation().cloned() else {
            return;
        };
        if !self.update_preview() {
            return;
        }
        let Some(gfx) = self.gfx.as_ref() else {
            return;
        };
        let Some(preview) = gfx.preview.as_ref() else {
            return;
        };
        let d = Draw {
            rt: &gfx.rt,
            brush: &gfx.brush,
            dw: &self.dw,
        };
        let pane = self.preview_pane((animation.width(), animation.height()));
        if d.just_bitmap(&preview.bitmap, pane, self.dpi == 96.0)
            .is_err()
        {
            self.gfx = None;
            self.invalidate();
        }
    }

    pub fn paint_screen(&self, d: &Draw, screen: ScreenSpec, preview: Option<&ID2D1Bitmap>) {
        let c = R::from(self.frame.screen);
        self.card(d, c, t().screen, "\u{E7F4}");
        d.text(
            &t().screen_info(screen.width, screen.height, screen.max_frames),
            &self.fonts.small_r,
            R::new(c.x1 - 240.0, c.y0 + 14.0, c.x1 - 18.0, c.y0 + 34.0),
            MUTED,
        );
        let rows = layout::screen_rows(self.frame.screen);
        self.paint_well(d, rows.well.into(), (screen.width, screen.height), preview);
        self.paint_file_row(d, rows.file.into());
        self.paint_fit_row(d, rows.fit.into());
        self.paint_notes(d, rows.notes.into());
        self.paint_action_row(d, rows.action.into());
    }

    fn paint_well(&self, d: &Draw, well: R, size: (u16, u16), preview: Option<&ID2D1Bitmap>) {
        d.round(well, 12.0, WELL);
        let hovered = self.hover == Some(Hit::Preview);
        let edge = if hovered { BORDER_HI } else { BORDER };
        d.round_stroke(well, 12.0, edge, 1.0);
        let pane = self.preview_pane(size);
        // A thin frame, as the screen has on the keyboard.
        let frame = R::new(pane.x0 - 2.0, pane.y0 - 2.0, pane.x1 + 2.0, pane.y1 + 2.0);
        let loaded = self.state.animation().is_some();
        d.round(frame, 4.0, alpha(ACCENT, if loaded { 0.45 } else { 0.25 }));
        d.round(pane, 2.0, SCREEN_DARK);
        match preview.filter(|_| loaded) {
            Some(bitmap) => d.bitmap(bitmap, pane, self.dpi == 96.0),
            None => self.paint_well_hint(d, well, pane),
        }
        self.item(
            well,
            Hit::Preview,
            self.state.preview_name(),
            Role::Button,
            false,
            true,
        );
    }

    fn paint_well_hint(&self, d: &Draw, well: R, pane: R) {
        let WellHint { main, more } = self.state.well_hint();
        if more.is_empty() {
            d.text(&main, &self.fonts.body_c, pane, SILVER);
            return;
        }
        // A small screen can't hold the words, so they go under it and it keeps the arrow.
        let fits = d.text_width(&main, &self.fonts.body_c) + 16.0 <= pane.w();
        let mid = pane.mid_y();
        let (arrow_mid, text_top) = if fits {
            (mid - 28.0, mid - 14.0)
        } else {
            (mid, pane.y1 + 6.0)
        };
        d.text(
            "\u{E898}",
            &self.fonts.icon,
            R::new(pane.x0, arrow_mid - 12.0, pane.x1, arrow_mid + 12.0),
            ACCENT_HI,
        );
        d.text(
            &main,
            &self.fonts.body_c,
            R::new(well.x0, text_top, well.x1, text_top + 20.0),
            TEXT,
        );
        d.text(
            &more,
            &self.fonts.small_c,
            R::new(well.x0, text_top + 20.0, well.x1, text_top + 38.0),
            MUTED,
        );
    }

    fn paint_file_row(&self, d: &Draw, row: R) {
        let button_w = (d.text_width(t().choose_file, &self.fonts.button) + 26.0).max(110.0);
        let button = R::new(row.x1 - button_w, row.y0 + 2.0, row.x1, row.y1 - 2.0);
        d.button(
            button,
            t().choose_file,
            &self.fonts.button,
            false,
            self.look(Hit::ChooseFile, false, true),
        );
        self.item(
            button,
            Hit::ChooseFile,
            t().choose_file,
            Role::Button,
            false,
            true,
        );
        let text = R::new(row.x0, row.y0, button.x0 - 10.0, row.y1);
        match self.state.file_name() {
            Some(name) => {
                self.text_item(text, TextItem::File, &name, Some(self.state.file_detail()));
                let name = fitted_text(d, &name, &self.fonts.body, text.w());
                d.text(
                    &name,
                    &self.fonts.body,
                    R::new(text.x0, row.y0, text.x1, row.y0 + 19.0),
                    TEXT,
                );
                d.text(
                    &self.state.file_detail(),
                    &self.fonts.small,
                    R::new(text.x0, row.y0 + 19.0, text.x1, row.y1),
                    MUTED,
                );
            }
            None => d.text(t().no_file, &self.fonts.body, text, MUTED),
        }
    }

    fn paint_fit_row(&self, d: &Draw, row: R) {
        let chips = R::new(row.x0, row.y0, row.x0 + 234.0, row.y1);
        for (fit, part) in FITS.iter().zip(split_row(
            (chips.x0, chips.y0, chips.x1, chips.y1),
            FITS.len(),
            6.0,
        )) {
            let hit = Hit::Fit(*fit);
            let selected = self.state.fit == *fit;
            d.chip(
                part.into(),
                t().fit_name(*fit),
                &self.fonts.chip,
                self.look(hit, selected, true),
            );
            self.item(
                part.into(),
                hit,
                t().labelled(t().fit, t().fit_name(*fit)),
                Role::Choice,
                selected,
                true,
            );
        }
        let Some(delay) = self.state.frame_delay() else {
            return;
        };
        let slower = R::new(row.x1 - 30.0, row.y0, row.x1, row.y1);
        let faster = R::new(row.x1 - 118.0, row.y0, row.x1 - 88.0, row.y1);
        d.chip(
            faster,
            "\u{2212}",
            &self.fonts.chip,
            self.look(Hit::Faster, false, true),
        );
        d.chip(
            slower,
            "+",
            &self.fonts.chip,
            self.look(Hit::Slower, false, true),
        );
        d.text(
            &format!("{delay} ms"),
            &self.fonts.small_c,
            R::new(faster.x1, row.y0, slower.x0, row.y1),
            SILVER,
        );
        let name = |what: &str| format!("{}: {what}", t().frame_time);
        self.item(
            faster,
            Hit::Faster,
            name(t().faster),
            Role::Button,
            false,
            true,
        );
        self.item(
            slower,
            Hit::Slower,
            name(t().slower),
            Role::Button,
            false,
            true,
        );
    }

    fn paint_notes(&self, d: &Draw, row: R) {
        let notes = self.state.screen_notes();
        let Some(first) = notes.first() else {
            return;
        };
        let text = notes
            .iter()
            .map(|note| note.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        d.text(&text, &self.fonts.note, row, tone_color(first.tone));
        self.text_item(row, TextItem::Notes, &text, None);
    }

    fn paint_action_row(&self, d: &Draw, row: R) {
        match self.state.upload {
            Upload::Idle => {
                let enabled = self.state.can_upload();
                let button = R::new(row.x0, row.y0, row.x0 + 196.0, row.y1);
                d.button(
                    button,
                    t().upload,
                    &self.fonts.button,
                    true,
                    self.look(Hit::Upload, false, enabled),
                );
                self.item(
                    button,
                    Hit::Upload,
                    t().upload,
                    Role::Button,
                    false,
                    enabled,
                );
                if let Some(estimate) = self.state.estimate_text() {
                    d.text(
                        &estimate,
                        &self.fonts.small_r,
                        R::new(button.x1 + 10.0, row.y0, row.x1, row.y1),
                        MUTED,
                    );
                }
            }
            Upload::Starting { .. } | Upload::Running { .. } => self.paint_progress(d, row),
        }
    }

    fn paint_progress(&self, d: &Draw, row: R) {
        let cancelling = self.state.upload.is_cancelling();
        let cancel_w = (d.text_width(t().cancel, &self.fonts.button) + 28.0).max(84.0);
        let cancel = R::new(row.x1 - cancel_w, row.y0, row.x1, row.y1);
        let enabled = !cancelling;
        d.button(
            cancel,
            t().cancel,
            &self.fonts.button,
            false,
            self.look(Hit::Cancel, false, enabled),
        );
        self.item(
            cancel,
            Hit::Cancel,
            t().cancel,
            Role::Button,
            false,
            enabled,
        );

        let area = R::new(row.x0, row.y0, cancel.x0 - 12.0, row.y1);
        let share = self.state.upload.share();
        let progress = self.state.progress_text();
        self.text_item(
            area,
            TextItem::Progress,
            t().uploading,
            Some(progress.percent.clone()),
        );
        d.text(
            &progress.label,
            &self.fonts.small,
            R::new(area.x0, area.y0, area.x0 + 110.0, area.y0 + 18.0),
            TEXT,
        );
        d.text(
            &progress.remaining,
            &self.fonts.small_r,
            R::new(area.x0 + 110.0, area.y0, area.x1, area.y0 + 18.0),
            MUTED,
        );
        let bar = R::new(area.x0, area.y1 - 11.0, area.x1, area.y1 - 3.0);
        d.round(bar, 4.0, TRACK);
        if share > 0.0 {
            let fill = R::new(bar.x0, bar.y0, bar.x0 + bar.w() * share, bar.y1);
            let color = if cancelling { CYAN } else { ACCENT_HI };
            d.round(fill, 4.0, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_go_into_the_order_a_bitmap_holds() {
        assert_eq!(bgra_from_rgb(&[1, 2, 3]), [3, 2, 1, 255]);
        assert_eq!(
            bgra_from_rgb(&[1, 2, 3, 4, 5, 6]),
            [3, 2, 1, 255, 6, 5, 4, 255]
        );
    }
}
