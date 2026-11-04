//! The window itself: its size, showing and hiding it, and the tray.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use slint::platform::WindowEvent;
use slint::winit_030::WinitWindowAccessor;
use slint::{CloseRequestResponse, ComponentHandle, Global, Timer, TimerMode};

use super::wiring::raise;
use super::{App, with_app};
use crate::i18n::t;
use crate::session::SystemAutostart;
use crate::state::{Note, Tone};
use crate::ui_linux::tray::{self, Close};
use crate::ui_linux::{DialogKind, View, fit, view};
use crate::worker::Cmd;

/// A window minimized at login because there was no tray yet goes into a tray that turns up this soon.
const TRAY_GRACE: Duration = Duration::from_secs(60);
/// Closing again this soon after minimizing means that it didn't work.
const MINIMIZE_GRACE: Duration = Duration::from_secs(2);
/// A new window doesn't always reach the window manager straight away.
const MINIMIZE_AGAIN_AFTER: Duration = Duration::from_millis(300);
/// Input that was already on its way when the window was minimized doesn't mean it's back.
const SEEN_GRACE: Duration = Duration::from_millis(500);
/// How long to wait for the window to exist, or for a bigger one, and how many times.
const FIT_RETRY: Duration = Duration::from_millis(100);
const MAX_FIT_TRIES: u32 = 50;
const MINIMIZE_RETRY: Duration = Duration::from_millis(100);
const MAX_MINIMIZE_TRIES: u32 = 100;
/// A window being dragged says so all the way, and only where it ends up matters.
const REFIT_AFTER_MOVE: Duration = Duration::from_millis(300);
/// How often a window that keeps being resized gets told its size, in case that isn't Slint's doing
/// and it's something that won't take no for an answer, as a tiling window manager.
const MAX_SIZE_REFITS: u32 = 5;

impl App {
    /// A picture has no screen, so it's the size a demo says the screen allows.
    pub(in crate::ui_linux) fn fit_offscreen(&mut self) {
        let height = self.frame.height;
        let (forced_dpi, work_area) = self
            .session
            .demo_args()
            .map_or((None, None), |args| (args.dpi, args.work_area));
        let os_scale = forced_dpi.map_or(1.0, |dpi| dpi as f32 / 96.0);
        let scale = work_area.map_or(os_scale, |work| fit::scale_for(os_scale, height, work));
        self.window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            });
        let (width, pixels) = fit::size_at(height, scale);
        self.window
            .window()
            .set_size(slint::PhysicalSize::new(width, pixels));
        self.sync_hairline();
    }

    fn sync_hairline(&self) {
        let scale = self.window.window().scale_factor();
        View::get(&self.window).set_hairline(fit::hairline_at(scale));
    }

    /// The window's size for the layout, drawing smaller if the screen is too small for it. Comes
    /// back to look again until the window exists, and until it has grown enough to draw larger.
    pub(in crate::ui_linux) fn fit_window(&mut self) {
        if self.session.is_scripted() {
            self.fit_offscreen();
            return;
        }
        let work_area = self.session.demo_args().and_then(|args| args.work_area);
        let fitted = fit::fit(&self.window, self.frame.height, work_area);
        let settled = fitted.is_some_and(|fitted| !fitted.growing);
        if settled {
            self.fit_tries = 0;
        } else if self.window.window().is_visible() && self.fit_tries < MAX_FIT_TRIES {
            self.fit_tries += 1;
            Timer::single_shot(FIT_RETRY, || with_app(App::fit_window));
        }
        if cfg!(debug_assertions) {
            eprintln!("fitted at {fitted:?}: {:?}", self.window.window().size());
        }
        self.sync_hairline();
        self.can_drop = !fit::is_wayland(&self.window);
        View::get(&self.window).set_screen(view::screen(&self.state, self.can_drop));
    }

    /// The window has moved, maybe to another screen.
    pub(in crate::ui_linux) fn refit_soon(&mut self) {
        self.refit_timer
            .start(TimerMode::SingleShot, REFIT_AFTER_MOVE, || {
                with_app(App::fit_window);
            });
    }

    /// The window has another size. Slint asks for one in winit's pixels when the layout changes, which
    /// can come after this asked for one in real pixels, so this looks again.
    pub(in crate::ui_linux) fn window_resized(&mut self) {
        if self.size_refits < MAX_SIZE_REFITS {
            self.size_refits += 1;
            self.refit_soon();
        }
    }

    pub(in crate::ui_linux) fn is_showing(&self) -> bool {
        let window = self.window.window();
        window.is_visible() && !window.is_minimized()
    }

    pub(in crate::ui_linux) fn show_window(&mut self) {
        let window = self.window.window();
        // What was read before may be old by now, if the window was hidden.
        if !window.is_visible() {
            self.send(Some(Cmd::Refresh));
        }
        let _ = self.window.show();
        self.window.window().set_minimized(false);
        self.parked_at = None;
        self.minimized_at = None;
        self.autostart = self.session.autostart_now(&SystemAutostart, self.autostart);
        self.update_tray();
        self.restart_preview();
        self.refresh();
        self.fit_window();
        raise(&self.window);
    }

    /// Closing the window hides it, and while hidden nothing gets drawn, so what's for drawing and the
    /// memory it took can go.
    pub(in crate::ui_linux) fn hide_window(&mut self) {
        View::get(&self.window).set_dialog(DialogKind::None);
        let _ = self.window.hide();
        self.save_settings();
        self.restart_preview();
        trim_memory();
    }

    /// Asks for the window to go down to the dock. Slint only passes that on when it doesn't think
    /// it's minimized already, and a window that came back on Wayland leaves it thinking that it is,
    /// so this also tells winit directly.
    pub(in crate::ui_linux) fn minimize(&mut self) {
        self.window.window().set_minimized(true);
        self.window
            .window()
            .with_winit_window(|window| window.set_minimized(true));
        self.minimized_at = Some(Instant::now());
        self.sync_preview_timer();
    }

    /// Starts minimized. A window that doesn't exist yet can't be minimized, and Slint forgets being
    /// asked, while still thinking that it is, so this waits for the window.
    pub(in crate::ui_linux) fn park(&mut self) {
        self.parked_at = Some(Instant::now());
        self.minimize_once_there();
    }

    fn minimize_once_there(&mut self) {
        if self.window.window().has_winit_window() {
            self.minimize();
            let weak = self.window.as_weak();
            Timer::single_shot(MINIMIZE_AGAIN_AFTER, move || {
                if let Some(window) = weak.upgrade() {
                    window
                        .window()
                        .with_winit_window(|window| window.set_minimized(true));
                }
            });
        } else if self.minimize_tries < MAX_MINIMIZE_TRIES {
            self.minimize_tries += 1;
            Timer::single_shot(MINIMIZE_RETRY, || with_app(App::minimize_once_there));
        }
    }

    /// Something shows that the window is on screen. It's the only way to know that it came back on
    /// Wayland, where clients aren't told, and where Slint would go on thinking it's minimized.
    pub(in crate::ui_linux) fn window_seen(&mut self) {
        if !self.window.window().is_minimized() {
            return;
        }
        if self
            .minimized_at
            .is_some_and(|at| at.elapsed() < SEEN_GRACE)
        {
            return;
        }
        self.window.window().set_minimized(false);
        self.minimized_at = None;
        self.parked_at = None;
        self.restart_preview();
        self.refresh();
    }

    pub(in crate::ui_linux) fn close_requested(&mut self) -> CloseRequestResponse {
        let just_minimized = self
            .minimized_at
            .is_some_and(|at| at.elapsed() < MINIMIZE_GRACE);
        match tray::close(
            self.tray_online.load(Ordering::Relaxed),
            self.settings.tray_note_shown,
            just_minimized,
        ) {
            Close::NoteFirst => {
                self.settings.tray_note_shown = true;
                self.save_settings();
                self.open_dialog(DialogKind::Note, false);
                CloseRequestResponse::KeepWindowShown
            }
            Close::Hide | Close::HideAnyway => {
                self.minimized_at = None;
                self.hide_window();
                CloseRequestResponse::HideWindow
            }
            Close::Minimize => {
                self.minimize();
                self.say(t().linux.no_tray, Tone::Warning);
                self.save_settings();
                CloseRequestResponse::KeepWindowShown
            }
        }
    }

    pub(in crate::ui_linux) fn tray_came_online(&mut self) {
        // It might have been saying there wasn't one.
        if self
            .state
            .message
            .as_ref()
            .is_some_and(|message: &Note| message.text == t().linux.no_tray)
        {
            self.state.message = None;
            self.refresh();
        }
        let parked = self.parked_at.take();
        if parked.is_some_and(|at| at.elapsed() < TRAY_GRACE) {
            self.hide_window();
        }
    }

    /// A window hidden in a tray that has gone would be out of reach, so it goes to the dock.
    pub(in crate::ui_linux) fn tray_went_offline(&mut self) {
        if !self.window.window().is_visible() {
            let _ = self.window.show();
            self.minimize_once_there();
            self.say(t().linux.no_tray, Tone::Warning);
            self.fit_window();
        }
    }

    /// Starts with the system or not. A demo only pretends.
    pub(in crate::ui_linux) fn toggle_autostart(&mut self) {
        self.autostart = self
            .session
            .set_autostart(!self.autostart, &SystemAutostart);
        self.update_tray();
        self.refresh();
    }

    pub(in crate::ui_linux) fn toggle_auto_clock(&mut self) {
        self.settings.auto_clock = !self.settings.auto_clock;
        self.send(Some(Cmd::SetAutoClock(self.settings.auto_clock)));
        self.save_settings();
        self.refresh();
    }

    fn update_tray(&self) {
        // It waits for the tray's thread, which this one has no reason to do.
        if let Some(tray) = self.tray.clone() {
            let autostart = self.autostart;
            std::thread::spawn(move || {
                tray.update(|tray| tray.autostart = autostart);
            });
        }
    }
}

/// Gives back to the system the memory that drawing took, as far as the C library can.
fn trim_memory() {
    #[cfg(target_env = "gnu")]
    // SAFETY: it takes a number, and only rearranges what the allocator holds.
    unsafe {
        libc::malloc_trim(0);
    }
}
