//! What the interface knows, and what follows from each of the worker's events and each thing the
//! user does. No drawing and no system calls, so that both interfaces can share it and tests can run
//! it without a window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use unkludge_protocol::Error;
use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::{
    ColorChoice, ColorSetting, CurrentLighting, LightMode, LightingChange, ModeChoice, Palettes,
    Rgb, SleepSetting,
};
use unkludge_protocol::model::{Effect, Model, Status};
use unkludge_protocol::screen::{Animation, Fit};

use crate::i18n::t;
use crate::layout::step_at;
use crate::settings::Settings;
use crate::worker::{Cmd, Event, LoadedImage, Problem, ReportData};

mod texts;

pub use texts::{FITS, WellHint, brightness_text};

/// The kind of thing a text says, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Good,
    Warning,
    Bad,
}

/// A text to show, and the kind of thing it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub tone: Tone,
}

impl Note {
    pub fn new(text: impl Into<String>, tone: Tone) -> Note {
        Note {
            text: text.into(),
            tone,
        }
    }

    /// Passing news goes away by itself. A problem stays until something replaces it.
    pub fn fades(&self) -> bool {
        matches!(self.tone, Tone::Neutral | Tone::Good)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// The worker hasn't said yet.
    Searching,
    Absent,
    /// A keyboard seems to be there, but this keeps it from being used.
    Blocked(Error),
    Connected(&'static Model),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upload {
    Idle,
    /// Asked for, and the worker hasn't said yet. It can be cancelled already.
    Starting {
        cancelling: bool,
    },
    Running {
        done: usize,
        total: usize,
        remaining: Duration,
        cancelling: bool,
    },
}

impl Upload {
    pub fn is_idle(&self) -> bool {
        *self == Upload::Idle
    }

    pub fn is_cancelling(&self) -> bool {
        matches!(
            self,
            Upload::Starting { cancelling: true }
                | Upload::Running {
                    cancelling: true,
                    ..
                }
        )
    }

    /// How far along, from 0 to 1.
    pub fn share(&self) -> f32 {
        match *self {
            Upload::Running { done, total, .. } if total > 0 => done as f32 / total as f32,
            _ => 0.0,
        }
    }
}

/// The file chosen for the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChosenFile {
    pub path: PathBuf,
    /// As decoded for the connected keyboard, once that's done.
    pub loaded: Option<LoadedImage>,
    pub loading: bool,
}

/// What an event means besides the state changing.
#[derive(Default)]
pub struct Reaction {
    /// To send to the worker.
    pub cmds: Vec<Cmd>,
    /// The settings changed.
    pub save_settings: bool,
    /// To save and show.
    pub report: Option<ReportData>,
}

/// Which lighting controls the current effect has. Values are as the keyboard has them, unless a
/// slider has moved since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightControls {
    /// Nothing can be changed: an upload is running, or the keyboard's settings can't be used.
    pub enabled: bool,
    pub mode: Option<LightMode>,
    pub effect: Option<&'static Effect>,
    /// The step of the model's brightness list, and how many steps there are.
    pub brightness: Option<(usize, usize)>,
    /// The value, the lowest and the highest.
    pub speed: Option<(u8, u8, u8)>,
    pub color: Option<ColorSetting>,
    pub colors: bool,
    pub rainbow: bool,
    pub sleep: Option<SleepSetting>,
}

/// A slider of the lights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slider {
    Brightness,
    Speed,
}

/// What to show when there's no keyboard to set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyView {
    pub headline: String,
    pub detail: String,
    pub tone: Tone,
}

/// What the interface knows.
pub struct State {
    pub link: Link,
    pub lighting: Option<CurrentLighting>,
    pub rk_app: bool,
    pub hung: bool,
    pub file: Option<ChosenFile>,
    pub fit: Fit,
    /// `None` means what the file says.
    pub delay: Option<u16>,
    pub upload: Upload,
    /// How the last upload went, shown with the screen's controls.
    pub outcome: Option<Note>,
    /// Why the last file can't be used, which an upload ending doesn't change.
    pub file_note: Option<Note>,
    /// The upload that's running didn't get its finish confirmed.
    unconfirmed: bool,
    /// The settings file couldn't be read, so nothing gets saved. See [`Settings::read_error`].
    pub settings_unreadable: bool,
    /// Anything else, at the foot of the window.
    pub message: Option<Note>,
    /// Slider values on their way to the keyboard, which the next word from it replaces.
    pending: LightingChange,
}

impl Default for State {
    fn default() -> Self {
        State {
            link: Link::Searching,
            lighting: None,
            rk_app: false,
            hung: false,
            file: None,
            fit: Fit::default(),
            delay: None,
            upload: Upload::Idle,
            outcome: None,
            file_note: None,
            unconfirmed: false,
            settings_unreadable: false,
            message: None,
            pending: LightingChange::default(),
        }
    }
}

impl State {
    /// How things stand with these settings loaded.
    pub fn with_settings(settings: &Settings) -> State {
        State {
            settings_unreadable: settings.read_error.is_some(),
            ..State::default()
        }
    }

    pub fn model(&self) -> Option<&'static Model> {
        match self.link {
            Link::Connected(model) => Some(model),
            _ => None,
        }
    }

    /// The tallest thing the preview must make room for. Before there's a keyboard, the commonest
    /// size, so that a keyboard arriving usually leaves the window as it is.
    pub fn preview_height(&self) -> u16 {
        self.model()
            .and_then(|model| model.screen)
            .map_or(172, |screen| screen.height)
    }

    /// Takes in what the worker said.
    pub fn apply(&mut self, event: Event, settings: &mut Settings) -> Reaction {
        let mut reaction = Reaction::default();
        match event {
            Event::Connected { model } => {
                self.link = Link::Connected(model);
                self.message = None;
                self.lighting = None;
                if let Some(cmd) = self.load_chosen() {
                    reaction.cmds.push(cmd);
                }
            }
            Event::Disconnected => {
                self.link = Link::Absent;
                self.lost_keyboard();
            }
            Event::CannotOpen(error) => {
                self.link = Link::Blocked(error);
                self.lost_keyboard();
            }
            Event::Lighting(lighting) => {
                self.lighting = Some(lighting);
                self.pending = LightingChange::default();
            }
            Event::PalettesChanged { pid, palettes } => {
                settings.palettes.insert(pid, *palettes);
                reaction.save_settings = true;
            }
            Event::ImageLoaded(image) => {
                if let Some(file) = &mut self.file {
                    file.loaded = Some(image);
                    file.loading = false;
                }
            }
            Event::ImageFailed(error) => {
                self.file = None;
                self.file_note = Some(Note::new(
                    format!("{}: {}", t().image_failed, t().error(&error)),
                    Tone::Bad,
                ));
            }
            Event::UploadStarted { total, estimate } => {
                self.upload = Upload::Running {
                    done: 0,
                    total,
                    remaining: estimate,
                    cancelling: self.upload.is_cancelling(),
                };
                self.outcome = None;
            }
            Event::Progress {
                done,
                total,
                remaining,
            } => {
                self.upload = Upload::Running {
                    done,
                    total,
                    remaining,
                    cancelling: self.upload.is_cancelling(),
                };
            }
            Event::Cancelling => {
                if let Upload::Starting { cancelling } | Upload::Running { cancelling, .. } =
                    &mut self.upload
                {
                    *cancelling = true;
                }
            }
            Event::UploadUnconfirmed => {
                self.unconfirmed = true;
            }
            Event::UploadDone => {
                self.upload = Upload::Idle;
                // After `UploadUnconfirmed`, that's the news.
                self.outcome = Some(if std::mem::take(&mut self.unconfirmed) {
                    Note::new(t().upload_unconfirmed, Tone::Warning)
                } else {
                    Note::new(t().upload_done, Tone::Good)
                });
            }
            Event::UploadCancelled => {
                self.upload = Upload::Idle;
                self.outcome = Some(Note::new(t().upload_cancelled, Tone::Neutral));
            }
            Event::UploadFailed(error) => {
                self.upload = Upload::Idle;
                self.outcome = Some(Note::new(
                    format!("{}: {}", t().upload_failed, t().error(&error)),
                    Tone::Bad,
                ));
            }
            Event::ClockSynced => {
                self.message = Some(Note::new(t().clock_synced, Tone::Good));
            }
            Event::Error(problem) => {
                // An upload that was asked for and refused never starts.
                if matches!(self.upload, Upload::Starting { .. }) {
                    self.upload = Upload::Idle;
                }
                if problem == Problem::NoKeyboard
                    && let Some(file) = &mut self.file
                {
                    file.loading = false;
                }
                self.pending = LightingChange::default();
                self.message = Some(Note::new(t().problem(&problem), Tone::Bad));
            }
            Event::RkAppRunning(running) => self.rk_app = running,
            Event::Hung => self.hung = true,
            Event::Responding => self.hung = false,
            Event::ReportReady(data) => reaction.report = Some(data),
        }
        reaction
    }

    fn lost_keyboard(&mut self) {
        self.lighting = None;
        self.upload = Upload::Idle;
        self.message = None;
        if let Some(file) = &mut self.file {
            file.loaded = None;
            file.loading = false;
        }
    }

    /// Decodes the chosen file again, for the keyboard that's there now and the current choices.
    fn load_chosen(&mut self) -> Option<Cmd> {
        self.model()?;
        let (fit, delay) = (self.fit, self.delay);
        let file = self.file.as_mut()?;
        file.loading = true;
        Some(Cmd::LoadImage {
            path: file.path.clone(),
            fit,
            delay,
        })
    }

    // What the user can do with the screen.

    /// A new file. With no keyboard yet it waits for one.
    pub fn choose_file(&mut self, path: PathBuf) -> Option<Cmd> {
        self.file = Some(ChosenFile {
            path,
            loaded: None,
            loading: false,
        });
        self.delay = None;
        self.outcome = None;
        self.file_note = None;
        self.load_chosen()
    }

    pub fn set_fit(&mut self, fit: Fit) -> Option<Cmd> {
        if fit == self.fit {
            return None;
        }
        self.fit = fit;
        self.load_chosen()
    }

    /// One step slower or faster on an animation. A still image has no use for it.
    pub fn step_delay(&mut self, slower: bool) -> Option<Cmd> {
        let animation = self.animation()?;
        if animation.frames().len() < 2 {
            return None;
        }
        let delay = next_delay(animation.delay_ms(), slower);
        if delay == animation.delay_ms() {
            return None;
        }
        self.delay = Some(delay);
        self.load_chosen()
    }

    pub fn animation(&self) -> Option<&Arc<Animation>> {
        Some(&self.file.as_ref()?.loaded.as_ref()?.animation)
    }

    pub fn can_upload(&self) -> bool {
        self.upload.is_idle()
            && self.model().is_some()
            && self.file.as_ref().is_some_and(|file| !file.loading)
            && self.animation().is_some()
    }

    pub fn start_upload(&mut self) -> Option<Cmd> {
        if !self.can_upload() {
            return None;
        }
        let animation = Arc::clone(self.animation()?);
        self.upload = Upload::Starting { cancelling: false };
        self.outcome = None;
        self.unconfirmed = false;
        Some(Cmd::UploadScreen(animation))
    }

    pub fn cancel_upload(&self) -> Option<Cmd> {
        (!self.upload.is_idle() && !self.upload.is_cancelling()).then_some(Cmd::CancelUpload)
    }

    /// What to say about the file, with the screen's controls: the news from an upload, or else
    /// what will happen to the frames.
    pub fn screen_notes(&self) -> Vec<Note> {
        let mut news = Vec::new();
        if self.upload.is_cancelling() {
            news.push(Note::new(t().cancelling, Tone::Warning));
        } else if let Some(outcome) = &self.outcome {
            news.push(outcome.clone());
        }
        news.extend(self.file_note.clone());
        if !news.is_empty() {
            return news;
        }
        let Some(image) = self.file.as_ref().and_then(|file| file.loaded.as_ref()) else {
            return Vec::new();
        };
        let frames = image.animation.frames().len();
        let mut notes = Vec::new();
        if image.animation.truncated() {
            notes.push(Note::new(t().frames_cut(frames), Tone::Warning));
        }
        if image.plan.frame_count() < frames {
            let draw = self
                .model()
                .and_then(|model| model.screen)
                .map_or(0, |screen| screen.draw_frame_time);
            notes.push(Note::new(
                t().frames_thinned(
                    image.plan.frame_count(),
                    frames,
                    image.animation.delay_ms(),
                    draw,
                ),
                Tone::Warning,
            ));
        }
        notes
    }

    /// What the file holds, or that it's being read.
    pub fn file_detail(&self) -> String {
        let Some(file) = &self.file else {
            return String::new();
        };
        if file.loading {
            return t().reading_file.to_string();
        }
        match &file.loaded {
            Some(image) => {
                t().frames_info(image.animation.frames().len(), image.animation.delay_ms())
            }
            None => String::new(),
        }
    }

    pub fn file_name(&self) -> Option<String> {
        let file = self.file.as_ref()?;
        Some(display_name(&file.path))
    }

    // What the user can do with the lights.

    /// Controls for the current effect.
    pub fn light_controls(&self) -> LightControls {
        let mut controls = LightControls {
            enabled: false,
            mode: None,
            effect: None,
            brightness: None,
            speed: None,
            color: None,
            colors: false,
            rainbow: false,
            sleep: None,
        };
        let (Some(model), Some(lighting)) = (self.model(), self.lighting) else {
            return controls;
        };
        controls.enabled = self.upload.is_idle();
        controls.mode = Some(lighting.mode);
        controls.sleep = model.lighting.sleep.then_some(lighting.sleep);
        let LightMode::Effect(id) = lighting.mode else {
            return controls;
        };
        let Some(effect) = model.lighting.effects.iter().find(|e| e.id == id) else {
            return controls;
        };
        controls.effect = Some(effect);
        let steps = model.lighting.brightness;
        if effect.brightness && !steps.is_empty() {
            let value = self.pending.brightness.or(lighting.brightness).unwrap_or(0);
            controls.brightness = Some((nearest_step(steps, value), steps.len()));
        }
        if effect.speed {
            let value = self.pending.speed.or(lighting.speed).unwrap_or(0);
            let (min, max) = (model.lighting.speed_min, model.lighting.speed_max);
            controls.speed = Some((value.clamp(min, max), min, max));
        }
        controls.color = lighting.color;
        controls.colors = effect.color;
        controls.rainbow = effect.rainbow;
        controls
    }

    fn change(&self, change: LightingChange) -> Option<LightingChange> {
        self.light_controls().enabled.then_some(change)
    }

    pub fn set_mode(&self, mode: ModeChoice) -> Option<Cmd> {
        self.change(LightingChange {
            mode: Some(mode),
            ..LightingChange::default()
        })
        .map(Cmd::SetLighting)
    }

    pub fn set_color(&self, color: ColorChoice) -> Option<Cmd> {
        self.change(LightingChange {
            color: Some(color),
            ..LightingChange::default()
        })
        .map(Cmd::SetLighting)
    }

    pub fn set_sleep(&self, sleep: SleepTime) -> Option<Cmd> {
        self.change(LightingChange {
            sleep: Some(sleep),
            ..LightingChange::default()
        })
        .map(Cmd::SetLighting)
    }

    /// A step of the model's brightness list, while a slider moves.
    pub fn adjust_brightness(&mut self, step: usize) -> Option<Cmd> {
        let value = *self.model()?.lighting.brightness.get(step)?;
        if self.light_controls().brightness.map(|(now, _)| now) == Some(step) {
            return None;
        }
        let change = self.change(LightingChange {
            brightness: Some(value),
            ..LightingChange::default()
        })?;
        self.pending.brightness = Some(value);
        Some(Cmd::AdjustLighting(change))
    }

    /// A speed, while a slider moves.
    pub fn adjust_speed(&mut self, value: u8) -> Option<Cmd> {
        if self.light_controls().speed.map(|(now, _, _)| now) == Some(value) {
            return None;
        }
        let change = self.change(LightingChange {
            speed: Some(value),
            ..LightingChange::default()
        })?;
        self.pending.speed = Some(value);
        Some(Cmd::AdjustLighting(change))
    }

    /// One step along a slider, from the keyboard. Stops at the ends.
    pub fn nudge(&mut self, slider: Slider, by: i32) -> Option<Cmd> {
        let controls = self.light_controls();
        match slider {
            Slider::Brightness => {
                let (step, steps) = controls.brightness?;
                self.adjust_brightness((step as i32 + by).clamp(0, steps as i32 - 1) as usize)
            }
            Slider::Speed => {
                let (value, min, max) = controls.speed?;
                let value = (i32::from(value) + by).clamp(i32::from(min), i32::from(max));
                self.adjust_speed(value as u8)
            }
        }
    }

    /// A slider being dragged to `x`, on a track running from `x0` to `x1`.
    pub fn drag_slider(&mut self, slider: Slider, x: f32, x0: f32, x1: f32) -> Option<Cmd> {
        let controls = self.light_controls();
        match slider {
            Slider::Brightness => {
                let (_, steps) = controls.brightness?;
                self.adjust_brightness(step_at(x, x0, x1, steps))
            }
            Slider::Speed => {
                let (_, min, max) = controls.speed?;
                let steps = usize::from(max - min) + 1;
                self.adjust_speed(min + step_at(x, x0, x1, steps) as u8)
            }
        }
    }

    /// The first custom colour for a model writes every effect's colours, and the keyboard's own
    /// can't be read first, so the others go back to the defaults. After that there's a saved copy.
    pub fn custom_color_resets_others(&self, settings: &Settings) -> bool {
        self.model()
            .is_some_and(|model| !settings.palettes.contains_key(&model.pid))
    }

    /// The colours the current effect has, as far as this computer knows.
    pub fn slot_colors(&self, settings: &Settings) -> [Rgb; 7] {
        let effect = self.light_controls().effect.map_or(0, |effect| effect.id);
        let saved = self
            .model()
            .and_then(|model| settings.palettes.get(&model.pid));
        let defaults = Palettes::default();
        let palettes = saved.unwrap_or(&defaults);
        std::array::from_fn(|slot| {
            palettes
                .get(effect, slot as u8)
                .unwrap_or(Rgb::new(0, 0, 0))
        })
    }

    // The header.

    /// The pill at the top right: what's up with the keyboard.
    pub fn status(&self) -> Note {
        if self.hung {
            return Note::new(t().not_responding, Tone::Warning);
        }
        match self.link {
            Link::Searching => Note::new(t().looking, Tone::Neutral),
            Link::Absent => Note::new(t().disconnected, Tone::Bad),
            Link::Blocked(Error::Unsupported(_)) => Note::new(t().unsupported_short, Tone::Warning),
            Link::Blocked(Error::DeviceBusy) => Note::new(t().in_use_short, Tone::Warning),
            Link::Blocked(_) => Note::new(t().cannot_open_short, Tone::Bad),
            Link::Connected(_) => Note::new(t().connected, Tone::Good),
        }
    }

    pub fn is_experimental(&self) -> bool {
        self.model()
            .is_some_and(|model| model.status == Status::Experimental)
    }

    /// Whether a report is the thing to do next: a model not tried yet, or one not supported.
    pub fn offers_report(&self) -> bool {
        self.is_experimental() || matches!(self.link, Link::Blocked(Error::Unsupported(_)))
    }

    /// What to show when there's no keyboard to set.
    pub fn empty_view(&self) -> EmptyView {
        let view = |headline: &str, detail: String, tone| EmptyView {
            headline: headline.to_string(),
            detail,
            tone,
        };
        match &self.link {
            Link::Blocked(Error::Unsupported(pid)) => view(
                t().unsupported_title,
                t().unsupported_detail(*pid),
                Tone::Warning,
            ),
            Link::Blocked(Error::DeviceBusy) => view(
                t().in_use_title,
                t().in_use_detail.to_string(),
                Tone::Warning,
            ),
            Link::Blocked(error) => view(t().cannot_open, t().error(error), Tone::Bad),
            _ if self.file.is_some() => view(
                t().connect_keyboard,
                t().file_waiting(&self.file_name().unwrap_or_default()),
                Tone::Neutral,
            ),
            Link::Searching => view(t().connect_keyboard, t().looking.to_string(), Tone::Neutral),
            _ => view(
                t().connect_keyboard,
                t().connect_hint.to_string(),
                Tone::Neutral,
            ),
        }
    }

    /// Warnings that stay for as long as they're true, across the whole window.
    pub fn banners(&self) -> Vec<Note> {
        let mut banners = Vec::new();
        if self.settings_unreadable {
            banners.push(Note::new(t().settings_unreadable, Tone::Warning));
        }
        if self.hung {
            banners.push(Note::new(t().hung, Tone::Warning));
        }
        if self.rk_app {
            banners.push(Note::new(t().rk_app_warning, Tone::Warning));
        }
        banners
    }
}

/// How far along a step is, as a percentage, for showing a slider's value.
pub fn percent(step: usize, steps: usize) -> u32 {
    if steps < 2 {
        return 0;
    }
    (step * 100 / (steps - 1)) as u32
}

/// An animation's preview needn't go faster than this, whatever the file says.
pub const PREVIEW_MIN_MS: u32 = 33;

/// How often an animation's preview should look for its next frame.
pub fn preview_interval(delay_ms: u16) -> u32 {
    u32::from(delay_ms).max(PREVIEW_MIN_MS)
}

/// Which frame of an animation is showing, `elapsed_ms` after it started.
pub fn preview_frame(elapsed_ms: u128, frames: usize, delay_ms: u16) -> usize {
    if frames < 2 {
        return 0;
    }
    (elapsed_ms / u128::from(delay_ms.max(1)) % frames as u128) as usize
}

/// A file's name, for showing. No folder: it isn't needed, and a window may end up in a screenshot.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The place in the list whose value is closest. A keyboard can hold a value that isn't listed.
pub fn nearest_step(steps: &[u8], value: u8) -> usize {
    steps
        .iter()
        .enumerate()
        .min_by_key(|(_, step)| step.abs_diff(value))
        .map_or(0, |(i, _)| i)
}

/// The delays the stepper goes through, in milliseconds.
const DELAYS: [u16; 19] = [
    20, 30, 40, 50, 60, 70, 80, 100, 120, 150, 180, 200, 250, 300, 400, 500, 750, 1000, 2000,
];

/// The next delay in the list from `delay`, which may not be in it.
pub fn next_delay(delay: u16, slower: bool) -> u16 {
    let next = if slower {
        DELAYS.iter().find(|&&step| step > delay)
    } else {
        DELAYS.iter().rev().find(|&&step| step < delay)
    };
    next.copied().unwrap_or(delay)
}

#[cfg(test)]
mod tests;
