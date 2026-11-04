//! The mouse and the keyboard. Both work on the list of items that the last paint left.

use unkludge_protocol::lighting::ColorChoice;
use windows::Win32::Foundation::{LPARAM, POINT};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Accessibility::NotifyWinEvent;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_DOWN,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_FOCUS, GetCursorPos, HTCLIENT, IDC_HAND, LoadCursorW, OBJID_CLIENT, SetCursor,
};

use super::gfx::{ACCENT_HI, Draw, R, alpha};
use super::{App, Deferred, Hit, Role};
use crate::layout;
use crate::state::Slider;
use crate::worker::Cmd;

/// What Tab does: the next or previous of what can be used, around the end of the list, starting
/// from where the focus is even if that has stopped being usable. Each item goes with whether it can
/// be used now.
fn next_focus(items: &[(Hit, bool)], focus: Option<Hit>, backwards: bool) -> Option<Hit> {
    let count = items.len();
    let at = focus.and_then(|focus| items.iter().position(|(hit, _)| *hit == focus));
    (1..=count)
        .map(|step| match (at, backwards) {
            (Some(at), false) => (at + step) % count,
            (Some(at), true) => (at + count - step) % count,
            (None, false) => step - 1,
            (None, true) => count - step,
        })
        .map(|index| items[index])
        .find(|(_, usable)| *usable)
        .map(|(hit, _)| hit)
}

/// Whether a key's message should do anything. Holding Space or Enter would otherwise do the same
/// thing over and over, each time something written to the keyboard. Arrows are meant to repeat.
fn key_acts(key: VIRTUAL_KEY, repeat: bool) -> bool {
    !(repeat && (key == VK_RETURN || key == VK_SPACE))
}

/// What an arrow key means: how far along a slider, and whether it's forwards among other things.
fn arrow_move(key: VIRTUAL_KEY) -> Option<(i32, bool)> {
    match key {
        VK_RIGHT => Some((1, true)),
        VK_UP => Some((1, false)),
        VK_DOWN => Some((-1, true)),
        VK_LEFT => Some((-1, false)),
        _ => None,
    }
}

/// Where the focus goes on clicking something. Not on Upload, as it stays out of sight after a click,
/// and a later Space would start another upload nobody can see was asked for.
fn focus_after_click(hit: Option<Hit>) -> Option<Hit> {
    hit.filter(|hit| *hit != Hit::Upload)
}

/// Where the focus goes when an upload ends. It went to Cancel by keyboard, so it comes back to
/// Upload, in sight. If it got there any other way nobody's looking for it, so it goes away.
fn focus_after_upload(focus: Option<Hit>, by_keyboard: bool) -> Option<Hit> {
    match focus {
        Some(Hit::Cancel) => by_keyboard.then_some(Hit::Upload),
        other => other,
    }
}

impl App {
    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.shared
            .borrow()
            .items
            .iter()
            .rev()
            .find(|item| item.is_usable() && item.rect.contains(x, y))
            .map(|item| item.hit)
    }

    /// Moves the focus, and tells a screen reader that's listening.
    pub fn set_focus(&mut self, focus: Option<Hit>) {
        self.shared.borrow_mut().focus = focus;
        self.announce_focus();
    }

    /// Tells a screen reader where the focus is, once what has it has been painted.
    pub fn announce_focus(&mut self) {
        let (focus, child) = {
            let shared = self.shared.borrow();
            let child = shared
                .focus
                .and_then(|focus| shared.items.iter().position(|item| item.hit == focus));
            (shared.focus, child)
        };
        if focus == self.announced_focus {
            return;
        }
        match child {
            Some(child) => {
                // SAFETY: the handle is this window's own. Children count from 1.
                unsafe {
                    NotifyWinEvent(
                        EVENT_OBJECT_FOCUS,
                        self.hwnd,
                        OBJID_CLIENT.0,
                        child as i32 + 1,
                    );
                }
                self.announced_focus = focus;
            }
            None if focus.is_none() => self.announced_focus = None,
            // Not painted yet: the next paint comes back here.
            None => {}
        }
    }

    /// An upload just started from the keyboard or a screen reader: Cancel is what's next.
    pub fn follow_upload(&mut self, hit: Hit) {
        if hit == Hit::Upload && !self.state.upload.is_idle() {
            self.set_focus(Some(Hit::Cancel));
        }
    }

    /// An upload just ended.
    pub fn upload_ended(&mut self) {
        let focus = self.shared.borrow().focus;
        self.set_focus(focus_after_upload(focus, self.show_focus));
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
        self.set_focus(focus_after_click(hit));
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
        let [brightness, speed] = layout::slider_parts(rows.sliders);
        layout::slider_track(if hit == Hit::Speed { speed } else { brightness })
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
            Hit::Text(_) => {}
        }
        self.invalidate();
    }

    /// Says whether the key meant something here.
    pub fn key_down(&mut self, key: u16, repeat: bool) -> bool {
        let key = VIRTUAL_KEY(key);
        if !key_acts(key, repeat) {
            return true;
        }
        let items: Vec<(Hit, bool)> = self
            .shared
            .borrow()
            .items
            .iter()
            .filter(|item| item.role != Role::Text)
            .map(|item| (item.hit, item.is_usable()))
            .collect();
        let focus = self.shared.borrow().focus;
        if let Some((by, forward)) = arrow_move(key) {
            if focus.is_some_and(|hit| self.nudge(hit, by)) {
                self.show_focus = true;
                self.invalidate();
            } else {
                // Among chips and the like, the arrows move as Tab does.
                self.move_focus(next_focus(&items, focus, !forward));
            }
            return true;
        }
        match key {
            VK_TAB => {
                // SAFETY: no pointers involved.
                let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
                self.move_focus(next_focus(&items, focus, shift));
                true
            }
            VK_RETURN | VK_SPACE => {
                let usable = focus.filter(|hit| items.contains(&(*hit, true)));
                if let Some(hit) = usable {
                    self.show_focus = true;
                    self.activate(hit);
                    self.follow_upload(hit);
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
    use unkludge_protocol::screen::Fit;

    fn usable(hits: &[Hit]) -> Vec<(Hit, bool)> {
        hits.iter().map(|hit| (*hit, true)).collect()
    }

    #[test]
    fn tab_goes_through_the_items_and_around() {
        let hits = usable(&[Hit::Report, Hit::Preview, Hit::Upload]);
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
        let hits = usable(&[Hit::Report, Hit::Preview, Hit::Upload]);
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
        let hits = usable(&[Hit::Report, Hit::Preview]);
        assert_eq!(
            next_focus(&hits, Some(Hit::Cancel), false),
            Some(Hit::Report)
        );
        assert_eq!(next_focus(&[], Some(Hit::Cancel), false), None);
    }

    #[test]
    fn tab_skips_what_cant_be_used() {
        let hits = [
            (Hit::Report, true),
            (Hit::Upload, false),
            (Hit::SyncClock, true),
        ];
        assert_eq!(
            next_focus(&hits, Some(Hit::Report), false),
            Some(Hit::SyncClock)
        );
        assert_eq!(
            next_focus(&hits, Some(Hit::SyncClock), true),
            Some(Hit::Report)
        );
    }

    #[test]
    fn tab_leaves_a_button_that_was_disabled_under_the_focus_to_its_neighbour() {
        // Cancel while cancelling.
        let hits = [
            (Hit::Fit(Fit::Stretch), true),
            (Hit::Cancel, false),
            (Hit::SyncClock, true),
        ];
        assert_eq!(
            next_focus(&hits, Some(Hit::Cancel), false),
            Some(Hit::SyncClock)
        );
        assert_eq!(
            next_focus(&hits, Some(Hit::Cancel), true),
            Some(Hit::Fit(Fit::Stretch))
        );
    }

    #[test]
    fn with_nothing_usable_there_is_nowhere_to_go() {
        assert_eq!(next_focus(&[(Hit::Upload, false)], None, false), None);
    }

    #[test]
    fn holding_space_or_enter_does_it_once() {
        for key in [VK_SPACE, VK_RETURN] {
            assert!(key_acts(key, false));
            assert!(!key_acts(key, true));
        }
    }

    #[test]
    fn holding_an_arrow_or_tab_keeps_going() {
        for key in [VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_TAB] {
            assert!(key_acts(key, true));
            assert!(key_acts(key, false));
        }
    }

    #[test]
    fn right_and_up_raise_a_slider_left_and_down_lower_it() {
        assert_eq!(arrow_move(VK_RIGHT).map(|(by, _)| by), Some(1));
        assert_eq!(arrow_move(VK_UP).map(|(by, _)| by), Some(1));
        assert_eq!(arrow_move(VK_LEFT).map(|(by, _)| by), Some(-1));
        assert_eq!(arrow_move(VK_DOWN).map(|(by, _)| by), Some(-1));
        assert_eq!(arrow_move(VK_TAB), None);
    }

    #[test]
    fn right_and_down_go_forwards_among_chips() {
        assert_eq!(arrow_move(VK_RIGHT).map(|(_, forward)| forward), Some(true));
        assert_eq!(arrow_move(VK_DOWN).map(|(_, forward)| forward), Some(true));
        assert_eq!(arrow_move(VK_LEFT).map(|(_, forward)| forward), Some(false));
        assert_eq!(arrow_move(VK_UP).map(|(_, forward)| forward), Some(false));
    }

    #[test]
    fn clicking_upload_leaves_no_focus_behind_for_space_to_find() {
        assert_eq!(focus_after_click(Some(Hit::Upload)), None);
        assert_eq!(
            focus_after_click(Some(Hit::SyncClock)),
            Some(Hit::SyncClock)
        );
        assert_eq!(focus_after_click(None), None);
    }

    #[test]
    fn the_focus_comes_back_to_upload_only_if_the_keyboard_took_it_away() {
        assert_eq!(
            focus_after_upload(Some(Hit::Cancel), true),
            Some(Hit::Upload)
        );
        assert_eq!(focus_after_upload(Some(Hit::Cancel), false), None);
    }

    #[test]
    fn an_upload_ending_leaves_a_focus_elsewhere_alone() {
        assert_eq!(
            focus_after_upload(Some(Hit::SyncClock), true),
            Some(Hit::SyncClock)
        );
        assert_eq!(focus_after_upload(None, true), None);
    }
}
