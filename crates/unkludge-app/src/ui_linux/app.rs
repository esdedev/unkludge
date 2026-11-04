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
use slint::platform::Key;
use slint::{ComponentHandle, Global, Model, ModelRc, Timer, TimerMode, VecModel};

use super::tray::Tray;
use super::{AppWindow, Banner, Choice, DialogKind, Swatch, View, dialogs, resume, shot, view};
use crate::demo::Script;
use crate::drive::{Drained, MESSAGE_TIME, PROGRESS_PAINT_GAP, Parts};
use crate::i18n::t;
use crate::layout;
use crate::session::{Session, SystemAutostart, folder_not_opened};
use crate::settings::Settings;
use crate::state::{Note, Slider, State, Tone};
use crate::worker::{Cmd, Event, WorkerHandle};
use crate::{log, sys};

mod modal;
mod preview;
mod start;
mod window;
mod wiring;

#[cfg(test)]
pub(super) use preview::wanted_preview_timer;
#[cfg(test)]
pub(super) use start::Outside;
pub(super) use start::RealOutside;
pub(super) use wiring::press;
#[cfg(test)]
pub(super) use wiring::wire as wire_for_tests;

/// Files dropped together come one at a time this close together.
const DROP_BURST: Duration = Duration::from_millis(500);

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
            if sys::end_asked() {
                worker.shutdown_within(crate::worker::SYSTEM_END_WAIT);
            } else {
                worker.shutdown();
            }
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
    /// Where settings and reports go. A test has its own.
    data_dir: Option<PathBuf>,
    script: Option<Script>,
    shot_scheduled: bool,
    autostart: bool,
    tray: Option<Handle<Tray>>,
    /// Whether there's a tray to stay in right now, which the tray's thread keeps up to date.
    pub(super) tray_online: Arc<AtomicBool>,
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
    sleep_watch: resume::SleepWatch,
    end_timer: Timer,
    /// For a move of the window, which ends some time after it stops saying so.
    refit_timer: Timer,
    /// When the last redraw that an upload's progress asked for was.
    last_refresh: Instant,
    /// Minimized at login for want of a tray, which may yet turn up.
    pub(super) parked_at: Option<Instant>,
    /// When it was last asked to minimize, for as long as that's still the news.
    pub(super) minimized_at: Option<Instant>,
    /// A dialog of the window's own came from the keyboard, so the focus goes back afterwards.
    dialog_by_keyboard: bool,
    /// A file is being asked for, so another shouldn't be.
    pub(super) choosing_file: bool,
    picker: dialogs::Picker,
    /// When a file was last dropped.
    last_drop: Option<Instant>,
    color: Option<ColorEdit>,
    /// How many times the window wasn't there yet to be fitted, or to be minimized.
    pub(super) fit_tries: u32,
    pub(super) size_refits: u32,
    minimize_tries: u32,
    #[cfg(test)]
    pub(super) frames_decoded: usize,
    /// What would have gone to the worker.
    #[cfg(test)]
    pub(super) sent: RefCell<Vec<&'static str>>,
    /// And the lighting changes among it.
    #[cfg(test)]
    pub(super) sent_lighting: RefCell<Vec<unkludge_protocol::lighting::LightingChange>>,
}

impl App {
    pub(super) fn new(
        window: AppWindow,
        settings: Settings,
        session: Session,
        data_dir: Option<PathBuf>,
    ) -> App {
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
            data_dir,
            script: None,
            shot_scheduled: false,
            autostart,
            tray: None,
            tray_online: Arc::new(AtomicBool::new(false)),
            granting: false,
            can_drop: true,
            rows,
            preview_since: Instant::now(),
            preview_every: None,
            preview_shown: None,
            preview_timer: Timer::default(),
            message_timer: Timer::default(),
            sleep_timer: Timer::default(),
            sleep_watch: resume::SleepWatch::default(),
            end_timer: Timer::default(),
            refit_timer: Timer::default(),
            last_refresh: Instant::now(),
            parked_at: None,
            minimized_at: None,
            dialog_by_keyboard: false,
            choosing_file: false,
            picker: dialogs::Picker::default(),
            last_drop: None,
            color: None,
            fit_tries: 0,
            size_refits: 0,
            minimize_tries: 0,
            #[cfg(test)]
            frames_decoded: 0,
            #[cfg(test)]
            sent: RefCell::default(),
            #[cfg(test)]
            sent_lighting: RefCell::default(),
        }
    }

    pub(super) fn send(&self, cmd: Option<Cmd>) {
        #[cfg(test)]
        if let Some(cmd) = &cmd {
            self.sent.borrow_mut().push(super::tests::name_of(cmd));
            if let Cmd::SetLighting(change) | Cmd::AdjustLighting(change) = cmd {
                self.sent_lighting.borrow_mut().push(*change);
            }
        }
        if let (Some(worker), Some(cmd)) = (&self.worker, cmd) {
            worker.send(cmd);
        }
    }

    pub(super) fn save_settings(&self) {
        if let Err(error) = self
            .session
            .save_settings(&self.settings, self.data_dir.as_deref())
        {
            log::error(&format!("the settings can't be saved: {error}"));
        }
    }

    pub(super) fn quit(&mut self) {
        if cfg!(debug_assertions) {
            let window = self.window.window();
            eprintln!(
                "ending at scale {}: {:?}, minimized {}",
                window.scale_factor(),
                window.size(),
                window.is_minimized()
            );
        }
        self.save_settings();
        // A dialog from another program would otherwise stay up with nothing to answer to.
        self.picker.stop();
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
        // What had the focus has gone out of sight, and Space would find it once it's back. A dialog
        // that's up has the focus, and needs it for Escape.
        let dialog = View::get(&self.window).get_dialog();
        if had_keyboard && self.state.model().is_none() && dialog == DialogKind::None {
            self.window.invoke_drop_focus();
        }
        if drained.save_settings {
            self.save_settings();
        }
        for target in &drained.to_open {
            if let Err(error) = sys::open(target) {
                self.state.message = Some(folder_not_opened(target, &error));
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
            data_dir: self.data_dir.clone(),
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
            self.size_refits = 0;
            self.fit_window();
        }
    }

    // What the user does.

    pub(super) fn choose_path(&mut self, path: PathBuf) {
        let cmd = self.state.choose_file(path);
        self.send(cmd);
        self.restart_preview();
        self.refresh();
    }

    /// A file dropped on the window. Of several at once the first counts, as on Windows, and nothing
    /// gets dropped through a dialog.
    pub(super) fn file_dropped(&mut self, path: PathBuf, now: Instant) {
        let same_drop = self
            .last_drop
            .is_some_and(|at| now.saturating_duration_since(at) < DROP_BURST);
        self.last_drop = Some(now);
        if !same_drop && !View::get(&self.window).get_modal() {
            self.choose_path(path);
        }
    }

    /// Nothing can be used while a program asks for a file, as with a dialog of the window's own.
    /// Says whether one should be asked for.
    pub(super) fn begin_choosing(&mut self) -> bool {
        if std::mem::replace(&mut self.choosing_file, true) {
            return false;
        }
        View::get(&self.window).set_picking(true);
        true
    }

    /// Asks for a file on a thread of its own, as that waits for the user.
    pub(super) fn ask_for_file(&mut self, by_keyboard: bool) {
        if !self.begin_choosing() {
            return;
        }
        let picker = self.picker.clone();
        std::thread::spawn(move || {
            let picked = picker.choose_image();
            on_ui(move |app| app.file_picked(picked, by_keyboard));
        });
    }

    pub(super) fn file_picked(&mut self, picked: dialogs::Picked, by_keyboard: bool) {
        self.choosing_file = false;
        View::get(&self.window).set_picking(false);
        if picked == dialogs::Picked::NoProgram {
            self.open_dialog(DialogKind::Path, by_keyboard);
            return;
        }
        if by_keyboard {
            self.window.invoke_focus_choose_file();
        }
        if let dialogs::Picked::File(path) = picked {
            self.choose_path(path);
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

#[cfg(test)]
pub(super) fn set_app(app: App) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

#[cfg(test)]
pub(super) fn take_app() -> Option<App> {
    APP.with(|cell| cell.borrow_mut().take())
}
