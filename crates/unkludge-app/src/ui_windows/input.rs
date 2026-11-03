//! The mouse and the keyboard. Both work on the list of items that the last paint left.

use unkludge_protocol::lighting::ColorChoice;
use windows::Win32::Foundation::{LPARAM, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Accessibility::NotifyWinEvent;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DOWN, VK_LEFT,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_FOCUS, GetCursorPos, HTCLIENT, IDC_HAND, LoadCursorW, OBJID_CLIENT, SetCursor,
};

use super::gfx::{ACCENT_HI, Draw, R, alpha};
use super::{App, Deferred, Hit};
use crate::layout;
use crate::state::Slider;
use crate::worker::Cmd;

/// What Tab does: the next or previous of the enabled items, around the end of the list.
fn next_focus(hits: &[Hit], focus: Option<Hit>, backwards: bool) -> Option<Hit> {
    if hits.is_empty() {
        return None;
    }
    let count = hits.len();
    let index = match focus.and_then(|focus| hits.iter().position(|hit| *hit == focus)) {
        Some(at) if backwards => (at + count - 1) % count,
        Some(at) => (at + 1) % count,
        None if backwards => count - 1,
        None => 0,
    };
    Some(hits[index])
}

impl App {
    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.shared
            .borrow()
            .items
            .iter()
            .rev()
            .find(|item| item.enabled && item.rect.contains(x, y))
            .map(|item| item.hit)
    }

    /// Moves the focus, and tells a screen reader that's listening.
    pub fn set_focus(&mut self, focus: Option<Hit>) {
        let child = {
            let mut shared = self.shared.borrow_mut();
            if shared.focus == focus {
                return;
            }
            shared.focus = focus;
            focus.and_then(|focus| shared.items.iter().position(|item| item.hit == focus))
        };
        if let Some(child) = child {
            // SAFETY: the handle is this window's own. Children count from 1.
            unsafe {
                NotifyWinEvent(
                    EVENT_OBJECT_FOCUS,
                    self.hwnd,
                    OBJID_CLIENT.0,
                    child as i32 + 1,
                );
            }
        }
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) {
        if self.drag.is_some() {
            self.drag_to(x);
            return;
        }
        let hover = self.hit_at(x, y);
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
        if !self.tracking_mouse {
            self.tracking_mouse = true;
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: the structure is valid for the call.
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
        }
    }

    pub fn mouse_left(&mut self) {
        self.tracking_mouse = false;
        if self.hover.take().is_some() {
            self.invalidate();
        }
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) {
        self.show_focus = false;
        let hit = self.hit_at(x, y);
        self.set_focus(hit);
        match hit {
            Some(Hit::Brightness) => self.begin_drag(Slider::Brightness, x),
            Some(Hit::Speed) => self.begin_drag(Slider::Speed, x),
            Some(hit) => self.activate(hit),
            None => {}
        }
        self.invalidate();
    }

    pub fn end_drag(&mut self) {
        if self.drag.take().is_some() {
            // SAFETY: no arguments, and it's fine if this thread no longer has the mouse.
            unsafe {
                let _ = ReleaseCapture();
            }
            self.invalidate();
        }
    }

    /// The part of the window a slider's thumb runs along.
    pub fn slider_track(&self, hit: Hit) -> (f32, f32) {
        let rows = layout::light_rows(self.frame.lights);
        let parts = layout::split_row(rows.sliders, 2, 24.0);
        let part = if hit == Hit::Speed {
            parts[1]
        } else {
            parts[0]
        };
        (part.0 + 8.0, part.2 - 8.0)
    }

    /// Moves a slider being dragged to where the mouse is.
    pub fn drag_to(&mut self, x: f32) {
        if let Some(slider) = self.drag {
            let hit = match slider {
                Slider::Brightness => Hit::Brightness,
                Slider::Speed => Hit::Speed,
            };
            let (x0, x1) = self.slider_track(hit);
            let cmd = self.state.drag_slider(slider, x, x0, x1);
            self.send(cmd);
        }
        self.invalidate();
    }

    /// One step along a slider, from the keyboard. Says whether it was a slider.
    fn nudge(&mut self, hit: Hit, by: i32) -> bool {
        let slider = match hit {
            Hit::Brightness => Slider::Brightness,
            Hit::Speed => Slider::Speed,
            _ => return false,
        };
        let cmd = self.state.nudge(slider, by);
        self.send(cmd);
        true
    }

    /// Does what an item is for.
    pub fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Report => self.send(Some(Cmd::Report)),
            Hit::Preview | Hit::ChooseFile => self.deferred = Some(Deferred::ChooseFile),
            Hit::Fit(fit) => {
                let cmd = self.state.set_fit(fit);
                self.send(cmd);
            }
            Hit::Slower | Hit::Faster => {
                let cmd = self.state.step_delay(hit == Hit::Slower);
                self.send(cmd);
            }
            Hit::Upload => {
                let cmd = self.state.start_upload();
                self.send(cmd);
            }
            Hit::Cancel => self.send(self.state.cancel_upload()),
            Hit::SyncClock => self.send(Some(Cmd::SyncClock)),
            Hit::AutoClock => self.toggle_auto_clock(),
            Hit::Mode(mode) => self.send(self.state.set_mode(mode)),
            // Moved by dragging or with the arrows. There's nothing to do for just pressing them.
            Hit::Brightness | Hit::Speed => {}
            Hit::Slot(slot) => self.send(self.state.set_color(ColorChoice::Slot(slot))),
            Hit::Rainbow => self.send(self.state.set_color(ColorChoice::Rainbow)),
            Hit::CustomColor => self.deferred = Some(Deferred::CustomColor),
            Hit::Sleep(sleep) => self.send(self.state.set_sleep(sleep)),
            Hit::Autostart => {
                let on = !self.autostart;
                self.set_autostart(on);
            }
        }
        self.invalidate();
    }

    /// Says whether the key meant something here.
    pub fn key_down(&mut self, key: u16) -> bool {
        let enabled: Vec<Hit> = self
            .shared
            .borrow()
            .items
            .iter()
            .filter(|item| item.enabled)
            .map(|item| item.hit)
            .collect();
        let focus = self.shared.borrow().focus;
        let key = windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(key);
        match key {
            VK_TAB => {
                // SAFETY: no pointers involved.
                let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
                self.move_focus(next_focus(&enabled, focus, shift));
                true
            }
            VK_RETURN | VK_SPACE => {
                if let Some(hit) = focus.filter(|hit| enabled.contains(hit)) {
                    self.show_focus = true;
                    self.activate(hit);
                }
                true
            }
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
                let forward = key == VK_RIGHT || key == VK_DOWN;
                let by = if key == VK_RIGHT || key == VK_UP {
                    1
                } else {
                    -1
                };
                let on_slider = focus.is_some_and(|hit| self.nudge(hit, by));
                if on_slider {
                    self.show_focus = true;
                    self.invalidate();
                } else {
                    // Among chips and the like, the arrows move as Tab does.
                    self.move_focus(next_focus(&enabled, focus, !forward));
                }
                true
            }
            _ => false,
        }
    }

    fn move_focus(&mut self, focus: Option<Hit>) {
        self.show_focus = true;
        self.set_focus(focus);
        self.invalidate();
    }

    /// The hand over what can be clicked.
    pub fn set_cursor(&self, lp: LPARAM) -> bool {
        if (lp.0 & 0xFFFF) as u32 != HTCLIENT {
            return false;
        }
        let mut point = POINT::default();
        // SAFETY: the point is written by the first call and read and written by the second.
        unsafe {
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(self.hwnd, &mut point);
        }
        let k = 96.0 / self.dpi;
        if self
            .hit_at(point.x as f32 * k, point.y as f32 * k)
            .is_none()
        {
            return false;
        }
        // SAFETY: loads a cursor every system has, which is never freed.
        unsafe {
            let _ = SetCursor(LoadCursorW(None, IDC_HAND).ok());
        }
        true
    }

    /// The ring around what has the focus, when it got there by keyboard.
    pub fn paint_focus(&self, d: &Draw) {
        if !self.show_focus {
            return;
        }
        let shared = self.shared.borrow();
        let Some(focus) = shared.focus else {
            return;
        };
        if let Some(item) = shared.items.iter().find(|item| item.hit == focus) {
            let r = item.rect;
            let ring = R::new(r.x0 - 3.0, r.y0 - 3.0, r.x1 + 3.0, r.y1 + 3.0);
            d.round(ring, 10.0, alpha(ACCENT_HI, 0.12));
            d.round_stroke(ring, 10.0, ACCENT_HI, 2.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_goes_through_the_items_and_around() {
        let hits = [Hit::Report, Hit::Preview, Hit::Upload];
        assert_eq!(next_focus(&hits, None, false), Some(Hit::Report));
        assert_eq!(
            next_focus(&hits, Some(Hit::Report), false),
            Some(Hit::Preview)
        );
        assert_eq!(
            next_focus(&hits, Some(Hit::Upload), false),
            Some(Hit::Report)
        );
    }

    #[test]
    fn shift_tab_goes_the_other_way() {
        let hits = [Hit::Report, Hit::Preview, Hit::Upload];
        assert_eq!(next_focus(&hits, None, true), Some(Hit::Upload));
        assert_eq!(
            next_focus(&hits, Some(Hit::Report), true),
            Some(Hit::Upload)
        );
        assert_eq!(
            next_focus(&hits, Some(Hit::Upload), true),
            Some(Hit::Preview)
        );
    }

    #[test]
    fn a_focus_that_has_gone_away_starts_over() {
        let hits = [Hit::Report, Hit::Preview];
        assert_eq!(
            next_focus(&hits, Some(Hit::Cancel), false),
            Some(Hit::Report)
        );
        assert_eq!(next_focus(&[], Some(Hit::Cancel), false), None);
    }
}
