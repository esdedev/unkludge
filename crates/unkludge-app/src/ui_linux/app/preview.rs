//! The preview of the file on the keyboard's screen. Decoding frames is most of what an open window
//! costs, so nothing gets decoded unless it can be seen and it's another frame.

use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Image, Rgb8Pixel, SharedPixelBuffer, TimerMode};

use super::{App, with_app};
use crate::state::{preview_frame, preview_interval};
use crate::ui_linux::View;
use crate::worker::rgb_from_565;

/// A window that's minimized looks at this rarely, and it's nothing but looking.
const MINIMIZED_CHECK: Duration = Duration::from_secs(1);

impl App {
    pub(in crate::ui_linux) fn restart_preview(&mut self) {
        self.preview_since = Instant::now();
        self.preview_shown = None;
        self.update_preview();
        self.sync_preview_timer();
    }

    /// Runs the preview's timer if there's an animation to show and a window that can show it. A
    /// minimized window can't say when it's back, so there it keeps looking, rarely.
    pub(in crate::ui_linux) fn sync_preview_timer(&mut self) {
        let window = self.window.window();
        let wanted = wanted_preview_timer(
            window.is_visible(),
            window.is_minimized(),
            self.state
                .animation()
                .map(|a| (a.frames().len(), a.delay_ms())),
        );
        if wanted == self.preview_every {
            return;
        }
        self.preview_every = wanted;
        match wanted {
            Some(every) => self.preview_timer.start(TimerMode::Repeated, every, || {
                with_app(|app| {
                    app.update_preview();
                    app.sync_preview_timer();
                });
            }),
            None => self.preview_timer.stop(),
        }
    }

    /// Puts the frame that's due into the window, unless it's there already. There's nothing to do
    /// while nobody can see it: decoding frames is most of what an open window costs.
    pub(in crate::ui_linux) fn update_preview(&mut self) {
        let Some(animation) = self.state.animation().cloned() else {
            if self.preview_shown.take().is_some() {
                View::get(&self.window).set_picture(Image::default());
            }
            return;
        };
        if !self.is_showing() {
            return;
        }
        let frames = animation.frames();
        let elapsed = self.preview_since.elapsed().as_millis();
        let at = preview_frame(elapsed, frames.len(), animation.delay_ms());
        let same = self
            .preview_shown
            .as_ref()
            .is_some_and(|(held, shown)| Arc::ptr_eq(held, &animation) && *shown == at);
        if same {
            return;
        }
        let Some(frame) = frames.get(at) else {
            return;
        };
        let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(
            u32::from(animation.width()),
            u32::from(animation.height()),
        );
        let rgb = rgb_from_565(frame);
        if rgb.len() == pixels.as_bytes().len() {
            pixels.make_mut_bytes().copy_from_slice(&rgb);
        }
        View::get(&self.window).set_picture(Image::from_rgb8(pixels));
        self.preview_shown = Some((animation, at));
        #[cfg(test)]
        {
            self.frames_decoded += 1;
        }
    }
}

/// How often the preview should look for its next frame, if at all.
pub(in crate::ui_linux) fn wanted_preview_timer(
    visible: bool,
    minimized: bool,
    animation: Option<(usize, u16)>,
) -> Option<Duration> {
    let (_, delay) = animation.filter(|(frames, _)| *frames > 1)?;
    if !visible {
        return None;
    }
    if minimized {
        return Some(MINIMIZED_CHECK);
    }
    Some(Duration::from_millis(u64::from(preview_interval(delay))))
}
