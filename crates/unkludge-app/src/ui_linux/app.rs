//! The window's whole state and what follows from each thing that happens. The markup's callbacks come
//! here, one thread only, through [`with_app`].

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use unkludge_protocol::lighting::Rgb;
use unkludge_protocol::screen::Animation;
use ksni::blocking::Handle;
use slint::platform::{Key, WindowEvent};
use slint::{
    CloseRequestResponse, ComponentHandle, Global, Model, ModelRc, Timer, TimerMode, VecModel,
};

use super::tray::{self, Close, Start, Tray};
use super::{
    APP_ID, AppWindow, Banner, Choice, DialogKind, Swatch, View, dialogs, fit, resume, shot, view,
};
use crate::demo::Script;
use crate::drive::{Drained, MESSAGE_TIME, PROGRESS_PAINT_GAP, Parts};
use crate::i18n::t;
use crate::layout;
use crate::session::{Session, SystemAutostart, open_real_keyboard};
use crate::settings::Settings;
use crate::state::{Note, Slider, State, Tone};
use crate::worker::{Cmd, Event, Notify, WorkerHandle};
use crate::{log, sys};

mod modal;
mod preview;
mod wiring;

#[cfg(test)]
pub(super) use preview::wanted_preview_timer;
pub(super) use wiring::press;
#[cfg(test)]
pub(super) use wiring::wire as wire_for_tests;
use wiring::{app_icon, hook_window_events, raise, wire};

/// A window minimized at login because there was no tray yet goes into a tray that turns up this soon.
const TRAY_GRACE: Duration = Duration::from_secs(60);
/// Closing again this soon after minimizing means that it didn't work.
const MINIMIZE_GRACE: Duration = Duration::from_secs(2);
/// A new window doesn't always reach the window manager straight away.
const MINIMIZE_AGAIN_AFTER: Duration = Duration::from_millis(300);
/// How long to wait for the window to exist, and how many times.
const FIT_RETRY: Duration = Duration::from_millis(100);
const MAX_FIT_TRIES: u32 = 50;
/// How long a scripted demo waits to reach its state before giving up.
const SHOT_GIVE_UP: Duration = Duration::from_secs(90);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs something with the app, if there is one. What comes while the app is busy with something else
/// runs right after it, so it doesn't get lost.
pub fn with_app(f: impl FnOnce(&mut App) + 'static) {
    let waiting = APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => {
            if let Some(app) = app.as_mut() {
                f(app);
            }
            None
        }
        Err(_) => Some(f),
    });
    if let Some(f) = waiting {
        Timer::single_shot(Duration::ZERO, move || with_app(f));
    }
}

/// As [`with_app`], for from another thread.
pub fn on_ui(f: impl FnOnce(&mut App) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with_app(f));
}

/// What quitting leaves to do once the event loop has stopped: the worker gets to finish up.
pub fn finish() {
    let app = APP.with(|cell| cell.borrow_mut().take());
    if let Some(mut app) = app {
        app.save_settings();
        if let Some(worker) = app.worker.take() {
            worker.shutdown();
        }
    }
}

/// What the markup's lists are made from. They're kept, and only the rows that change are changed,
/// so that a list being set again doesn't take the focus from what's in it.
struct Rows {
    banners: Rc<VecModel<Banner>>,
    fits: Rc<VecModel<Choice>>,
    effects: Rc<VecModel<Choice>>,
    sleeps: Rc<VecModel<Choice>>,
    swatches: Rc<VecModel<Swatch>>,
}

impl Rows {
    fn new(window: &AppWindow) -> Rows {
        let rows = Rows {
            banners: Rc::default(),
            fits: Rc::default(),
            effects: Rc::default(),
            sleeps: Rc::default(),
            swatches: Rc::default(),
        };
        let view = View::get(window);
        view.set_banners(ModelRc::from(Rc::clone(&rows.banners)));
        view.set_fits(ModelRc::from(Rc::clone(&rows.fits)));
        view.set_effects(ModelRc::from(Rc::clone(&rows.effects)));
        view.set_sleeps(ModelRc::from(Rc::clone(&rows.sleeps)));
        view.set_swatches(ModelRc::from(Rc::clone(&rows.swatches)));
        rows
    }
}

/// Changes only the rows that differ.
fn sync_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (at, row) in rows.into_iter().enumerate() {
        if model.row_data(at).as_ref() != Some(&row) {
            model.set_row_data(at, row);
        }
    }
}

/// The colour picker's colour, as it's being chosen.
#[derive(Clone, Copy)]
struct ColorEdit {
    rgb: Rgb,
    warning: bool,
}

pub struct App {
    pub(super) window: AppWindow,
    pub(super) settings: Settings,
    pub(super) state: State,
    frame: layout::Frame,
    worker: Option<WorkerHandle>,
    pub(super) events: Option<Receiver<Event>>,
    /// A wake-up is on its way, so the worker needn't send another.
    woken: Arc<AtomicBool>,
    session: Session,
    script: Option<Script>,
    shot_scheduled: bool,
    autostart: bool,
    tray: Option<Handle<Tray>>,
    /// Waiting for the system's password dialog.
    granting: bool,
    /// Whether files can be dropped on the window. Not on Wayland, where winit doesn't pass them on.
    can_drop: bool,
    rows: Rows,
    /// When the animation being shown started.
    preview_since: Instant,
    /// What the preview's timer is set to, if it's running.
    preview_every: Option<Duration>,
    /// Which frame the window holds, so that a frame only gets decoded when it's another.
    preview_shown: Option<(Arc<Animation>, usize)>,
    preview_timer: Timer,
    message_timer: Timer,
    sleep_timer: Timer,
    /// When the last redraw that an upload's progress asked for was.
    last_refresh: Instant,
    /// Minimized at login for want of a tray, which may yet turn up.
    parked_at: Option<Instant>,
    minimized_at: Option<Instant>,
    /// A dialog of the window's own came from the keyboard, so the focus goes back afterwards.
    dialog_by_keyboard: bool,
    /// A file is being asked for, so another shouldn't be.
    choosing_file: bool,
    color: Option<ColorEdit>,
    /// How many times the window wasn't there yet to be fitted.
    fit_tries: u32,
    #[cfg(test)]
    pub(super) frames_decoded: usize,
    /// What would have gone to the worker.
    #[cfg(test)]
    pub(super) sent: RefCell<Vec<&'static str>>,
}

impl App {
    pub(super) fn new(window: AppWindow, settings: Settings, session: Session) -> App {
        let autostart = session.autostart_now(&SystemAutostart, false);
        let state = State::with_settings(&settings);
        let frame = layout::Frame::new(state.banners().len(), state.preview_height());
        let rows = Rows::new(&window);
        App {
            window,
            settings,
            state,
            frame,
            worker: None,
            events: None,
            woken: Arc::new(AtomicBool::new(false)),
            session,
            script: None,
            shot_scheduled: false,
            autostart,
            tray: None,
            granting: false,
            can_drop: true,
            rows,
            preview_since: Instant::now(),
            preview_every: None,
            preview_shown: None,
            preview_timer: Timer::default(),
            message_timer: Timer::default(),
            sleep_timer: Timer::default(),
            last_refresh: Instant::now(),
            parked_at: None,
            minimized_at: None,
            dialog_by_keyboard: false,
            choosing_file: false,
            color: None,
            fit_tries: 0,
            #[cfg(test)]
            frames_decoded: 0,
            #[cfg(test)]
            sent: RefCell::default(),
        }
    }

    /// Everything that comes before the event loop, in the order it has to.
    pub(super) fn start(
        window: AppWindow,
        settings: Settings,
        session: Session,
        start_hidden: bool,
    ) {
        let view = View::get(&window);
        view.set_t(view::labels());
        view.set_icon(app_icon());
        let scripted = session.is_scripted();
        let demo = session.is_demo();
        view.set_window_title(window_title(demo).into());
        wire(&window);
        let mut app = App::new(window.clone_strong(), settings, session);
        app.refresh();
        if !scripted {
            app.tray = tray::spawn(Tray {
                title: window_title(demo),
                id: if demo { "unkludge-demo" } else { APP_ID },
                autostart: app.autostart,
            });
        }
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        if scripted {
            with_app(App::start_scripted);
        } else {
            let weak = window.as_weak();
            hook_window_events(&window);
            if !demo {
                sys::on_show_request(|| on_ui(App::show_window));
            }
            with_app(move |app| app.start_shown(start_hidden));
            // The window itself only exists once the event loop is going.
            Timer::single_shot(Duration::ZERO, move || {
                if weak.upgrade().is_some() {
                    with_app(App::fit_window);
                }
            });
        }
    }

    fn start_shown(&mut self, start_hidden: bool) {
        self.exit_when_told();
        let start = tray::start(start_hidden, tray::is_online());
        if start != Start::Hidden {
            let _ = self.window.show();
        }
        if start == Start::Minimized {
            self.window.window().set_minimized(true);
            self.parked_at = Some(Instant::now());
            let weak = self.window.as_weak();
            Timer::single_shot(MINIMIZE_AGAIN_AFTER, move || {
                if let Some(window) = weak.upgrade() {
                    window.window().set_minimized(true);
                }
            });
        }
        if !tray::is_online() {
            self.state.message = Some(Note::new(t().linux.no_tray, Tone::Warning));
            self.refresh();
        }
        self.watch_for_sleep();
        // After showing, so that the first write to a keyboard, which is the clock, doesn't come
        // from a program that hasn't appeared yet.
        self.start_worker();
    }

    fn start_scripted(&mut self) {
        self.exit_when_told();
        self.fit_offscreen();
        let _ = self.window.show();
        Timer::single_shot(SHOT_GIVE_UP, || {
            log::error("the demo didn't reach its state, so there's no picture");
            with_app(App::quit);
        });
        self.start_worker();
    }

    fn exit_when_told(&self) {
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
            sys::data_dir(),
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

    pub(super) fn send(&self, cmd: Option<Cmd>) {
        #[cfg(test)]
        if let Some(cmd) = &cmd {
            self.sent.borrow_mut().push(super::tests::name_of(cmd));
        }
        if let (Some(worker), Some(cmd)) = (&self.worker, cmd) {
            worker.send(cmd);
        }
    }

    pub(super) fn save_settings(&self) {
        let dir = sys::data_dir();
        if let Err(error) = self.session.save_settings(&self.settings, dir.as_deref()) {
            log::error(&format!("the settings can't be saved: {error}"));
        }
    }

    pub(super) fn quit(&mut self) {
        if cfg!(debug_assertions) {
            let window = self.window.window();
            eprintln!(
                "ending at scale {}: {:?}",
                window.scale_factor(),
                window.size()
            );
        }
        self.save_settings();
        // It doesn't get waited for: the process is about to end, and the tray with it.
        if let Some(tray) = self.tray.take() {
            let _ = tray.shutdown();
        }
        let _ = slint::quit_event_loop();
    }

    // What the worker says.

    pub(super) fn drain_events(&mut self) {
        self.woken.store(false, Ordering::SeqCst);
        let had_keyboard = self.state.model().is_some();
        let drained = self.take_events();
        // What had the focus has gone out of sight, and Space would still find it.
        if had_keyboard && self.state.model().is_none() {
            self.window.invoke_drop_focus();
        }
        if drained.save_settings {
            self.save_settings();
        }
        for target in &drained.to_open {
            if let Err(error) = sys::open(target) {
                log::error(&format!("the report's folder can't be opened: {error}"));
            }
        }
        if drained.image_changed {
            self.restart_preview();
        }
        if let Some(fades) = drained.message_fades {
            self.time_message(fades);
        }
        if !drained.only_progress || self.last_refresh.elapsed() >= PROGRESS_PAINT_GAP {
            self.refresh();
        }
        if drained.upload_ended {
            self.window.invoke_upload_ended();
        }
        self.maybe_schedule_shot();
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

    fn time_message(&mut self, fades: bool) {
        if !fades {
            self.message_timer.stop();
            return;
        }
        self.message_timer
            .start(TimerMode::SingleShot, MESSAGE_TIME, || {
                with_app(|app| {
                    if app.state.message.as_ref().is_some_and(Note::fades) {
                        app.state.message = None;
                        app.refresh();
                    }
                });
            });
    }

    fn say(&mut self, text: &str, tone: Tone) {
        let note = Note::new(text, tone);
        let fades = note.fades();
        self.state.message = Some(note);
        self.time_message(fades);
        self.refresh();
    }

    /// Gives the window everything it shows. What hasn't changed doesn't get redrawn.
    pub(super) fn refresh(&mut self) {
        self.last_refresh = Instant::now();
        let frame = layout::Frame::new(self.state.banners().len(), self.state.preview_height());
        let size_changed = frame.height != self.frame.height;
        self.frame = frame;
        let view = View::get(&self.window);
        view.set_frame(view::frame(&self.state, &self.frame));
        view.set_header(view::header(&self.state, &self.session));
        view.set_screen(view::screen(&self.state, self.can_drop));
        view.set_lights(view::lights(&self.state));
        // A demo doesn't look at what's on the system.
        let rule_installed = !self.session.is_demo() && sys::udev_rule_installed();
        view.set_empty(view::empty(&self.state, self.granting, rule_installed));
        view.set_footer(view::footer(
            &self.state,
            &self.session,
            self.autostart,
            self.settings.auto_clock,
        ));
        sync_rows(&self.rows.banners, view::banners(&self.state, &self.frame));
        sync_rows(&self.rows.fits, view::fits(&self.state, &self.frame));
        sync_rows(&self.rows.effects, view::effects(&self.state, &self.frame));
        sync_rows(&self.rows.sleeps, view::sleeps(&self.state, &self.frame));
        sync_rows(
            &self.rows.swatches,
            view::swatches(&self.state, &self.settings),
        );
        self.update_preview();
        self.sync_preview_timer();
        if size_changed {
            self.fit_window();
        }
    }

    /// A picture has no screen, so it's the size a demo says the screen allows.
    fn fit_offscreen(&mut self) {
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

    /// The window's size for the layout, drawing smaller if the screen is too small for it.
    pub(super) fn fit_window(&mut self) {
        if self.session.is_scripted() {
            self.fit_offscreen();
            return;
        }
        let work_area = self.session.demo_args().and_then(|args| args.work_area);
        let fitted = fit::fit(&self.window, self.frame.height, work_area);
        // The window itself only exists once the event loop has got to it.
        if fitted.is_some() {
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

    // The window itself.

    fn is_showing(&self) -> bool {
        let window = self.window.window();
        window.is_visible() && !window.is_minimized()
    }

    pub(super) fn show_window(&mut self) {
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

    /// Closing the window hides it, and while hidden nothing gets drawn.
    fn hide_window(&mut self) {
        View::get(&self.window).set_dialog(DialogKind::None);
        let _ = self.window.hide();
        self.save_settings();
        self.restart_preview();
    }

    pub(super) fn close_requested(&mut self) -> CloseRequestResponse {
        let just_minimized = self
            .minimized_at
            .is_some_and(|at| at.elapsed() < MINIMIZE_GRACE);
        let note_up = View::get(&self.window).get_dialog() == DialogKind::Note;
        match tray::close(
            tray::is_online(),
            self.settings.tray_note_shown || note_up,
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
                self.window.window().set_minimized(true);
                self.minimized_at = Some(Instant::now());
                self.say(t().linux.no_tray, Tone::Warning);
                self.save_settings();
                CloseRequestResponse::KeepWindowShown
            }
        }
    }

    pub(super) fn tray_came_online(&mut self) {
        // It might have been saying there wasn't one.
        if self
            .state
            .message
            .as_ref()
            .is_some_and(|message| message.text == t().linux.no_tray)
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
    pub(super) fn tray_went_offline(&mut self) {
        if !self.window.window().is_visible() {
            let _ = self.window.show();
            self.window.window().set_minimized(true);
            self.say(t().linux.no_tray, Tone::Warning);
        }
    }

    /// Starts with the system or not. A demo only pretends.
    pub(super) fn toggle_autostart(&mut self) {
        self.autostart = self
            .session
            .set_autostart(!self.autostart, &SystemAutostart);
        self.update_tray();
        self.refresh();
    }

    pub(super) fn toggle_auto_clock(&mut self) {
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

    fn watch_for_sleep(&mut self) {
        let mut watch = resume::SleepWatch::default();
        watch.woke_up(resume::time_asleep());
        self.sleep_timer
            .start(TimerMode::Repeated, resume::CHECK_EVERY, move || {
                if watch.woke_up(resume::time_asleep()) {
                    with_app(|app| app.send(Some(Cmd::Resumed)));
                }
            });
    }

    // What the user does.

    pub(super) fn choose_path(&mut self, path: PathBuf) {
        let cmd = self.state.choose_file(path);
        self.send(cmd);
        self.restart_preview();
        self.refresh();
    }

    /// Asks for a file on a thread of its own, as that waits for the user.
    pub(super) fn ask_for_file(&mut self, by_keyboard: bool) {
        if std::mem::replace(&mut self.choosing_file, true) {
            return;
        }
        std::thread::spawn(move || {
            let picked = dialogs::choose_image();
            on_ui(move |app| app.file_picked(picked, by_keyboard));
        });
    }

    pub(super) fn file_picked(&mut self, picked: dialogs::Picked, by_keyboard: bool) {
        self.choosing_file = false;
        match picked {
            dialogs::Picked::File(path) => self.choose_path(path),
            dialogs::Picked::Cancelled => {}
            dialogs::Picked::NoProgram => self.open_dialog(DialogKind::Path, by_keyboard),
        }
    }

    pub(super) fn path_entered(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.close_dialog();
        self.choose_path(PathBuf::from(text));
    }

    pub(super) fn upload(&mut self, by_keyboard: bool) {
        let cmd = self.state.start_upload();
        let started = cmd.is_some();
        self.send(cmd);
        self.refresh();
        // Cancel is what's next, and Space mustn't start another.
        if started && by_keyboard {
            self.window.invoke_focus_cancel();
        }
    }

    pub(super) fn drag_slider(&mut self, slider: Slider, x: f32, x0: f32, x1: f32) {
        let cmd = self.state.drag_slider(slider, x, x0, x1);
        self.send(cmd);
        self.refresh();
    }

    pub(super) fn nudge(&mut self, slider: Slider, by: i32) {
        let cmd = self.state.nudge(slider, by);
        self.send(cmd);
        self.refresh();
    }

    /// Runs the system's password dialog on a thread of its own, as that waits for the user.
    pub(super) fn grant_access(&mut self) {
        if std::mem::replace(&mut self.granting, true) {
            return;
        }
        self.say(t().linux.granting, Tone::Neutral);
        let session = self.session.clone();
        std::thread::spawn(move || {
            let result = session.grant_access(sys::grant_access);
            on_ui(move |app| app.access_granted(result));
        });
    }

    pub(super) fn access_granted(&mut self, result: Result<(), sys::GrantError>) {
        self.granting = false;
        let tone = if result.is_ok() {
            Tone::Good
        } else {
            Tone::Bad
        };
        self.say(t().linux.grant_result(result), tone);
        // The worker looks every couple of seconds anyway, and needn't wait.
        if result.is_ok() {
            self.send(Some(Cmd::Refresh));
        }
    }

    // Scripted demos.

    fn maybe_schedule_shot(&mut self) {
        let Some(args) = self.session.demo_args() else {
            return;
        };
        let reached = self.script.as_ref().is_some_and(Script::reached);
        if args.shot.is_some() && !self.shot_scheduled && reached {
            self.shot_scheduled = true;
            Timer::single_shot(args.shot_delay, || with_app(App::take_shot));
        }
    }

    fn take_shot(&mut self) {
        self.refresh();
        let tabs = self.script.as_ref().map_or(0, Script::tabs_before_shot);
        for _ in 0..tabs {
            press(self.window.window(), Key::Tab);
        }
        // A pressed key's ring shows on the next look at what's dirty, which a picture is.
        let window = self.window.clone_strong();
        let path = self.session.demo_args().and_then(|args| args.shot.clone());
        if let Some(path) = path
            && let Err(error) = shot::save(window.window(), &path)
        {
            log::error(&format!("the picture can't be saved: {error}"));
        }
        self.quit();
    }
}

fn window_title(demo: bool) -> String {
    if demo {
        format!("Unkludge - {}", t().demo)
    } else {
        "Unkludge".to_string()
    }
}

#[cfg(test)]
pub(super) fn set_app(app: App) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

#[cfg(test)]
pub(super) fn take_app() -> Option<App> {
    APP.with(|cell| cell.borrow_mut().take())
}
