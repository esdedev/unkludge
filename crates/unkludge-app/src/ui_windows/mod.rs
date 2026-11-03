//! The Windows interface: a Win32 window drawn with Direct2D and DirectWrite, and a tray icon.
//!
//! Each file holds one part of [`App`]: `startup` its start, `window` its messages and drawing, `events` what the worker
//! says, `input` the mouse and the keyboard, `header`, `screen_section` and `lights_section` what's drawn, `tray` the tray,
//! `dialogs` the system's dialogs, `a11y` what screen readers see, and `shot` the demo's pictures.
//! What the state means and where things go aren't here: see `state` and `layout`, which the Linux
//! interface shares.

mod a11y;
mod dialogs;
mod events;
mod gfx;
mod header;
mod input;
mod lights_section;
mod screen_section;
mod shot;
mod startup;
mod tray;
mod window;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::ModeChoice;
use unkludge_protocol::screen::Fit;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::ID2D1Factory;
use windows::Win32::Graphics::DirectWrite::IDWriteFactory;
use windows::Win32::UI::Accessibility::IAccessible;
use windows::Win32::UI::WindowsAndMessaging::HICON;

pub use dialogs::refuse_start;
pub use startup::run;

use crate::args::DemoArgs;
use crate::demo::Script;
use crate::layout::Frame;
use crate::settings::Settings;
use crate::state::{Slider, State};
use crate::worker::{Event, WorkerHandle};
use gfx::{Fonts, Gfx, R};

/// Something that can be clicked or focused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Report,
    Preview,
    ChooseFile,
    Fit(Fit),
    Slower,
    Faster,
    Upload,
    Cancel,
    SyncClock,
    AutoClock,
    Mode(ModeChoice),
    Brightness,
    Speed,
    Slot(u8),
    Rainbow,
    CustomColor,
    Sleep(SleepTime),
    Autostart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Button,
    Toggle,
    /// One of a set of choices.
    Choice,
    Slider,
    Link,
}

/// One interactive thing as painted last. The tab order, the hit test and what a screen reader sees
/// all come from the list of these.
#[derive(Clone, Debug)]
pub struct Item {
    pub rect: R,
    pub hit: Hit,
    pub name: String,
    /// What a slider or the like reads as.
    pub value: Option<String>,
    pub role: Role,
    /// A toggle that's on, or the choice that's chosen.
    pub on: bool,
    pub enabled: bool,
}

/// What the window and its screen reader object share.
#[derive(Default)]
pub struct Shared {
    pub items: Vec<Item>,
    pub focus: Option<Hit>,
    /// Device pixels per DIP.
    pub scale: f32,
}

/// Something that opens a dialog, which runs its own message loop, so it can't be done while the
/// app is borrowed for handling a message.
#[derive(Clone, Copy, PartialEq)]
enum Deferred {
    ChooseFile,
    CustomColor,
    TrayMenu,
}

/// A value that glides to where it's going, for the toggles.
struct Anim {
    value: f32,
    target: f32,
}

impl Anim {
    fn new(on: bool) -> Anim {
        let value = f32::from(u8::from(on));
        Anim {
            value,
            target: value,
        }
    }

    fn set(&mut self, on: bool) {
        self.target = f32::from(u8::from(on));
    }

    /// Whether it's still moving.
    fn step(&mut self) -> bool {
        let left = self.target - self.value;
        if left.abs() < 0.01 {
            self.value = self.target;
            false
        } else {
            self.value += left * 0.28;
            true
        }
    }
}

/// A demo's parts. See `crate::demo`.
struct DemoSession {
    args: DemoArgs,
    script: Script,
    shot_timer_set: bool,
}

pub struct App {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    fonts: Fonts,
    gfx: Option<Gfx>,
    /// Pixels per inch: the monitor's, or what a demo asked for.
    dpi: f32,
    settings: Settings,
    state: State,
    frame: Frame,
    /// `None` once it's been told to stop.
    worker: Option<WorkerHandle>,
    events: Receiver<Event>,
    /// A wake-up message is on its way, so the worker needn't post another.
    woken: Arc<AtomicBool>,
    demo: Option<DemoSession>,
    shared: Rc<RefCell<Shared>>,
    accessible: Option<IAccessible>,
    /// The ring around what has the focus shows after using the keyboard, not the mouse.
    show_focus: bool,
    hover: Option<Hit>,
    tracking_mouse: bool,
    drag: Option<Slider>,
    autostart: bool,
    a_autostart: Anim,
    a_auto_clock: Anim,
    /// When the animation being shown started.
    preview_since: Instant,
    /// When the whole window was last drawn.
    last_paint: Instant,
    /// What the preview's timer is set to, if it's running.
    preview_timer: Option<u32>,
    deferred: Option<Deferred>,
    /// A dialog is open: the tray's menu waits, and quitting too.
    modal: bool,
    tray_icon: HICON,
    quit: bool,
}
