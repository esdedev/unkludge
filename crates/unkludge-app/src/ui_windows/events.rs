//! What the worker says, and what follows from it in the window. See `drive` for what doesn't depend
//! on the window.

use std::sync::atomic::Ordering;
use std::time::Instant;

use windows::Win32::UI::Input::KeyboardAndMouse::VK_TAB;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::App;
use super::window::{TIMER_MESSAGE, TIMER_PREVIEW, TIMER_SHOT};
use crate::demo::Script;
use crate::drive::{Drained, MESSAGE_TIME, PROGRESS_PAINT_GAP, Parts};
use crate::state::preview_interval;
use crate::worker::Cmd;
use crate::{log, sys};

impl App {
    pub(super) fn drain_events(&mut self) {
        self.woken.store(false, Ordering::SeqCst);
        let drained = self.take_events();
        if drained.save_settings {
            self.save_settings();
        }
        self.to_open.extend(drained.to_open);
        if drained.upload_ended {
            self.upload_ended();
        }
        self.refresh_frame();
        if drained.image_changed {
            self.restart_preview();
        }
        if let Some(fades) = drained.message_fades {
            // SAFETY: the handle is this window's own.
            unsafe {
                if fades {
                    SetTimer(
                        Some(self.hwnd),
                        TIMER_MESSAGE,
                        MESSAGE_TIME.as_millis() as u32,
                        None,
                    );
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_MESSAGE);
                }
            }
        }
        self.maybe_schedule_shot();
        if !drained.only_progress || self.last_paint.elapsed() >= PROGRESS_PAINT_GAP {
            self.invalidate();
        }
    }

    fn take_events(&mut self) -> Drained {
        let Some(events) = &self.events else {
            return Drained::default();
        };
        let worker = &self.worker;
        let mut send = |cmd: Cmd| {
            if let Some(worker) = worker {
                worker.send(cmd);
            }
        };
        Parts {
            state: &mut self.state,
            settings: &mut self.settings,
            session: &self.session,
            script: &mut self.script,
            data_dir: sys::data_dir(),
            send: &mut send,
        }
        .drain(events)
    }

    /// Starts an animation's preview from its first frame.
    pub fn restart_preview(&mut self) {
        self.preview_since = Instant::now();
        self.sync_preview_timer();
    }

    /// Runs the preview's timer if there's an animation to show and a window that can show it.
    pub fn sync_preview_timer(&mut self) {
        let showing = self.is_showing();
        let wanted = self
            .state
            .animation()
            .filter(|animation| animation.frames().len() > 1 && showing)
            .map(|animation| preview_interval(animation.delay_ms()));
        if wanted != self.preview_timer {
            // SAFETY: the handle is this window's own.
            unsafe {
                match wanted {
                    Some(ms) => {
                        SetTimer(Some(self.hwnd), TIMER_PREVIEW, ms, None);
                    }
                    None => {
                        let _ = KillTimer(Some(self.hwnd), TIMER_PREVIEW);
                    }
                }
            }
            self.preview_timer = wanted;
        }
    }

    // Scripted demos.

    fn maybe_schedule_shot(&mut self) {
        let Some(args) = self.session.demo_args() else {
            return;
        };
        let reached = self.script.as_ref().is_some_and(Script::reached);
        if args.shot.is_some() && !self.shot_timer_set && reached {
            self.shot_timer_set = true;
            let delay = args.shot_delay.as_millis().min(u128::from(u32::MAX)) as u32;
            // SAFETY: the handle is this window's own.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_SHOT, delay.max(1), None);
            }
        }
    }

    pub(super) fn take_shot(&mut self) {
        let Some(path) = self.session.demo_args().and_then(|args| args.shot.clone()) else {
            return;
        };
        let tabs = self.script.as_ref().map_or(0, Script::tabs_before_shot);
        // The keyboard works on what was last drawn, so draw once first.
        let mut saved = self.save_shot(&path);
        if tabs > 0 {
            for _ in 0..tabs {
                self.key_down(VK_TAB.0, false);
            }
            saved = self.save_shot(&path);
        }
        if let Err(error) = saved {
            log::error(&format!("the picture can't be saved: {error}"));
        }
        self.quit = true;
    }
}
