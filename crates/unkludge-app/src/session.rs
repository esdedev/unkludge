//! Whether this run is the real app or a demo, and everything that differs between the two. It's all
//! here, away from any window, because getting one of these wrong means a demo writing to a real
//! keyboard or changing what starts with the system. What touches the system comes in as an argument,
//! so that tests can see it isn't touched.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use unkludge_protocol::device::{HidCollection, list_collections};
use unkludge_protocol::model;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};
use unkludge_shared::device_lock;

use crate::args::DemoArgs;
use crate::demo::{DemoKeyboard, Script};
use crate::i18n::t;
use crate::settings::{self, Settings};
use crate::state::{Note, Tone};
use crate::worker::{self, Event, Notify, Options, ReportData, WorkerHandle};
use crate::{log, report, sys};

/// A demo's settings are its own, next to the real ones.
const DEMO_SETTINGS: &str = "settings-demo.txt";

/// The system's entry for starting with it.
pub trait Autostart {
    fn set(&self, on: bool);
    fn enabled(&self) -> bool;
    /// See [`sys::refresh_autostart`].
    fn refresh(&self, force: bool);
}

/// The real entry.
pub struct SystemAutostart;

impl Autostart for SystemAutostart {
    fn set(&self, on: bool) {
        sys::set_autostart(on);
    }

    fn enabled(&self) -> bool {
        sys::autostart_enabled()
    }

    fn refresh(&self, force: bool) {
        sys::refresh_autostart(force);
    }
}

/// The worker, once it's running.
pub struct Running {
    pub worker: WorkerHandle,
    pub events: Receiver<Event>,
    /// What takes a demo to the state asked for.
    pub script: Option<Script>,
}

/// What came of a report: what to say, and what to open.
#[derive(Debug, PartialEq, Eq)]
pub struct ReportOutcome {
    pub note: Note,
    /// Folders and pages. Opening them can take a while, so that's up to the caller.
    pub open: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Session {
    Real,
    Demo(DemoArgs),
}

impl Session {
    pub fn new(demo: Option<DemoArgs>) -> Session {
        demo.map_or(Session::Real, Session::Demo)
    }

    pub fn is_demo(&self) -> bool {
        matches!(self, Session::Demo(_))
    }

    pub fn demo_args(&self) -> Option<&DemoArgs> {
        match self {
            Session::Real => None,
            Session::Demo(args) => Some(args),
        }
    }

    /// A scripted demo takes its picture and goes, and leaves nothing open behind it.
    pub fn is_scripted(&self) -> bool {
        self.demo_args().is_some_and(|args| args.shot.is_some())
    }

    /// Whether only one copy of this may run. A demo keeps out of the real copy's way.
    pub fn claims_single_instance(&self) -> bool {
        !self.is_demo()
    }

    /// Whether finding another copy running should bring up its window. A copy started with the
    /// system is only making sure that one is running.
    pub fn shows_other_copy(start_hidden: bool) -> bool {
        !start_hidden
    }

    /// The settings file's name in the data folder.
    pub fn settings_file(&self) -> &'static str {
        match self {
            Session::Real => settings::FILE_NAME,
            Session::Demo(_) => DEMO_SETTINGS,
        }
    }

    /// The settings, and on the real app what a launch does about starting with the system.
    /// `installed` says whether a first launch should make it start with the system.
    pub fn load_settings(
        &self,
        data_dir: Option<&Path>,
        autostart: &dyn Autostart,
        installed: bool,
    ) -> Settings {
        let file = data_dir.map(|dir| dir.join(self.settings_file()));
        let settings = Settings::load_at(file.as_deref());
        if self.is_demo() {
            return settings;
        }
        if settings.existed {
            // If the entry points at a copy that's gone, points it at this one.
            autostart.refresh(false);
            return settings;
        }
        if installed {
            autostart.set(true);
        }
        if let Err(error) = self.save_settings(&settings, data_dir) {
            log::error(&format!("the settings can't be saved: {error}"));
        }
        settings
    }

    pub fn save_settings(&self, settings: &Settings, data_dir: Option<&Path>) -> io::Result<()> {
        let dir = data_dir.ok_or(io::ErrorKind::NotFound)?;
        settings.save_to(&dir.join(self.settings_file()))
    }

    /// Whether the app starts with the system, which is the entry's to say. For a demo that's what
    /// it last pretended.
    pub fn autostart_now(&self, autostart: &dyn Autostart, pretended: bool) -> bool {
        if self.is_demo() {
            pretended
        } else {
            autostart.enabled()
        }
    }

    /// Asks to start with the system or not, and says how that came out.
    pub fn set_autostart(&self, on: bool, autostart: &dyn Autostart) -> bool {
        if self.is_demo() {
            return on;
        }
        autostart.set(on);
        autostart.enabled()
    }

    /// Saves a report. A demo's is a file of its own, and it doesn't look at the system's devices.
    pub fn save_report(&self, data: &ReportData, data_dir: Option<&Path>) -> io::Result<PathBuf> {
        let dir = data_dir.ok_or(io::ErrorKind::NotFound)?;
        if self.is_demo() {
            report::save_demo(data, dir)
        } else {
            report::save(data, dir)
        }
    }

    /// Saves a report and says what comes next. `FORM_URL` is `None` until the project is public, so
    /// until then showing the file is all there is to do.
    pub fn report_ready(&self, data: &ReportData, data_dir: Option<&Path>) -> ReportOutcome {
        match self.save_report(data, data_dir) {
            Ok(path) => {
                let mut open = Vec::new();
                if !self.is_scripted() {
                    open.push(path.parent().unwrap_or(&path).to_path_buf());
                    open.extend(report::FORM_URL.map(PathBuf::from));
                }
                ReportOutcome {
                    note: Note::new(t().report_saved, Tone::Neutral),
                    open,
                }
            }
            Err(error) => ReportOutcome {
                note: Note::new(format!("{}: {error}", t().report_failed), Tone::Bad),
                open: Vec::new(),
            },
        }
    }

    /// Installs what gives this system's users access to keyboards. A demo only pretends, as it asks
    /// for a password and changes the system.
    #[cfg(target_os = "linux")]
    pub fn grant_access(
        &self,
        install: impl FnOnce() -> Result<(), sys::GrantError>,
    ) -> Result<(), sys::GrantError> {
        if self.is_demo() { Ok(()) } else { install() }
    }

    /// Starts the worker. `open_real` is the only way to a real keyboard, and only the real app
    /// uses it. `data_dir` is where a demo keeps its picture for as long as it runs.
    pub fn spawn_worker<T, F>(
        &self,
        settings: &Settings,
        data_dir: Option<PathBuf>,
        notify: Notify,
        open_real: F,
        rk_app_running: Box<dyn FnMut() -> bool + Send>,
    ) -> Result<Running, String>
    where
        T: Transport + 'static,
        F: FnMut() -> Result<Device<T>, Error> + Send + 'static,
    {
        let options = |rk_app_running| Options {
            auto_clock: settings.auto_clock,
            palettes: settings.palettes.clone(),
            rk_app_running,
        };
        match self {
            Session::Real => {
                let (worker, events) = worker::spawn(open_real, options(rk_app_running), notify);
                Ok(Running {
                    worker,
                    events,
                    script: None,
                })
            }
            Session::Demo(args) => {
                let model = model::find(args.pid)
                    .ok_or_else(|| format!("no model {:04X} for the demo", args.pid))?;
                let keyboard = DemoKeyboard::new(model, args.state);
                let looking = keyboard.clone();
                let rk_app = keyboard.clone();
                let (worker, events) = worker::spawn(
                    move || looking.open(),
                    options(Box::new(move || rk_app.rk_app_running())),
                    notify,
                );
                Ok(Running {
                    worker,
                    events,
                    script: Some(Script::new(args.state, keyboard, data_dir)),
                })
            }
        }
    }
}

/// What the real app opens: the first supported keyboard. When there's none, a device of the same
/// vendor that this version can't use is worth saying so about. It's only read from the system's
/// list, and never opened. The device keeps the lock for as long as it's open, and if a `unkludge`
/// command has it now, that gets said and the worker looks again in a moment.
pub fn open_real_keyboard() -> Result<Device, Error> {
    match device_lock::open_locked(Duration::ZERO, || Device::open(None)) {
        Err(Error::NotFound) => {
            let listed = list_collections().unwrap_or_default();
            Err(unsupported_device(&listed).map_or(Error::NotFound, Error::Unsupported))
        }
        other => other,
    }
}

/// The first listed device that this version can't use.
pub fn unsupported_device(listed: &[HidCollection]) -> Option<u16> {
    listed
        .iter()
        .map(|collection| collection.pid)
        .find(|&pid| !model::find(pid).is_some_and(model::Model::is_supported))
}

#[cfg(test)]
mod tests;
