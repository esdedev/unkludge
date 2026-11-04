//! The thread that owns the keyboard. The interface sends it [`Cmd`]s and gets [`Event`]s back, and
//! [`Notify`] wakes the interface up to read them, so the interface never waits for the keyboard.
//!
//! [`Worker::step`] does everything that's due at a given time and says when it wants to run again.
//! The thread only calls it in a loop, so tests can drive it with a clock of their own. Where the
//! device comes from is up to a factory, which is also how a demo build gets a keyboard that doesn't
//! exist.
//!
//! **During an upload** the worker is inside the upload. It sees commands between steps, and those
//! that come during the upload's last pauses right after it, all under these rules:
//! - A lighting change, or another upload, is refused with [`Problem::Busy`], and nothing about it
//!   stays behind. An upload can take many minutes, so a change that took effect when it ended would
//!   come as a surprise, and whether a keyboard takes config writes during an upload is unknown. The
//!   interface should disable those controls between [`Event::UploadStarted`] and the end.
//! - The clock waits, as the official app does while it's busy, and is set once after the upload
//!   ends in any way, if the keyboard is still there. [`Cmd::SyncClock`] always counts. For
//!   [`Cmd::Resumed`] it depends on the automatic clock still being on by then.
//! - [`Cmd::LoadImage`] and [`Cmd::Report`] work, neither of them touching the keyboard.
//! - [`Cmd::Refresh`] does nothing, as the next look at the keyboard is close after the upload.
//!
//! **Cancelling** goes straight from [`WorkerHandle::send`] to a flag that the upload reads, and
//! [`Event::Cancelling`] goes out at once. A stalled keyboard can take up to 16 s to get through the
//! ack waits that are left. Every `Cancelling` is followed by exactly one end event:
//! [`Event::UploadCancelled`], or [`Event::UploadDone`] or [`Event::UploadFailed`] if the upload got
//! to its end first. An upload that hasn't started by then doesn't start, and gets `UploadCancelled`.
//!
//! **A keyboard whose config block can't be used** stays connected, as it's there, but nothing is
//! written to it: no lighting, no clock, no upload. Each command says [`Problem::UnusableConfig`], and
//! a clock that was due is set if a later read comes back good.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use unkludge_protocol::clock::set_clock;
use unkludge_protocol::config::Config;
use unkludge_protocol::lighting::{self, CurrentLighting, LightingChange, Palettes};
use unkludge_protocol::model::Model;
use unkludge_protocol::screen::{Animation, Fit, UploadPlan};
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};
use unkludge_shared::local_time::device_now;

mod loader;
#[cfg(test)]
mod tests;
mod timing;
mod upload;

use loader::{LoadRequest, Loader, decode_image};
use timing::{Beat, ChunkTime, ChunkTimer, WatchTimes, watch};

/// How often to look for a keyboard when there is none.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
/// How often to read a connected keyboard's settings, which also notices it going away.
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// How long a slider has to stay still before its value goes to the keyboard.
const SETTLE_TIME: Duration = Duration::from_millis(300);
/// How many errors a report can list.
const MAX_ERRORS: usize = 20;

/// How long things take when nothing is being tested.
const TIMINGS: Timings = Timings {
    watch: WatchTimes {
        hung_after: Duration::from_secs(30),
        check_every: Duration::from_secs(1),
    },
    shutdown_wait: Duration::from_secs(2),
};

#[derive(Clone, Copy)]
struct Timings {
    watch: WatchTimes,
    /// How long [`WorkerHandle::shutdown`] gives an upload to abort.
    shutdown_wait: Duration,
}

/// Wakes the interface so that it reads the waiting events. Must not block.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// Where the worker gets the time, so that a test can have it pass differently.
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What the interface can ask for.
pub enum Cmd {
    /// Changes settings now: a choice of effect, colour or sleep time.
    SetLighting(LightingChange),
    /// Changes settings once `SETTLE_TIME` passes with no other such command: for a slider being
    /// dragged. Fields given in a burst add up, the latest value of each one winning.
    AdjustLighting(LightingChange),
    /// Sets the clock now, or after the upload if one is running.
    SyncClock,
    /// The computer came back from sleep: looks at the keyboard now, and if it's still there sets
    /// the clock, as long as the automatic clock is on.
    Resumed,
    /// Decodes a file for the connected keyboard's screen, on a thread of its own. Ends in
    /// [`Event::ImageLoaded`] or [`Event::ImageFailed`], and a later request replaces an earlier one,
    /// which then tells nothing, even if the later one is refused with [`Problem::NoKeyboard`].
    /// Change of fit or delay means loading again. Frames past the model's limit are cut, which
    /// [`Animation::truncated`] says.
    LoadImage {
        path: PathBuf,
        fit: Fit,
        delay: Option<u16>,
    },
    /// Sends an image from [`Event::ImageLoaded`] to the screen.
    UploadScreen(Arc<Animation>),
    CancelUpload,
    /// Reads the keyboard's settings now and tells them, or looks for a keyboard now.
    Refresh,
    SetAutoClock(bool),
    /// Gathers what a report needs, which comes back in [`Event::ReportReady`]. Only reads.
    Report,
}

/// Why a command didn't work, for the interface to put into words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Not possible during an upload.
    Busy,
    /// Needs a keyboard, and none is connected.
    NoKeyboard,
    /// Would write to a keyboard whose config block can't be used, which isn't done.
    UnusableConfig,
    Device(Error),
}

/// An image decoded for the connected keyboard, as it would be sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedImage {
    pub path: PathBuf,
    /// The frames at the screen's size. [`rgb_from_565`] turns one into pixels to show.
    pub animation: Arc<Animation>,
    /// What an upload would send. It can have fewer frames than the animation.
    pub plan: UploadPlan,
    /// How long uploading it may take: as [`Event::UploadStarted`] would say now.
    pub estimate: Duration,
}

/// What the keyboard side knows for a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportData {
    pub model: Option<&'static Model>,
    /// The block, or why it isn't here. In English, like the rest of a report.
    pub config: Result<Vec<u8>, String>,
    /// Device errors since the app started, oldest first and in English.
    pub errors: Vec<String>,
}

/// What happened. The worker sends each of these once, when it happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Connected {
        model: &'static Model,
    },
    /// There is no keyboard. Also the first thing told if there's none to begin with.
    Disconnected,
    /// A keyboard seems to be there, but this error keeps it from being used. On Linux that's
    /// usually a missing udev rule. Told once per error.
    CannotOpen(Error),
    Lighting(CurrentLighting),
    /// To be saved. The worker keeps its own copy of the palettes it was given.
    PalettesChanged {
        pid: u16,
        palettes: Box<Palettes>,
    },
    ImageLoaded(LoadedImage),
    ImageFailed(Error),
    /// An upload began. It ends in `UploadDone`, `UploadCancelled` or `UploadFailed`.
    UploadStarted {
        /// The steps, as in [`Event::Progress`].
        total: usize,
        /// The steps times the time a chunk takes, measured so far or else assumed.
        estimate: Duration,
    },
    Progress {
        done: usize,
        total: usize,
        /// The steps left times the time a chunk takes, measured so far or else assumed.
        remaining: Duration,
    },
    /// A cancel was asked for. Tell the user, as it can take a while. Exactly one of `UploadDone`,
    /// `UploadCancelled` and `UploadFailed` follows.
    Cancelling,
    UploadDone,
    /// The keyboard took every frame but didn't acknowledge the finish, so it may not show the new
    /// animation. `UploadDone` follows.
    UploadUnconfirmed,
    UploadCancelled,
    UploadFailed(Error),
    ClockSynced,
    Error(Problem),
    /// Told at the start and whenever it changes.
    RkAppRunning(bool),
    /// Something the keyboard should have answered has taken over 30 s, and the worker can't do
    /// anything else until it does. Told once, until `Responding`.
    Hung,
    /// After `Hung`, things have moved again.
    Responding,
    ReportReady(ReportData),
}

/// What the worker starts with.
pub struct Options {
    pub auto_clock: bool,
    /// By product ID. A model that isn't here starts from the default palettes.
    pub palettes: BTreeMap<u16, Palettes>,
    pub rk_app_running: Box<dyn FnMut() -> bool + Send>,
}

#[derive(Clone)]
struct Emitter {
    events: Sender<Event>,
    notify: Notify,
}

impl Emitter {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        (self.notify)();
    }
}

/// What the interface's thread and the worker's share about an upload.
#[derive(Default)]
struct Shared {
    /// Its lock also keeps a cancel and the end of an upload from crossing, so that a `Cancelling`
    /// is always told before the end event.
    uploading: Mutex<bool>,
    cancel: AtomicBool,
}

impl Shared {
    /// Lets an upload that's been asked for be cancelled even before the worker gets to it.
    fn begin_upload(&self) {
        *self
            .uploading
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = true;
    }

    fn request_cancel(&self, emitter: &Emitter) {
        let uploading = self
            .uploading
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *uploading && !self.cancel.swap(true, Ordering::SeqCst) {
            emitter.emit(Event::Cancelling);
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Returns whether a cancel had been asked for.
    fn end_upload(&self) -> bool {
        let mut uploading = self
            .uploading
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *uploading = false;
        self.cancel.swap(false, Ordering::SeqCst)
    }
}

/// The interface's end of the worker.
pub struct WorkerHandle {
    cmds: Sender<Cmd>,
    shared: Arc<Shared>,
    emitter: Emitter,
    thread: Option<JoinHandle<()>>,
    beat: Arc<Beat>,
    shutdown_wait: Duration,
}

impl WorkerHandle {
    /// Never waits for the keyboard.
    pub fn send(&self, cmd: Cmd) {
        match cmd {
            Cmd::CancelUpload => self.shared.request_cancel(&self.emitter),
            cmd => {
                if matches!(cmd, Cmd::UploadScreen(_)) {
                    self.shared.begin_upload();
                }
                let _ = self.cmds.send(cmd);
            }
        }
    }

    /// Stops the worker, for quitting. A slider that hasn't settled gets written. An upload gets a
    /// moment to abort, so that the keyboard isn't left waiting for the rest of it, but a stalled
    /// keyboard doesn't hold up quitting.
    pub fn shutdown(self) {
        self.shared.request_cancel(&self.emitter);
        let WorkerHandle {
            cmds,
            thread,
            beat,
            shutdown_wait,
            ..
        } = self;
        drop(cmds);
        beat.close();
        let Some(thread) = thread else { return };
        let deadline = Instant::now() + shutdown_wait;
        while !thread.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Starts the worker's thread. `open` is called on that thread when it's looking for a keyboard:
/// [`Device::open`] for the real thing.
pub fn spawn<T, F>(open: F, options: Options, notify: Notify) -> (WorkerHandle, Receiver<Event>)
where
    T: Transport + 'static,
    F: FnMut() -> Result<Device<T>, Error> + Send + 'static,
{
    spawn_with(open, options, notify, TIMINGS)
}

fn spawn_with<T, F>(
    open: F,
    options: Options,
    notify: Notify,
    timings: Timings,
) -> (WorkerHandle, Receiver<Event>)
where
    T: Transport + 'static,
    F: FnMut() -> Result<Device<T>, Error> + Send + 'static,
{
    let (cmd_tx, cmd_rx) = channel();
    let (event_tx, event_rx) = channel();
    let emitter = Emitter {
        events: event_tx,
        notify,
    };
    let shared = Arc::<Shared>::default();
    let beat = Arc::<Beat>::default();
    let thread = {
        let (emitter, shared, beat) = (emitter.clone(), Arc::clone(&shared), Arc::clone(&beat));
        thread::spawn(move || {
            let mut worker = Worker::new(open, cmd_rx, options, emitter, shared);
            worker.beat = beat;
            worker.run();
        })
    };
    {
        let (emitter, beat) = (emitter.clone(), Arc::clone(&beat));
        thread::spawn(move || watch(&beat, &emitter, timings.watch));
    }
    let handle = WorkerHandle {
        cmds: cmd_tx,
        shared,
        emitter,
        thread: Some(thread),
        beat,
        shutdown_wait: timings.shutdown_wait,
    };
    (handle, event_rx)
}

/// What's known about the keyboard, so that each change gets told once.
#[derive(PartialEq)]
enum Link {
    Unknown,
    Absent,
    Blocked(Error),
    Connected,
}

fn estimate(steps: usize, chunk_time: Duration) -> Duration {
    chunk_time.saturating_mul(u32::try_from(steps).unwrap_or(u32::MAX))
}

/// The keyboard's thread, one [`Worker::step`] at a time.
pub struct Worker<T: Transport, F> {
    open: F,
    cmds: Receiver<Cmd>,
    emitter: Emitter,
    shared: Arc<Shared>,
    rk_app_running: Box<dyn FnMut() -> bool + Send>,
    auto_clock: bool,
    palettes: BTreeMap<u16, Palettes>,

    device: Option<Device<T>>,
    /// The connected keyboard's model. Stays while an upload has the device.
    model: Option<&'static Model>,
    link: Link,
    lighting: Option<CurrentLighting>,
    rk_app: Option<bool>,
    /// When to look at the keyboard next. `None` means now.
    next_poll: Option<Instant>,
    /// What a slider is waiting to write, and when it last moved.
    adjusting: Option<(LightingChange, Instant)>,
    /// [`Cmd::SyncClock`] came during an upload.
    clock_asked: bool,
    /// The clock would be set, if the setting is on by then, once the keyboard is confirmed there or
    /// an upload has ended. Connecting always sets this, so losing the keyboard needn't clear it.
    auto_clock_due: bool,
    /// Why the keyboard's config block can't be used, if so. Nothing gets written while this is set,
    /// and it keeps a failing read from telling the same thing every few seconds.
    bad_config: Option<Error>,
    errors: VecDeque<String>,
    chunk_time: Arc<ChunkTime>,
    loader: Loader,
    beat: Arc<Beat>,
    clock: Clock,
}

impl<T: Transport + 'static, F: FnMut() -> Result<Device<T>, Error>> Worker<T, F> {
    fn new(
        open: F,
        cmds: Receiver<Cmd>,
        options: Options,
        emitter: Emitter,
        shared: Arc<Shared>,
    ) -> Self {
        Worker {
            open,
            cmds,
            loader: Loader::new(emitter.clone(), Arc::new(decode_image)),
            emitter,
            shared,
            rk_app_running: options.rk_app_running,
            auto_clock: options.auto_clock,
            palettes: options.palettes,
            device: None,
            model: None,
            link: Link::Unknown,
            lighting: None,
            rk_app: None,
            next_poll: None,
            adjusting: None,
            clock_asked: false,
            auto_clock_due: false,
            bad_config: None,
            errors: VecDeque::new(),
            chunk_time: Arc::default(),
            beat: Arc::default(),
            clock: Arc::new(Instant::now),
        }
    }

    /// Ends when the handle is gone, after writing what a slider was still waiting to write.
    fn run(mut self) {
        loop {
            self.beat.busy();
            let wait = self.step((self.clock)());
            self.beat.idle();
            match self.cmds.recv_timeout(wait) {
                Ok(cmd) => {
                    self.beat.busy();
                    self.handle(cmd, (self.clock)());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.beat.busy();
        self.flush_adjustment((self.clock)());
        self.beat.close();
    }

    /// Runs the commands waiting, then whatever is due at `now`. Returns how long until something
    /// else is due, unless a command comes first.
    pub fn step(&mut self, now: Instant) -> Duration {
        while let Ok(cmd) = self.cmds.try_recv() {
            self.handle(cmd, now);
        }
        if self
            .adjusting
            .is_some_and(|(_, moved)| now >= moved + SETTLE_TIME)
        {
            self.flush_adjustment(now);
        }
        // A read while a slider is moving could put it back where it was.
        if self.adjusting.is_none() && self.next_poll.is_none_or(|at| now >= at) {
            self.poll(now);
        }
        let due = match self.adjusting {
            Some((_, moved)) => moved + SETTLE_TIME,
            None => self.next_poll.unwrap_or(now),
        };
        due.saturating_duration_since(now)
    }

    fn handle(&mut self, cmd: Cmd, now: Instant) {
        match cmd {
            Cmd::SetLighting(change) => {
                // First, so that it goes to the effect it was meant for.
                self.flush_adjustment(now);
                self.apply_lighting(&change, now);
            }
            Cmd::AdjustLighting(change) => self.adjust(change, now),
            Cmd::SyncClock => self.sync_clock(now),
            Cmd::Resumed => {
                self.auto_clock_due = true;
                self.next_poll = None;
            }
            Cmd::LoadImage { path, fit, delay } => self.load_image(path, fit, delay),
            Cmd::UploadScreen(animation) => self.upload(&animation, now),
            Cmd::CancelUpload => self.shared.request_cancel(&self.emitter),
            Cmd::Refresh => {
                self.flush_adjustment(now);
                if self.device.is_some() {
                    self.refresh_now(now);
                } else {
                    self.next_poll = None;
                }
            }
            Cmd::SetAutoClock(on) => self.auto_clock = on,
            Cmd::Report => self.report(now),
        }
    }

    fn emit(&self, event: Event) {
        self.emitter.emit(event);
    }

    fn record(&mut self, error: &Error) {
        if self.errors.len() == MAX_ERRORS {
            self.errors.pop_front();
        }
        self.errors.push_back(error.to_string());
    }

    fn problem(&mut self, problem: Problem) {
        if let Problem::Device(error) = &problem {
            self.record(&error.clone());
        }
        self.emit(Event::Error(problem));
    }

    /// A command couldn't do its work on the keyboard.
    fn command_failed(&mut self, error: Error, now: Instant) {
        let gone = matches!(error, Error::Hid(_));
        self.problem(Problem::Device(error));
        if gone {
            self.lose_device(now);
        }
    }

    /// When to look at the keyboard next, counting from something just done to it.
    fn schedule_poll(&mut self, now: Instant) {
        let interval = if self.device.is_some() {
            REFRESH_INTERVAL
        } else {
            RECONNECT_INTERVAL
        };
        self.next_poll = Some(now + interval);
    }

    fn set_link(&mut self, link: Link) {
        if self.link == link {
            return;
        }
        match &link {
            Link::Absent => self.emit(Event::Disconnected),
            Link::Blocked(error) => {
                self.record(error);
                self.emit(Event::CannotOpen(error.clone()));
            }
            Link::Unknown | Link::Connected => {}
        }
        self.link = link;
    }

    fn lose_device(&mut self, now: Instant) {
        self.device = None;
        self.model = None;
        self.lighting = None;
        self.adjusting = None;
        self.clock_asked = false;
        self.set_link(Link::Absent);
        self.schedule_poll(now);
    }

    fn poll(&mut self, now: Instant) {
        let running = (self.rk_app_running)();
        if self.rk_app != Some(running) {
            self.rk_app = Some(running);
            self.emit(Event::RkAppRunning(running));
        }
        if self.device.is_some() {
            self.refresh_quietly(now);
        } else {
            self.connect(now);
        }
    }

    fn connect(&mut self, now: Instant) {
        match (self.open)() {
            Ok(mut device) => {
                let (chunk_time, clock, beat) = (
                    Arc::clone(&self.chunk_time),
                    Arc::clone(&self.clock),
                    Arc::clone(&self.beat),
                );
                let mut timer = ChunkTimer::default();
                device.on_packet = Some(Box::new(move |event| {
                    beat.busy();
                    timer.on_packet(event, clock(), &chunk_time);
                }));
                self.connected(device, now);
            }
            Err(Error::NotFound) => self.set_link(Link::Absent),
            Err(error) => self.set_link(Link::Blocked(error)),
        }
        self.schedule_poll(now);
    }

    /// A keyboard that can't even be read isn't one to tell the interface about, or it would come
    /// and go at every look. One that answers with a block it can't use is there, so it's told, and
    /// nothing will be written to it. Its clock waits for a good read.
    fn connected(&mut self, mut device: Device<T>, now: Instant) {
        let read = Config::read(&mut device);
        if let Err(error @ Error::Hid(_)) = read {
            self.set_link(Link::Blocked(error));
            return;
        }

        let model = device.model();
        self.device = Some(device);
        self.model = Some(model);
        self.link = Link::Connected;
        self.emit(Event::Connected { model });
        self.auto_clock_due = true;
        match read {
            Ok(config) => self.read_was_good(&config, true, now),
            Err(error) => {
                self.bad_config = Some(error.clone());
                self.problem(Problem::Device(error));
            }
        }
    }

    fn read_was_good(&mut self, config: &Config, tell_always: bool, now: Instant) {
        self.bad_config = None;
        if let Some(model) = self.model {
            let current = CurrentLighting::from_config(config, &model.lighting);
            if tell_always || self.lighting != Some(current) {
                self.emit(Event::Lighting(current));
            }
            self.lighting = Some(current);
        }
        self.set_clock_if_due(now);
    }

    fn set_clock_if_due(&mut self, now: Instant) {
        let asked = std::mem::take(&mut self.clock_asked);
        let automatic = std::mem::take(&mut self.auto_clock_due) && self.auto_clock;
        if asked || automatic {
            self.sync_clock(now);
        }
    }

    fn read_config(&mut self) -> Result<Config, Error> {
        match &mut self.device {
            Some(device) => Config::read(device),
            None => Err(Error::NotFound),
        }
    }

    /// The timer's look at the keyboard. Unplugging isn't an error worth a message.
    fn refresh_quietly(&mut self, now: Instant) {
        match self.read_config() {
            Ok(config) => self.read_was_good(&config, false, now),
            Err(error @ Error::Hid(_)) => {
                self.record(&error);
                self.lose_device(now);
            }
            Err(error) => {
                if self.bad_config.as_ref() != Some(&error) {
                    self.bad_config = Some(error.clone());
                    self.problem(Problem::Device(error));
                }
            }
        }
        self.schedule_poll(now);
    }

    fn refresh_now(&mut self, now: Instant) {
        match self.read_config() {
            Ok(config) => self.read_was_good(&config, true, now),
            Err(error @ Error::Hid(_)) => self.command_failed(error, now),
            Err(error) => {
                self.bad_config = Some(error.clone());
                self.problem(Problem::Device(error));
            }
        }
        self.schedule_poll(now);
    }

    fn adjust(&mut self, change: LightingChange, now: Instant) {
        if self.device.is_none() {
            self.problem(Problem::NoKeyboard);
            return;
        }
        let change = match self.adjusting {
            Some((earlier, _)) => merged(earlier, change),
            None => change,
        };
        self.adjusting = Some((change, now));
    }

    fn flush_adjustment(&mut self, now: Instant) {
        if let Some((change, _)) = self.adjusting.take() {
            self.apply_lighting(&change, now);
        }
    }

    /// Whether anything can be written to the keyboard.
    fn writable(&self) -> Result<(), Problem> {
        match (&self.device, &self.bad_config) {
            (None, _) => Err(Problem::NoKeyboard),
            (Some(_), Some(_)) => Err(Problem::UnusableConfig),
            (Some(_), None) => Ok(()),
        }
    }

    fn apply_lighting(&mut self, change: &LightingChange, now: Instant) {
        if let Err(problem) = self.writable() {
            self.problem(problem);
            return;
        }
        let Some(device) = &mut self.device else {
            return;
        };
        let pid = device.model().pid;
        let palettes = self.palettes.entry(pid).or_default();
        let before = palettes.clone();
        let applied = lighting::apply(device, change, palettes);
        let changed = (*palettes != before).then(|| Box::new(palettes.clone()));

        match applied {
            Ok(current) => {
                self.lighting = Some(current);
                self.emit(Event::Lighting(current));
                if let Some(palettes) = changed {
                    self.emit(Event::PalettesChanged { pid, palettes });
                }
            }
            Err(error) => self.command_failed(error, now),
        }
        // The keyboard may need a moment before a read shows what was just written.
        if self.device.is_some() {
            self.schedule_poll(now);
        }
    }

    fn sync_clock(&mut self, now: Instant) {
        if let Err(problem) = self.writable() {
            self.problem(problem);
            return;
        }
        let Some(device) = &mut self.device else {
            return;
        };
        let set = device_now().and_then(|time| set_clock(device, &time));
        match set {
            Ok(()) => self.emit(Event::ClockSynced),
            Err(error) => self.command_failed(error, now),
        }
    }

    fn load_image(&mut self, path: PathBuf, fit: Fit, delay: Option<u16>) {
        let Some(model) = self.model else {
            self.loader.supersede();
            self.problem(Problem::NoKeyboard);
            return;
        };
        self.loader.request(LoadRequest {
            path,
            fit,
            delay,
            model,
            chunk_time: self.chunk_time.mean(),
        });
    }

    fn report(&mut self, now: Instant) {
        if self.device.is_none() {
            self.tell_report(Err("no keyboard connected".to_string()));
            return;
        }
        match self.read_config() {
            Ok(config) => self.tell_report(Ok(config.as_bytes().to_vec())),
            Err(error) => {
                self.record(&error);
                self.tell_report(Err(error.to_string()));
                if matches!(error, Error::Hid(_)) {
                    self.lose_device(now);
                }
            }
        }
    }

    fn tell_report(&self, config: Result<Vec<u8>, String>) {
        self.emit(Event::ReportReady(ReportData {
            model: self.model,
            config,
            errors: self.errors.iter().cloned().collect(),
        }));
    }
}

/// `later`'s fields, and `earlier`'s where `later` has none.
fn merged(earlier: LightingChange, later: LightingChange) -> LightingChange {
    LightingChange {
        mode: later.mode.or(earlier.mode),
        brightness: later.brightness.or(earlier.brightness),
        speed: later.speed.or(earlier.speed),
        color: later.color.or(earlier.color),
        sleep: later.sleep.or(earlier.sleep),
    }
}

/// A frame of an [`Animation`], as `R G B` bytes for showing it. The result is three bytes for every
/// pixel of the screen, row by row from the top left.
pub fn rgb_from_565(frame: &[u8]) -> Vec<u8> {
    let (pixels, _) = frame.as_chunks::<2>();
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for &pixel in pixels {
        let value = u16::from_be_bytes(pixel);
        let red = (value >> 11) as u8;
        let green = (value >> 5 & 0x3F) as u8;
        let blue = (value & 0x1F) as u8;
        // The top bits repeat at the bottom, so that full scale in 5 or 6 bits is 255.
        rgb.push(red << 3 | red >> 2);
        rgb.push(green << 2 | green >> 4);
        rgb.push(blue << 3 | blue >> 2);
    }
    rgb
}
