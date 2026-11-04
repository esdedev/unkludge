//! What the state reads as, for an interface to show. Both interfaces show the same words, so which
//! words follow from what is decided here.

use unkludge_protocol::lighting::{LightMode, SleepSetting};
use unkludge_protocol::screen::Fit;

use super::{State, Upload, percent};
use crate::i18n::t;

/// The fits to choose from, in the order they're shown.
pub const FITS: [Fit; 3] = [Fit::Cover, Fit::Contain, Fit::Stretch];

/// What an upload in progress says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgressText {
    /// `42 %`, which a screen reader gets as the bar's value.
    pub percent: String,
    /// What goes over the bar on the left.
    pub label: String,
    /// What goes over the bar on the right. Empty until the keyboard has said something.
    pub remaining: String,
}

/// What the screen's preview says when it has no picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WellHint {
    pub main: String,
    /// A smaller second line. Empty when the main line says it all.
    pub more: String,
}

impl State {
    /// The biggest text in the window.
    pub fn title(&self) -> String {
        self.model()
            .map_or_else(|| "UNKLUDGE".to_string(), |model| model.name.to_uppercase())
    }

    /// What goes under [`State::title`].
    pub fn subtitle(&self) -> String {
        match self.model() {
            Some(model) => format!("Unkludge  ·  PID {:04X}", model.pid),
            None => t().tagline.to_string(),
        }
    }

    /// Why the current mode has no controls, for where they'd be.
    pub fn light_note(&self) -> &'static str {
        match self.light_controls().mode {
            Some(LightMode::Off) => t().lights_off_note,
            Some(LightMode::Custom) => t().custom_mode_note,
            Some(LightMode::Unknown(_)) => t().unknown_effect_note,
            _ => "",
        }
    }

    /// What goes next to the sleep times' name: a value that isn't one of them if the keyboard has
    /// one, and otherwise what's not known about the last of them, before anyone chooses it.
    pub fn sleep_note(&self) -> String {
        match self.light_controls().sleep {
            Some(SleepSetting::Unknown(raw)) => t().sleep_other(raw),
            Some(SleepSetting::Known(_)) => t().sleep_caveat.to_string(),
            None => String::new(),
        }
    }

    /// How long an upload of the chosen file may take, once it's been read.
    pub fn estimate_text(&self) -> Option<String> {
        let image = self.file.as_ref()?.loaded.as_ref()?;
        Some(t().about(image.estimate))
    }

    /// How long each frame shows, if the file is an animation and that can be changed.
    pub fn frame_delay(&self) -> Option<u16> {
        self.animation()
            .filter(|animation| animation.frames().len() > 1)
            .map(|animation| animation.delay_ms())
    }

    /// What a screen reader calls the preview, which is also where a file can be chosen.
    pub fn preview_name(&self) -> String {
        match self.file_name() {
            Some(file) => format!("{}: {file}", t().preview),
            None => format!("{}: {}", t().preview, t().drop_hint),
        }
    }

    /// What the preview says with no picture to show: how the file is coming along, or else how to
    /// choose one.
    pub fn well_hint(&self) -> WellHint {
        match self.file_detail() {
            detail if !detail.is_empty() => WellHint {
                main: detail,
                more: String::new(),
            },
            _ => WellHint {
                main: t().drop_hint.to_string(),
                more: t().drop_hint_more.to_string(),
            },
        }
    }

    /// How an upload is going. An upload that hasn't told anything yet is at 0 %.
    pub fn progress_text(&self) -> ProgressText {
        let percent = format!("{:.0} %", self.upload.share() * 100.0);
        ProgressText {
            label: format!("{}  ·  {percent}", t().uploading),
            remaining: match self.upload {
                Upload::Running { remaining, .. } => t().time_left(remaining),
                _ => String::new(),
            },
            percent,
        }
    }
}

/// A brightness step, as it reads next to its slider.
pub fn brightness_text(step: usize, steps: usize) -> String {
    format!("{} %", percent(step, steps))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use unkludge_protocol::lighting::CurrentLighting;
    use unkludge_protocol::model;
    use unkludge_protocol::screen::{Animation, LoadOptions};

    use super::*;
    use crate::settings::Settings;
    use crate::worker::{Event, LoadedImage};

    fn connected() -> State {
        let mut state = State::default();
        let mut settings = Settings::default();
        let model = model::find(0x0265).unwrap();
        state.apply(Event::Connected { model }, &mut settings);
        state
    }

    #[test]
    fn the_caveat_about_no_sleep_shows_before_anyone_chooses_it() {
        // `with_mode` gives a sleep value that isn't one of the choices, which comes first.
        let mut state = with_mode(LightMode::Off);
        assert_eq!(state.sleep_note(), t().sleep_other(0));
        if let Some(lighting) = &mut state.lighting {
            lighting.sleep = SleepSetting::Known(unkludge_protocol::config::SleepTime::Min5);
        }
        assert_eq!(state.sleep_note(), t().sleep_caveat);
        assert_eq!(State::default().sleep_note(), "");
    }

    fn with_mode(mode: LightMode) -> State {
        let mut state = connected();
        let mut settings = Settings::default();
        state.apply(
            Event::Lighting(CurrentLighting {
                mode,
                brightness: None,
                speed: None,
                color: None,
                sleep: unkludge_protocol::lighting::SleepSetting::Unknown(0),
            }),
            &mut settings,
        );
        state
    }

    fn loaded(frames: usize) -> LoadedImage {
        let model = model::find(0x0265).unwrap();
        let bytes = crate::demo::gif_bytes(frames, 120);
        let options = LoadOptions {
            truncate: true,
            ..LoadOptions::default()
        };
        let animation = Animation::from_bytes(&bytes, &model.screen.unwrap(), options).unwrap();
        LoadedImage {
            path: PathBuf::from("a.gif"),
            plan: animation.plan(model).unwrap(),
            animation: Arc::new(animation),
            estimate: Duration::from_secs(40),
        }
    }

    fn with_file(frames: usize) -> State {
        let mut state = connected();
        let _ = state.choose_file(PathBuf::from("/some/where/a.gif"));
        let mut settings = Settings::default();
        state.apply(Event::ImageLoaded(loaded(frames)), &mut settings);
        state
    }

    #[test]
    fn the_title_is_the_model_or_else_the_app() {
        assert_eq!(State::default().title(), "UNKLUDGE");
        assert_eq!(connected().title(), "RK S85 ES");
    }

    #[test]
    fn the_subtitle_says_which_keyboard_when_theres_one() {
        assert_eq!(State::default().subtitle(), t().tagline);
        assert_eq!(connected().subtitle(), "Unkludge  ·  PID 0265");
    }

    #[test]
    fn a_mode_without_controls_says_why() {
        assert_eq!(with_mode(LightMode::Off).light_note(), t().lights_off_note);
        assert_eq!(
            with_mode(LightMode::Custom).light_note(),
            t().custom_mode_note
        );
        assert_eq!(
            with_mode(LightMode::Unknown(99)).light_note(),
            t().unknown_effect_note
        );
        assert_eq!(with_mode(LightMode::Effect(2)).light_note(), "");
        assert_eq!(State::default().light_note(), "");
    }

    #[test]
    fn the_estimate_waits_for_the_file() {
        assert_eq!(connected().estimate_text(), None);
        assert_eq!(
            with_file(2).estimate_text().as_deref(),
            Some(t().about(Duration::from_secs(40)).as_str())
        );
    }

    #[test]
    fn only_an_animation_has_a_time_per_frame() {
        assert_eq!(connected().frame_delay(), None);
        assert_eq!(with_file(1).frame_delay(), None);
        assert_eq!(with_file(2).frame_delay(), Some(120));
    }

    #[test]
    fn the_preview_is_named_after_its_file_with_no_folders() {
        assert_eq!(
            connected().preview_name(),
            format!("{}: {}", t().preview, t().drop_hint)
        );
        assert_eq!(
            with_file(1).preview_name(),
            format!("{}: a.gif", t().preview)
        );
    }

    #[test]
    fn the_preview_says_how_to_choose_a_file_until_theres_something_else_to_say() {
        let hint = connected().well_hint();
        assert_eq!(hint.main, t().drop_hint);
        assert_eq!(hint.more, t().drop_hint_more);
        let mut reading = connected();
        let _ = reading.choose_file(PathBuf::from("a.gif"));
        let hint = reading.well_hint();
        assert_eq!(hint.main, t().reading_file);
        assert_eq!(hint.more, "");
    }

    #[test]
    fn an_upload_tells_how_far_along_it_is_and_how_long_is_left() {
        let mut state = connected();
        assert_eq!(state.progress_text().percent, "0 %");
        assert_eq!(state.progress_text().remaining, "");
        state.upload = Upload::Running {
            done: 42,
            total: 100,
            remaining: Duration::from_secs(6),
            cancelling: false,
        };
        let text = state.progress_text();
        assert_eq!(text.percent, "42 %");
        assert_eq!(text.label, format!("{}  ·  42 %", t().uploading));
        assert_eq!(text.remaining, t().time_left(Duration::from_secs(6)));
    }

    #[test]
    fn a_brightness_reads_as_a_percentage() {
        assert_eq!(brightness_text(3, 6), "60 %");
    }
}
