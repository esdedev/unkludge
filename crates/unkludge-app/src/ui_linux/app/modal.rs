//! The dialogs the window draws for itself. See `slint/dialogs.slint`.

use slint::Global;
use unkludge_protocol::lighting::ColorChoice;

use super::{App, ColorEdit};
use crate::ui_linux::{DialogKind, View, view};

impl App {
    pub(in crate::ui_linux) fn open_dialog(&mut self, kind: DialogKind, by_keyboard: bool) {
        self.dialog_by_keyboard = by_keyboard;
        View::get(&self.window).set_dialog(kind);
        self.window.invoke_focus_dialog();
    }

    pub(in crate::ui_linux) fn close_dialog(&mut self) {
        let kind = View::get(&self.window).get_dialog();
        View::get(&self.window).set_dialog(DialogKind::None);
        self.color = None;
        // With the keyboard gone, what opened the dialog has gone out of sight too.
        if self.dialog_by_keyboard && self.state.model().is_some() {
            match kind {
                DialogKind::Color => self.window.invoke_focus_custom_color(),
                DialogKind::Path => self.window.invoke_focus_choose_file(),
                _ => {}
            }
        } else {
            self.window.invoke_drop_focus();
        }
    }

    /// Escape, Cancel or OK on what has nothing else to say.
    pub(in crate::ui_linux) fn dialog_dismissed(&mut self) {
        let was_note = View::get(&self.window).get_dialog() == DialogKind::Note;
        self.close_dialog();
        if was_note {
            self.hide_window();
        }
    }

    pub(in crate::ui_linux) fn open_color_dialog(&mut self, by_keyboard: bool) {
        let rgb = self.state.slot_colors(&self.settings)[0];
        let warning = self.state.custom_color_resets_others(&self.settings);
        self.color = Some(ColorEdit { rgb, warning });
        View::get(&self.window).set_color_dialog(view::color_dialog(
            rgb,
            &view::color_code(rgb),
            warning,
        ));
        self.open_dialog(DialogKind::Color, by_keyboard);
    }

    pub(in crate::ui_linux) fn color_channel(&mut self, channel: i32, value: i32) {
        let Some(edit) = &mut self.color else {
            return;
        };
        let value = value.clamp(0, 255) as u8;
        match channel {
            0 => edit.rgb.r = value,
            1 => edit.rgb.g = value,
            _ => edit.rgb.b = value,
        }
        let edit = *edit;
        View::get(&self.window).set_color_dialog(view::color_dialog(
            edit.rgb,
            &view::color_code(edit.rgb),
            edit.warning,
        ));
    }

    /// A code being typed takes effect once it's a whole one, and the field keeps what's in it.
    pub(in crate::ui_linux) fn color_code_edited(&mut self, code: &str) {
        let Some(edit) = &mut self.color else {
            return;
        };
        if let Some(rgb) = view::parse_color_code(code) {
            edit.rgb = rgb;
        }
        let edit = *edit;
        View::get(&self.window).set_color_dialog(view::color_dialog(edit.rgb, code, edit.warning));
    }

    pub(in crate::ui_linux) fn color_ok(&mut self) {
        if let Some(edit) = self.color {
            self.send(self.state.set_color(ColorChoice::Custom(edit.rgb)));
        }
        self.close_dialog();
    }
}
