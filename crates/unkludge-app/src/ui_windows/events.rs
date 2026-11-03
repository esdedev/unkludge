//! What the worker says, and what follows from it in the window. A scripted demo moves along here too.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::VK_TAB;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::App;
use super::window::{TIMER_MESSAGE, TIMER_PREVIEW, TIMER_SHOT};
use crate::demo::Step;
use crate::i18n::t;
use crate::state::{Reaction, preview_interval};
use crate::worker::{self, Cmd, Event};
use crate::{log, report, sys};

/// How long passing news stays at the foot of the window.
const MESSAGE_TIME_MS: u32 = 6000;
const PROGRESS_PAINT_GAP: Duration = Duration::from_millis(250);

impl App {
    pub(super) fn drain_events(&mut self) {
        self.woken.store(false, Ordering::SeqCst);
        let message_before = self.state.message.clone();
        let mut image_changed = false;
        let mut only_progress = true;
        while let Ok(event) = self.events.try_recv() {
            only_progress &= matches!(event, Event::Progress { .. });
            if cfg!(debug_assertions) {
                print_event(&event);
            }
            image_changed |= matches!(
                event,
                Event::ImageLoaded(_) | Event::Disconnected | Event::ImageFailed(_)
            );
            self.take_event(event);
        }
        self.refresh_frame();
        if image_changed {
            self.restart_preview();
        }
        if self.state.message != message_before
            && let Some(message) = &self.state.message
        {
            // SAFETY: the handle is this window's own.
            unsafe {
                if message.fades() {
                    SetTimer(Some(self.hwnd), TIMER_MESSAGE, MESSAGE_TIME_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_MESSAGE);
                }
            }
        }
        self.maybe_schedule_shot();
        // A chunk can go every few milliseconds, and the bar needn't show every one.
        if !only_progress || self.last_paint.elapsed() >= PROGRESS_PAINT_GAP {
            self.invalidate();
        }
    }

    fn take_event(&mut self, event: Event) {
        let steps = match &mut self.demo {
            Some(demo) => demo.script.on_event(&event),
            None => Vec::new(),
        };
        let reaction = self.state.apply(event, &mut self.settings);
        self.react(reaction);
        for step in steps {
            match step {
                Step::ChooseFile(path) => {
                    let cmd = self.state.choose_file(path);
                    self.send(cmd);
                }
                Step::Upload => {
                    let cmd = self.state.start_upload();
                    self.send(cmd);
                }
                Step::Cancel => {
                    let cmd = self.state.cancel_upload();
                    self.send(cmd);
                }
                Step::Report => self.send(Some(Cmd::Report)),
                Step::Pretend(event) => self.take_event(event),
            }
        }
    }

    fn react(&mut self, reaction: Reaction) {
        for cmd in reaction.cmds {
            self.send(Some(cmd));
        }
        if reaction.save_settings {
            self.save_settings();
        }
        if let Some(data) = reaction.report {
            self.report_ready(&data);
        }
    }

    fn report_ready(&mut self, data: &worker::ReportData) {
        let saved = if self.is_demo() {
            report::save_demo(data)
        } else {
            report::save(data)
        };
        let note = match saved {
            Ok(path) => {
                // `FORM_URL` is `None` until the project is public, so showing the file is all
                // there's to do. A scripted demo leaves no folder window open behind it.
                if !self.scripted() {
                    let folder = path.parent().unwrap_or(&path);
                    if let Err(error) = sys::open(folder) {
                        log::error(&format!("the report's folder can't be opened: {error}"));
                    }
                }
                if let Some(form) = report::FORM_URL {
                    let _ = sys::open(std::path::Path::new(form));
                }
                t().report_saved.to_string()
            }
            Err(error) => format!("{}: {error}", t().report_failed),
        };
        self.state.message = Some(crate::state::Note {
            text: note,
            tone: crate::state::Tone::Neutral,
        });
    }

    /// Starts the preview's timer, if there's an animation to show and a window to show it in.
    pub fn restart_preview(&mut self) {
        self.preview_since = Instant::now();
        let wanted = self
            .state
            .animation()
            .filter(|animation| animation.frames().len() > 1 && self.is_visible())
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
        let Some(demo) = &mut self.demo else {
            return;
        };
        if demo.args.shot.is_some() && !demo.shot_timer_set && demo.script.reached() {
            demo.shot_timer_set = true;
            let delay = demo.args.shot_delay.as_millis().min(u128::from(u32::MAX)) as u32;
            // SAFETY: the handle is this window's own.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_SHOT, delay.max(1), None);
            }
        }
    }

    pub(super) fn take_shot(&mut self) {
        let Some(path) = self.demo.as_ref().and_then(|demo| demo.args.shot.clone()) else {
            return;
        };
        let tabs = self
            .demo
            .as_ref()
            .map_or(0, |demo| demo.script.tabs_before_shot());
        // The keyboard works on what was last drawn, so draw once first.
        let mut saved = self.save_shot(&path);
        if tabs > 0 {
            for _ in 0..tabs {
                self.key_down(VK_TAB.0);
            }
            saved = self.save_shot(&path);
        }
        if let Err(error) = saved {
            log::error(&format!("the picture can't be saved: {error}"));
        }
        self.quit = true;
    }
}

/// What the worker says, on the console of a debug build. Frames are many, and progress is constant.
fn print_event(event: &Event) {
    match event {
        Event::ImageLoaded(image) => {
            eprintln!(
                "image: {} frames, {} sent, {} chunks each, {:?}",
                image.animation.frames().len(),
                image.plan.frame_count(),
                image.plan.chunks_per_frame,
                image.estimate
            );
        }
        Event::Progress { .. } => {}
        other => eprintln!("{other:?}"),
    }
}
