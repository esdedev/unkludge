//! Starting up, in the order it has to go. What it reaches outside the window comes in through
//! [`Outside`], so that tests can see what a demo leaves alone, and never start a real worker.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ksni::blocking::Handle;
use slint::{ComponentHandle, Global, Timer, TimerMode};

use super::wiring::{app_icon, hook_window_events, wire};
use super::{APP, App, on_ui, with_app};
use crate::i18n::t;
use crate::log;
use crate::session::open_real_keyboard;
use crate::settings::Settings;
use crate::state::{Note, Tone};
use crate::sys;
use crate::ui_linux::tray::{self, Start, Tray};
use crate::ui_linux::{APP_ID, AppWindow, View, resume, view};
use crate::worker::Notify;

/// When to look at the screen again after starting. An X server that has only just started can say
/// its screen is small for a moment, and there's no word of that changing.
const REFIT_AFTER_START: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];
/// How long a scripted demo waits to reach its state before giving up.
const SHOT_GIVE_UP: Duration = Duration::from_secs(90);
/// How soon a signal to end gets noticed. Part of the little time the system leaves goes on this.
const END_CHECK_EVERY: Duration = Duration::from_millis(100);

/// What starting up does outside the window.
pub(in crate::ui_linux) trait Outside {
    fn spawn_tray(&self, tray: Tray) -> Option<Handle<Tray>>;
    fn listen_for_other_copies(&self);
    fn hook_window(&self, window: &AppWindow);
    fn start_worker(&self, app: &mut App);
    fn data_dir(&self) -> Option<PathBuf>;
}

/// The real thing.
pub(in crate::ui_linux) struct RealOutside;

impl Outside for RealOutside {
    fn spawn_tray(&self, tray: Tray) -> Option<Handle<Tray>> {
        tray::spawn(tray)
    }

    fn listen_for_other_copies(&self) {
        sys::on_show_request(|| on_ui(App::show_window));
    }

    fn hook_window(&self, window: &AppWindow) {
        hook_window_events(window);
    }

    fn start_worker(&self, app: &mut App) {
        app.start_worker();
    }

    fn data_dir(&self) -> Option<PathBuf> {
        sys::data_dir()
    }
}

impl App {
    /// Everything that comes before the event loop.
    pub(in crate::ui_linux) fn start(
        window: AppWindow,
        settings: Settings,
        session: crate::session::Session,
        start_hidden: bool,
        outside: &dyn Outside,
    ) {
        let view = View::get(&window);
        view.set_t(view::labels());
        view.set_icon(app_icon());
        let scripted = session.is_scripted();
        let demo = session.is_demo();
        let listens = session.claims_single_instance();
        view.set_window_title(window_title(demo).into());
        wire(&window);
        let mut app = App::new(window.clone_strong(), settings, session, outside.data_dir());
        app.refresh();
        if !scripted {
            app.tray = outside.spawn_tray(Tray {
                title: window_title(demo),
                id: if demo { "unkludge-demo" } else { APP_ID },
                autostart: app.autostart,
                online: Arc::clone(&app.tray_online),
            });
            outside.hook_window(&window);
            if listens {
                outside.listen_for_other_copies();
            }
        }
        if scripted {
            app.start_scripted(outside);
        } else {
            app.start_shown(start_hidden, outside);
            // The window itself only exists once the event loop is going.
            Timer::single_shot(Duration::ZERO, || with_app(App::fit_window));
            for after in REFIT_AFTER_START {
                Timer::single_shot(after, || with_app(App::fit_window));
            }
        }
        APP.with(|cell| *cell.borrow_mut() = Some(app));
    }

    fn start_shown(&mut self, start_hidden: bool, outside: &dyn Outside) {
        self.exit_when_told();
        let online = self.tray_online.load(Ordering::Relaxed);
        let start = tray::start(start_hidden, online);
        if start != Start::Hidden {
            let _ = self.window.show();
        }
        if start == Start::Minimized {
            self.park();
        }
        if !online {
            self.state.message = Some(Note::new(t().linux.no_tray, Tone::Warning));
            self.refresh();
        }
        self.watch_for_sleep();
        // The window only appears once the event loop is going, and what the worker says waits for
        // that too. Its first write to a keyboard, which is the clock, can come before that.
        outside.start_worker(self);
    }

    fn start_scripted(&mut self, outside: &dyn Outside) {
        self.exit_when_told();
        self.fit_offscreen();
        let _ = self.window.show();
        Timer::single_shot(SHOT_GIVE_UP, || {
            log::error("the demo didn't reach its state, so there's no picture");
            with_app(App::quit);
        });
        outside.start_worker(self);
    }

    /// Asked by a demo's arguments, or by the system.
    fn exit_when_told(&mut self) {
        self.watch_for_end();
        if let Some(after) = self.session.demo_args().and_then(|args| args.exit_after) {
            Timer::single_shot(after, || with_app(App::quit));
        }
    }

    /// Starts the worker, which starts looking for the keyboard.
    pub(super) fn start_worker(&mut self) {
        let notify: Notify = {
            let woken = Arc::clone(&self.woken);
            Arc::new(move || {
                if !woken.swap(true, Ordering::SeqCst) {
                    on_ui(App::drain_events);
                }
            })
        };
        let started = self.session.spawn_worker(
            &self.settings,
            self.data_dir.clone(),
            notify,
            open_real_keyboard,
            Box::new(sys::rk_app_running),
        );
        match started {
            Ok(running) => {
                self.worker = Some(running.worker);
                self.events = Some(running.events);
                self.script = running.script;
            }
            Err(problem) => log::error(&problem),
        }
    }

    fn watch_for_end(&mut self) {
        self.end_timer
            .start(TimerMode::Repeated, END_CHECK_EVERY, || {
                with_app(|app| app.look_for_end(sys::end_asked()));
            });
    }

    /// Takes whether the system has asked the program to end, which goes as any quitting does: the
    /// worker gets to cancel an upload, once the event loop has stopped.
    pub(in crate::ui_linux) fn look_for_end(&mut self, asked: bool) {
        if asked {
            self.end_timer.stop();
            self.quit();
        }
    }

    fn watch_for_sleep(&mut self) {
        self.sleep_watch.woke_up(resume::time_asleep());
        self.sleep_timer
            .start(TimerMode::Repeated, resume::CHECK_EVERY, || {
                with_app(|app| app.look_for_wake_up(resume::time_asleep()));
            });
    }

    /// Takes how long the computer has been asleep, and if that's more than before, the clock is due.
    pub(in crate::ui_linux) fn look_for_wake_up(&mut self, asleep: Option<Duration>) {
        if self.sleep_watch.woke_up(asleep) {
            self.send(Some(crate::worker::Cmd::Resumed));
        }
    }

    #[cfg(test)]
    pub(in crate::ui_linux) fn watching_for_end(&self) -> bool {
        self.end_timer.running()
    }

    #[cfg(test)]
    pub(in crate::ui_linux) fn watching_for_sleep(&self) -> bool {
        self.sleep_timer.running()
    }
}

fn window_title(demo: bool) -> String {
    if demo {
        format!("Unkludge - {}", t().demo)
    } else {
        "Unkludge".to_string()
    }
}
