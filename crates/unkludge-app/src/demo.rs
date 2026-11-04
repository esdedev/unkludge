//! A keyboard that doesn't exist, for seeing the interface without touching a real one. Nothing here
//! can open a device: the worker only gets the transport in this file, which answers as a keyboard
//! would. It takes what's written to its settings and shows it back, and it acks slowly enough for
//! an upload to be seen moving.
//!
//! [`Script`] takes the interface to a given state by doing what a user would, through the same
//! worker and the same events as the real thing.

use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use unkludge_protocol::config::CONFIG_LEN;
use unkludge_protocol::model::Model;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error, REPORT_ID};

use crate::worker::Event;

const CONFIG_WRITE: u8 = 0x04;
const CONFIG_READ: u8 = 0x84;
const SCREEN_DATA: u8 = 0x0C;
const SCREEN_CONTROL: u8 = 0x0D;
const HEADER_LEN: usize = 8;
const ACK_LEN: usize = 8;
const CONFIG_READ_HEADER: [u8; HEADER_LEN] =
    [REPORT_ID, CONFIG_READ, 0, 0, 1, 0, CONFIG_LEN as u8, 0];
/// The longest any pause of the protocol takes here.
const LONGEST_PAUSE: Duration = Duration::from_millis(2);
const STALL_SETTLES: Duration = Duration::from_millis(250);
/// An older model, which is known and not supported.
const UNSUPPORTED_PID: u16 = 0x002A;

/// Byte offsets of the config block, as in the protocol's docs.
const EFFECT: usize = 0x0A;
const CUSTOM_FLAG: usize = 0x09;
const SLEEP: usize = 0x18;
const EFFECT_SETTINGS: usize = 0x38;
const END_MARKER: usize = 0x7E;

/// What the demo can be asked to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoState {
    NoKeyboard,
    NoKeyboardRk,
    CannotOpen,
    /// A device of the same vendor that this version can't use.
    Unsupported,
    /// A supported keyboard without the collection Unkludge uses.
    NoCollection,
    /// A `unkludge` command has the keyboard.
    InUse,
    Connected,
    RkApp,
    Hung,
    Solid,
    LightsOff,
    CustomMode,
    OtherSleep,
    Image,
    Still,
    ImageLong,
    Loading,
    Uploading,
    Cancelling,
    Done,
    Cancelled,
    Failed,
    Unconfirmed,
    /// The focus ring, after pressing Tab a few times.
    Focus,
    /// What sending a report leaves behind.
    Report,
}

const STATES: [(&str, DemoState); 25] = [
    ("no-keyboard", DemoState::NoKeyboard),
    ("no-keyboard-rk", DemoState::NoKeyboardRk),
    ("cannot-open", DemoState::CannotOpen),
    ("unsupported", DemoState::Unsupported),
    ("no-collection", DemoState::NoCollection),
    ("in-use", DemoState::InUse),
    ("connected", DemoState::Connected),
    ("rk-app", DemoState::RkApp),
    ("hung", DemoState::Hung),
    ("solid", DemoState::Solid),
    ("lights-off", DemoState::LightsOff),
    ("custom-mode", DemoState::CustomMode),
    ("other-sleep", DemoState::OtherSleep),
    ("image", DemoState::Image),
    ("still", DemoState::Still),
    ("image-long", DemoState::ImageLong),
    ("loading", DemoState::Loading),
    ("uploading", DemoState::Uploading),
    ("cancelling", DemoState::Cancelling),
    ("done", DemoState::Done),
    ("cancelled", DemoState::Cancelled),
    ("failed", DemoState::Failed),
    ("unconfirmed", DemoState::Unconfirmed),
    ("focus", DemoState::Focus),
    ("report", DemoState::Report),
];

impl DemoState {
    pub fn parse(name: &str) -> Option<DemoState> {
        STATES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, state)| *state)
    }

    pub fn names() -> Vec<&'static str> {
        STATES.iter().map(|(name, _)| *name).collect()
    }

    /// How many frames of how many milliseconds the file it shows has, if it shows one.
    fn file_frames(self, model: &Model) -> Option<(usize, u16)> {
        let limit = model.screen.map_or(60, |s| usize::from(s.max_frames));
        match self {
            DemoState::Still => Some((1, 100)),
            DemoState::ImageLong => Some((limit + 12, 40)),
            DemoState::Image
            | DemoState::Loading
            | DemoState::Uploading
            | DemoState::Cancelling
            | DemoState::Done
            | DemoState::Cancelled
            | DemoState::Failed
            | DemoState::Unconfirmed => Some((24, 200)),
            _ => None,
        }
    }

    fn uploads(self) -> bool {
        matches!(
            self,
            DemoState::Uploading
                | DemoState::Cancelling
                | DemoState::Done
                | DemoState::Cancelled
                | DemoState::Failed
                | DemoState::Unconfirmed
        )
    }
}

/// What the keyboard that isn't there is doing. Shared with its transports and the script.
pub struct DemoKeyboard {
    model: &'static Model,
    present: AtomicBool,
    open_error: AtomicBool,
    unsupported: AtomicBool,
    no_collection: AtomicBool,
    in_use: AtomicBool,
    rk_app: AtomicBool,
    /// How long each chunk's ack takes.
    chunk_ms: AtomicU32,
    /// Stops acknowledging, as a stalled keyboard does.
    stalled: AtomicBool,
    /// Refuses chunks.
    refusing: AtomicBool,
    /// Doesn't acknowledge an upload's finish.
    no_finish_ack: AtomicBool,
    config: Mutex<[u8; CONFIG_LEN]>,
}

impl DemoKeyboard {
    pub fn new(model: &'static Model, state: DemoState) -> Arc<DemoKeyboard> {
        let absent = matches!(state, DemoState::NoKeyboard | DemoState::NoKeyboardRk);
        Arc::new(DemoKeyboard {
            model,
            present: AtomicBool::new(!absent),
            open_error: AtomicBool::new(state == DemoState::CannotOpen),
            unsupported: AtomicBool::new(state == DemoState::Unsupported),
            no_collection: AtomicBool::new(state == DemoState::NoCollection),
            in_use: AtomicBool::new(state == DemoState::InUse),
            rk_app: AtomicBool::new(matches!(state, DemoState::RkApp | DemoState::NoKeyboardRk)),
            chunk_ms: AtomicU32::new(1),
            stalled: AtomicBool::new(false),
            refusing: AtomicBool::new(false),
            no_finish_ack: AtomicBool::new(state == DemoState::Unconfirmed),
            config: Mutex::new(starting_config(model, state)),
        })
    }

    pub fn rk_app_running(&self) -> bool {
        self.rk_app.load(Ordering::SeqCst)
    }

    /// What the worker calls to look for a keyboard.
    pub fn open(self: &Arc<DemoKeyboard>) -> Result<Device<DemoTransport>, Error> {
        if self.open_error.load(Ordering::SeqCst) {
            return Err(Error::Hid("demo: the system refused access".to_string()));
        }
        if self.unsupported.load(Ordering::SeqCst) {
            return Err(Error::Unsupported(UNSUPPORTED_PID));
        }
        if self.no_collection.load(Ordering::SeqCst) {
            return Err(Error::NoUsableCollection(self.model.pid));
        }
        if self.in_use.load(Ordering::SeqCst) {
            return Err(Error::DeviceBusy);
        }
        if !self.present.load(Ordering::SeqCst) {
            return Err(Error::NotFound);
        }
        let screen = self
            .model
            .has_second_collection()
            .then(|| DemoTransport::new(Arc::clone(self)));
        Device::new(DemoTransport::new(Arc::clone(self)), screen, self.model)
    }

    fn config(&self) -> std::sync::MutexGuard<'_, [u8; CONFIG_LEN]> {
        self.config.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A config block that reads well on any supported model: every effect at a middle brightness, its
/// default speed and its second colour, showing the Breathing effect.
fn starting_config(model: &Model, state: DemoState) -> [u8; CONFIG_LEN] {
    let mut bytes = [0; CONFIG_LEN];
    let lighting = &model.lighting;
    let brightness = lighting
        .brightness
        .get(lighting.brightness.len() / 2)
        .copied()
        .unwrap_or(0);
    for effect in lighting.effects {
        let at = EFFECT_SETTINGS + 2 * usize::from(effect.id);
        bytes[at] = brightness;
        bytes[at + 1] = lighting.speed_default << 4 | 1;
    }
    bytes[EFFECT] = match state {
        DemoState::Solid => 1,
        DemoState::LightsOff => 0,
        _ => 2,
    };
    if state == DemoState::CustomMode {
        bytes[CUSTOM_FLAG] = 1;
    }
    // 0x14 is 10 min. 0x1E isn't one of the options, as a keyboard set elsewhere could have.
    bytes[SLEEP] = if state == DemoState::OtherSleep {
        0x1E
    } else {
        0x14
    };
    bytes[END_MARKER..].copy_from_slice(&[0x5A, 0xA5]);
    bytes
}

/// One collection of the keyboard that isn't there.
pub struct DemoTransport {
    keyboard: Arc<DemoKeyboard>,
    ack: Cell<Option<[u8; ACK_LEN]>>,
    config_requested: Cell<bool>,
}

impl DemoTransport {
    fn new(keyboard: Arc<DemoKeyboard>) -> DemoTransport {
        DemoTransport {
            keyboard,
            ack: Cell::new(None),
            config_requested: Cell::new(false),
        }
    }
}

impl Transport for DemoTransport {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        match report[1] {
            SCREEN_DATA if self.keyboard.stalled.load(Ordering::SeqCst) => {}
            SCREEN_DATA if self.keyboard.refusing.load(Ordering::SeqCst) => {
                self.ack.set(Some(ack(report[4], report[5], false)));
            }
            SCREEN_DATA => self.ack.set(Some(ack(report[4], report[5], true))),
            SCREEN_CONTROL => {
                // The state is in the top two bits of the first byte of data, and a finish is 0.
                let finish = report[HEADER_LEN] >> 6 == 0;
                if !(finish && self.keyboard.no_finish_ack.load(Ordering::SeqCst)) {
                    self.ack.set(Some(ack(0, 0, true)));
                }
            }
            CONFIG_READ => self.config_requested.set(true),
            CONFIG_WRITE => {
                let end = HEADER_LEN + CONFIG_LEN;
                self.keyboard
                    .config()
                    .copy_from_slice(&report[HEADER_LEN..end]);
            }
            _ => {}
        }
        Ok(())
    }

    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        if !self.config_requested.take() {
            return Err(Error::Hid("demo: only the config can be read".to_string()));
        }
        buf.fill(0);
        buf[..HEADER_LEN].copy_from_slice(&CONFIG_READ_HEADER);
        buf[HEADER_LEN..HEADER_LEN + CONFIG_LEN].copy_from_slice(&*self.keyboard.config());
        Ok(buf.len())
    }

    /// An ack that's due takes its time. With none due this waits a little, so that looking for an
    /// ack that won't come doesn't spin. A stalled keyboard uses up all of `ms`, the protocol's whole
    /// wait, which is what makes cancelling take a while.
    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error> {
        match self.ack.take() {
            Some(ack) => {
                let wait = self.keyboard.chunk_ms.load(Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(u64::from(wait)));
                buf[..ACK_LEN].copy_from_slice(&ack);
                Ok(ACK_LEN)
            }
            None => {
                let most = if self.keyboard.stalled.load(Ordering::SeqCst) {
                    ms
                } else {
                    50
                };
                let wait = ms.clamp(0, most);
                std::thread::sleep(Duration::from_millis(wait as u64));
                Ok(0)
            }
        }
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration.min(LONGEST_PAUSE));
    }
}

/// An ack: it repeats the chunk's index and check byte, and says whether the chunk was taken.
fn ack(idx: u8, check: u8, accepted: bool) -> [u8; ACK_LEN] {
    [REPORT_ID, 0x0A, 0x06, u8::from(accepted), idx, check, 0, 0]
}

/// The next thing to do to reach the state asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// As if chosen in the window.
    ChooseFile(PathBuf),
    /// As if the button were clicked.
    Upload,
    /// As if the button were clicked.
    Cancel,
    /// As if the link were clicked.
    Report,
    /// Something the keyboard takes too long to make happen for itself.
    Pretend(Event),
}

/// Takes the interface to a state, step by step, as the worker's events come in.
pub struct Script {
    state: DemoState,
    keyboard: Arc<DemoKeyboard>,
    folder: Option<PathBuf>,
    file: Option<PathBuf>,
    uploaded: bool,
    interfered: bool,
    reached: bool,
}

impl Script {
    /// `folder` is where the demo's picture goes for as long as this lasts: the user's own, as
    /// anything shared could have something waiting under the picture's name.
    pub fn new(state: DemoState, keyboard: Arc<DemoKeyboard>, folder: Option<PathBuf>) -> Script {
        Script {
            state,
            keyboard,
            folder,
            file: None,
            uploaded: false,
            interfered: false,
            reached: false,
        }
    }

    /// Whether it's there, so that a picture can be taken. Says yes once something has been shown
    /// for the state, which for an upload in progress means partway through it.
    pub fn reached(&self) -> bool {
        self.reached
    }

    /// The steps that an event calls for.
    pub fn on_event(&mut self, event: &Event) -> Vec<Step> {
        let mut steps = Vec::new();
        match event {
            Event::Disconnected
                if matches!(self.state, DemoState::NoKeyboard | DemoState::NoKeyboardRk) =>
            {
                self.reached = true;
            }
            Event::CannotOpen(_)
                if matches!(
                    self.state,
                    DemoState::CannotOpen
                        | DemoState::Unsupported
                        | DemoState::NoCollection
                        | DemoState::InUse
                ) =>
            {
                self.reached = true;
            }
            Event::Lighting(_) => {
                if let Some(step) = self.after_lights() {
                    steps.push(step);
                }
            }
            Event::ImageLoaded(_) => {
                if self.state.uploads() {
                    if !self.uploaded {
                        self.uploaded = true;
                        steps.push(Step::Upload);
                    }
                } else if self.state != DemoState::Loading {
                    self.reached = true;
                }
            }
            Event::Progress { done, total, .. } => {
                // Far enough along to see the bar, then slowly enough for it to stay there.
                if self.state == DemoState::Uploading && done * 100 >= total * 42 {
                    self.keyboard.chunk_ms.store(150, Ordering::SeqCst);
                    self.reached = true;
                }
                if done * 100 >= total * 12 && !self.interfered {
                    self.interfered = true;
                    match self.state {
                        DemoState::Cancelling => {
                            self.keyboard.stalled.store(true, Ordering::SeqCst);
                            // Time for the worker to get stuck on the next chunk, so that the
                            // cancel has to wait, as it does with a keyboard that has stopped.
                            std::thread::sleep(STALL_SETTLES);
                            steps.push(Step::Cancel);
                        }
                        DemoState::Cancelled => steps.push(Step::Cancel),
                        DemoState::Failed => {
                            self.keyboard.refusing.store(true, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
            }
            Event::Cancelling if self.state == DemoState::Cancelling => self.reached = true,
            Event::UploadDone | Event::UploadCancelled | Event::UploadFailed(_) => {
                self.reached = true;
            }
            Event::ReportReady(_) => self.reached = true,
            _ => {}
        }
        steps
    }

    /// The first word on the lights means the keyboard is in place.
    fn after_lights(&mut self) -> Option<Step> {
        match self.state.file_frames(self.keyboard.model) {
            Some((frames, delay)) => {
                if self.file.is_some() {
                    return None;
                }
                let written = write_gif(self.folder.as_deref()?, frames, delay);
                let path = match written {
                    Ok(path) => path,
                    Err(error) => {
                        crate::log::error(&format!("the demo's picture can't be written: {error}"));
                        return None;
                    }
                };
                self.file = Some(path.clone());
                if self.state == DemoState::Loading {
                    self.reached = true;
                }
                Some(Step::ChooseFile(path))
            }
            None if self.state == DemoState::Report => {
                let first = !self.uploaded;
                self.uploaded = true;
                first.then_some(Step::Report)
            }
            None => {
                let first = !self.reached;
                self.reached = true;
                (first && self.state == DemoState::Hung).then_some(Step::Pretend(Event::Hung))
            }
        }
    }
}

impl Script {
    /// How many times to press Tab before the picture.
    pub fn tabs_before_shot(&self) -> usize {
        if self.state == DemoState::Focus { 4 } else { 0 }
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// How many names to try for the demo's picture.
const GIF_NAMES: u32 = 16;

/// Writes the demo's picture as a new file. Whatever already has its name, a file or a link to one,
/// stays as it is and doesn't get written through: another name is tried instead.
fn write_gif(folder: &Path, frames: usize, delay: u16) -> io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let bytes = gif_bytes(frames, delay);
    for attempt in 0..GIF_NAMES {
        let name = format!("unkludge-demo-{}-{attempt}.gif", std::process::id());
        let path = folder.join(name);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(&bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// Size of the demo's picture, a little wider than most screens, so that the fits differ.
const GIF_WIDTH: usize = 320;
const GIF_HEIGHT: usize = 180;
/// The palette has this many colours of a repeating hue ramp, then white.
const HUES: usize = 127;
const WHITE: u8 = HUES as u8;
/// A code of 8 bits per pixel. The decoder's table has to be cleared before codes need 9.
const LZW_MIN_CODE: u8 = 7;
const LZW_CLEAR: u8 = 128;
const LZW_END: u8 = 129;
const PIXELS_BETWEEN_CLEARS: usize = 100;

/// An animated GIF, written by hand as the app has no image encoder. Its "compression" is 8 bits per
/// pixel, which is valid and simple.
pub fn gif_bytes(frames: usize, delay_ms: u16) -> Vec<u8> {
    let mut gif = Vec::with_capacity(frames * GIF_WIDTH * GIF_HEIGHT + 1024);
    gif.extend(b"GIF89a");
    gif.extend((GIF_WIDTH as u16).to_le_bytes());
    gif.extend((GIF_HEIGHT as u16).to_le_bytes());
    // A global colour table of 128 entries.
    gif.extend([0xF6, 0, 0]);
    for i in 0..HUES {
        gif.extend(hue(i as f32 / HUES as f32));
    }
    gif.extend([255, 255, 255]);
    // Loop forever.
    gif.extend([0x21, 0xFF, 0x0B]);
    gif.extend(b"NETSCAPE2.0");
    gif.extend([3, 1, 0, 0, 0]);
    let delay = (delay_ms / 10).to_le_bytes();
    for frame in 0..frames {
        gif.extend([0x21, 0xF9, 4, 0, delay[0], delay[1], 0, 0]);
        gif.push(0x2C);
        gif.extend([0, 0, 0, 0]);
        gif.extend((GIF_WIDTH as u16).to_le_bytes());
        gif.extend((GIF_HEIGHT as u16).to_le_bytes());
        gif.push(0);
        gif.push(LZW_MIN_CODE);
        for block in frame_codes(frame, frames).chunks(255) {
            gif.push(block.len() as u8);
            gif.extend(block);
        }
        gif.push(0);
    }
    gif.push(0x3B);
    gif
}

fn frame_codes(frame: usize, frames: usize) -> Vec<u8> {
    let phase = frame * HUES / frames.max(1);
    let mut codes = Vec::with_capacity(GIF_WIDTH * GIF_HEIGHT * 102 / 100 + 2);
    let mut since_clear = PIXELS_BETWEEN_CLEARS;
    for y in 0..GIF_HEIGHT {
        for x in 0..GIF_WIDTH {
            if since_clear == PIXELS_BETWEEN_CLEARS {
                codes.push(LZW_CLEAR);
                since_clear = 0;
            }
            codes.push(pixel(x, y, phase));
            since_clear += 1;
        }
    }
    codes.push(LZW_END);
    codes
}

/// A hue ramp sliding across the picture, with the name in white.
fn pixel(x: usize, y: usize, phase: usize) -> u8 {
    if in_title(x, y) {
        return WHITE;
    }
    (((x + y) / 4 + phase) % HUES) as u8
}

/// Letters of 5 by 7 cells, one bit each, top row first.
const TITLE: [[u8; 7]; 7] = [
    [
        0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
    ], // K
    [
        0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
    ], // E
    [
        0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
    ], // Y
    [
        0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
    ], // L
    [
        0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
    ], // U
    [
        0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
    ], // M
    [
        0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
    ], // E
];
const CELL: usize = 5;
const LETTER_W: usize = 6 * CELL;

fn in_title(x: usize, y: usize) -> bool {
    let left = (GIF_WIDTH - TITLE.len() * LETTER_W + CELL) / 2;
    let top = (GIF_HEIGHT - 7 * CELL) / 2;
    if x < left || y < top || y >= top + 7 * CELL {
        return false;
    }
    let letter = (x - left) / LETTER_W;
    let column = (x - left) % LETTER_W / CELL;
    let row = (y - top) / CELL;
    letter < TITLE.len() && column < 5 && TITLE[letter][row] >> (4 - column) & 1 == 1
}

/// A bright colour on the wheel, with `at` from 0 to 1.
fn hue(at: f32) -> [u8; 3] {
    let wheel = at * 6.0;
    let rise = wheel.fract();
    let (r, g, b) = match wheel as u8 {
        0 => (1.0, rise, 0.0),
        1 => (1.0 - rise, 1.0, 0.0),
        2 => (0.0, 1.0, rise),
        3 => (0.0, 1.0 - rise, 1.0),
        4 => (rise, 0.0, 1.0),
        _ => (1.0, 0.0, 1.0 - rise),
    };
    // Softened, so that white text stands out.
    [r, g, b].map(|c: f32| (60.0 + c * 170.0) as u8)
}

#[cfg(test)]
mod tests;
