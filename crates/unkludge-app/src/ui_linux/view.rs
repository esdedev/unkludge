//! What the window shows, worked out from the state: everything the markup is given, and which choice
//! each of its rows stands for. No window is needed for any of it.

use slint::{Color, SharedString};
use unkludge_protocol::Error;
use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::{ColorSetting, LightMode, ModeChoice, Rgb, SleepSetting};
use unkludge_protocol::screen::Fit;

use super::{
    Area, Banner, Choice, ColorDialog, EmptyView, FooterView, Frame, HeaderData, Labels,
    LightsView, ScreenView, Swatch, Tone,
};
use crate::i18n::t;
use crate::layout::{self, Rect};
use crate::session::Session;
use crate::settings::Settings;
use crate::state::{self, FITS, Link, State, WellHint, brightness_text, percent};

fn text(value: &str) -> SharedString {
    SharedString::from(value)
}

fn area((x0, y0, x1, y1): Rect) -> Area {
    Area {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

fn tone(tone: state::Tone) -> Tone {
    match tone {
        state::Tone::Neutral => Tone::Neutral,
        state::Tone::Good => Tone::Good,
        state::Tone::Warning => Tone::Warning,
        state::Tone::Bad => Tone::Bad,
    }
}

/// The words that don't change while the app runs.
pub fn labels() -> Labels {
    let (tr, linux) = (t(), &t().linux);
    Labels {
        send_report: text(tr.send_report),
        experimental: text(tr.experimental),
        demo: text(tr.demo),
        screen: text(tr.screen),
        lights: text(tr.lights),
        clock: text(tr.clock),
        choose_file: text(tr.choose_file),
        no_file: text(tr.no_file),
        fit: text(tr.fit),
        frame_time: text(tr.frame_time),
        slower: text(tr.slower),
        faster: text(tr.faster),
        upload: text(tr.upload),
        uploading: text(tr.uploading),
        cancel: text(tr.cancel),
        brightness: text(tr.brightness),
        speed: text(tr.speed),
        color: text(tr.color),
        rainbow: text(tr.rainbow),
        custom_color: text(tr.custom_color),
        own_colors: text(tr.own_colors),
        sleep: text(tr.sleep),
        sync_clock: text(tr.sync_clock),
        auto_clock: text(tr.auto_clock),
        auto_clock_more: text(tr.auto_clock_more),
        auto_clock_name: text(tr.auto_clock_name),
        start_with_system: text(tr.start_with_system),
        grant_access: text(linux.grant_access),
        red: text(linux.red),
        green: text(linux.green),
        blue: text(linux.blue),
        color_code: text(linux.color_code),
        color_preview: text(linux.color_preview),
        use_color: text(linux.use_color),
        path_title: text(linux.path_title),
        path_hint: text(linux.path_hint),
        path_field: text(linux.path_field),
        open_file: text(linux.open_file),
        note_title: text(tr.tray_note_title),
        note_text: text(tr.tray_note),
        ok: text(linux.ok),
    }
}

/// Where everything goes, as the Windows interface has it.
pub fn frame(state: &State, frame: &layout::Frame) -> Frame {
    let rows = layout::screen_rows(frame.screen);
    let lights = layout::light_rows(frame.lights);
    let [brightness, speed] = layout::slider_parts(lights.sliders);
    let size = state
        .model()
        .and_then(|model| model.screen)
        .map_or((320, 172), |screen| (screen.width, screen.height));
    Frame {
        height: frame.height,
        screen: area(frame.screen),
        lights: area(frame.lights),
        clock: area(frame.clock),
        empty: area(frame.empty),
        footer: area(frame.footer),
        well: area(rows.well),
        file: area(rows.file),
        fit: area(rows.fit),
        notes: area(rows.notes),
        action: area(rows.action),
        effects: area(lights.effects),
        brightness: area(brightness),
        speed: area(speed),
        colors: area(lights.colors),
        sleep: area(lights.sleep),
        preview: area(layout::centered(
            rows.well,
            f32::from(size.0),
            f32::from(size.1),
        )),
    }
}

pub fn header(state: &State, session: &Session) -> HeaderData {
    let status = state.status();
    HeaderData {
        title: state.title().into(),
        subtitle: state.subtitle().into(),
        status: status.text.into(),
        status_tone: tone(status.tone),
        experimental: state.is_experimental(),
        report: state.offers_report(),
        demo: session.is_demo(),
        connected: state.model().is_some(),
    }
}

pub fn banners(state: &State, frame: &layout::Frame) -> Vec<Banner> {
    state
        .banners()
        .iter()
        .zip(&frame.banners)
        .map(|(banner, rect)| Banner {
            area: area(*rect),
            text: text(&banner.text),
            tone: tone(banner.tone),
        })
        .collect()
}

pub fn screen(state: &State, can_drop: bool) -> ScreenView {
    let (width, height, frames) = state
        .model()
        .and_then(|model| model.screen)
        .map_or((0, 0, 0), |s| (s.width, s.height, s.max_frames));
    let hint = well_hint(state, can_drop);
    let notes = state.screen_notes();
    let joined = notes
        .iter()
        .map(|note| note.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let progress = state.progress_text();
    ScreenView {
        info: t().screen_info(width, height, frames).into(),
        has_picture: state.animation().is_some(),
        asks: !hint.more.is_empty() || (!can_drop && state.file.is_none()),
        hint_main: hint.main.into(),
        hint_more: hint.more.into(),
        preview_name: preview_name(state, can_drop).into(),
        has_file: state.file.is_some(),
        file_name: state.file_name().unwrap_or_default().into(),
        file_detail: state.file_detail().into(),
        animated: state.frame_delay().is_some(),
        delay: state
            .frame_delay()
            .map_or_else(String::new, |delay| format!("{delay} ms"))
            .into(),
        notes: joined.into(),
        notes_tone: notes.first().map_or(Tone::Neutral, |note| tone(note.tone)),
        idle: state.upload.is_idle(),
        can_upload: state.can_upload(),
        estimate: state.estimate_text().unwrap_or_default().into(),
        cancelling: state.upload.is_cancelling(),
        share: state.upload.share(),
        progress_label: progress.label.into(),
        progress_remaining: progress.remaining.into(),
        progress_percent: progress.percent.into(),
    }
}

/// Where files can't be dropped there's only the one way to choose them, which is all this says then.
fn well_hint(state: &State, can_drop: bool) -> WellHint {
    let hint = state.well_hint();
    if !can_drop && state.file.is_none() {
        return WellHint {
            main: t().linux.click_hint.to_string(),
            more: String::new(),
        };
    }
    hint
}

fn preview_name(state: &State, can_drop: bool) -> String {
    if !can_drop && state.file.is_none() {
        return t().labelled(t().preview, t().linux.click_hint);
    }
    state.preview_name()
}

pub fn fits(state: &State, frame: &layout::Frame) -> Vec<Choice> {
    let row = layout::screen_rows(frame.screen).fit;
    FITS.iter()
        .zip(layout::fit_chips(row, FITS.len()))
        .map(|(fit, part)| Choice {
            area: area(part),
            label: text(t().fit_name(*fit)),
            name: t().labelled(t().fit, t().fit_name(*fit)).into(),
            on: state.fit == *fit,
        })
        .collect()
}

/// The fit that a row of [`fits`] stands for.
pub fn fit_at(index: i32) -> Option<Fit> {
    FITS.get(usize::try_from(index).ok()?).copied()
}

/// Off, and then each of the model's effects.
pub fn effects(state: &State, frame: &layout::Frame) -> Vec<Choice> {
    let Some(model) = state.model() else {
        return Vec::new();
    };
    let controls = state.light_controls();
    let list = model.lighting.effects;
    let chips = layout::effect_chips(layout::light_rows(frame.lights).effects, list.len() + 1);
    let mut rows = vec![Choice {
        area: area(chips[0]),
        label: text(t().off),
        name: t().labelled(t().effect, t().off).into(),
        on: controls.mode == Some(LightMode::Off),
    }];
    rows.extend(list.iter().zip(&chips[1..]).map(|(effect, chip)| {
        let name = t().effect_name(effect);
        Choice {
            area: area(*chip),
            label: text(name),
            name: t().labelled(t().effect, name).into(),
            on: controls.mode == Some(LightMode::Effect(effect.id)),
        }
    }));
    rows
}

/// What a row of [`effects`] stands for.
pub fn effect_at(state: &State, index: i32) -> Option<ModeChoice> {
    let index = usize::try_from(index).ok()?;
    match index.checked_sub(1) {
        None => Some(ModeChoice::Off),
        Some(at) => Some(ModeChoice::Effect(
            state.model()?.lighting.effects.get(at)?.id,
        )),
    }
}

/// The sleep times to choose from, when the model has them.
pub fn sleeps(state: &State, frame: &layout::Frame) -> Vec<Choice> {
    let Some(sleep) = state.light_controls().sleep else {
        return Vec::new();
    };
    let row = layout::light_rows(frame.lights).sleep;
    SleepTime::ALL
        .iter()
        .zip(layout::sleep_chips(row, SleepTime::ALL.len()))
        .map(|(choice, part)| {
            let label = t().sleep_label(*choice);
            Choice {
                area: area(part),
                label: text(label),
                name: t().sleep_chip_name(*choice).into(),
                on: sleep == SleepSetting::Known(*choice),
            }
        })
        .collect()
}

/// The sleep time that a row of [`sleeps`] stands for.
pub fn sleep_at(index: i32) -> Option<SleepTime> {
    SleepTime::ALL.get(usize::try_from(index).ok()?).copied()
}

pub fn lights(state: &State) -> LightsView {
    let controls = state.light_controls();
    let (brightness_step, brightness_steps) = controls.brightness.unwrap_or((0, 0));
    let (speed, speed_min, speed_max) = controls.speed.unwrap_or((0, 0, 0));
    let has_effect = controls.effect.is_some();
    LightsView {
        enabled: controls.enabled,
        has_effect,
        note: text(state.light_note()),
        has_brightness: has_effect && controls.brightness.is_some(),
        brightness_step: brightness_step as i32,
        brightness_steps: brightness_steps as i32,
        brightness_text: brightness_text(brightness_step, brightness_steps).into(),
        brightness_percent: percent(brightness_step, brightness_steps) as i32,
        has_speed: has_effect && controls.speed.is_some(),
        speed_step: i32::from(speed - speed_min),
        speed_steps: i32::from(speed_max - speed_min) + 1,
        speed_text: speed.to_string().into(),
        speed_min: i32::from(speed_min),
        own_colors: has_effect && !controls.colors && !controls.rainbow,
        has_colors: has_effect && controls.colors,
        has_rainbow: has_effect && controls.rainbow,
        rainbow_on: controls.color == Some(ColorSetting::Rainbow),
        has_sleep: controls.sleep.is_some(),
        sleep_note: state.sleep_note().into(),
    }
}

pub fn swatches(state: &State, settings: &Settings) -> Vec<Swatch> {
    let controls = state.light_controls();
    if controls.effect.is_none() || !controls.colors {
        return Vec::new();
    }
    state
        .slot_colors(settings)
        .iter()
        .enumerate()
        .map(|(slot, rgb)| Swatch {
            color: color_of(*rgb),
            name: t().color_slot(slot as u8).into(),
            on: controls.color == Some(ColorSetting::Slot(slot as u8)),
        })
        .collect()
}

pub fn color_of(rgb: Rgb) -> Color {
    Color::from_rgb_u8(rgb.r, rgb.g, rgb.b)
}

/// Whether asking for access could help: a keyboard seems to be there, and the system said no.
pub fn access_may_help(state: &State) -> bool {
    matches!(state.link, Link::Blocked(Error::Hid(_)))
}

pub fn empty(state: &State, granting: bool, rule_installed: bool) -> EmptyView {
    let view = state.empty_view();
    let may_help = access_may_help(state);
    let grant_why = match (may_help, rule_installed) {
        (false, _) => "",
        (true, true) => t().linux.access_still_denied,
        (true, false) => t().linux.grant_why,
    };
    EmptyView {
        headline: view.headline.into(),
        detail: view.detail.into(),
        tone: tone(view.tone),
        can_grant: may_help && !rule_installed,
        granting,
        grant_why: text(grant_why),
    }
}

pub fn footer(state: &State, session: &Session, autostart: bool, auto_clock: bool) -> FooterView {
    let (message, message_tone, demo) = match &state.message {
        Some(message) => (message.text.as_str(), tone(message.tone), false),
        None if session.is_demo() => (t().demo_name, Tone::Neutral, true),
        None => ("", Tone::Neutral, false),
    };
    FooterView {
        message: text(message),
        tone: message_tone,
        demo,
        autostart,
        auto_clock,
    }
}

/// A colour's code, as it's written.
pub fn color_code(rgb: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb.r, rgb.g, rgb.b)
}

/// A colour from its code, with or without the `#`.
pub fn parse_color_code(code: &str) -> Option<Rgb> {
    let code = code.trim();
    let digits = code.strip_prefix('#').unwrap_or(code);
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let part = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Rgb::new(part(0)?, part(2)?, part(4)?))
}

/// The colour picker, showing a colour. `code` is what the field holds, which can be a code still
/// being typed, and `warning` what a model's first custom colour has to say.
pub fn color_dialog(rgb: Rgb, code: &str, warning: bool) -> ColorDialog {
    ColorDialog {
        title: if warning {
            text(t().first_custom_title)
        } else {
            text(t().custom_color.trim_end_matches('…'))
        },
        warning: if warning {
            text(t().first_custom_warning)
        } else {
            SharedString::new()
        },
        red: i32::from(rgb.r),
        green: i32::from(rgb.g),
        blue: i32::from(rgb.b),
        code: text(code),
        color: color_of(rgb),
    }
}

#[cfg(test)]
mod tests {
    use unkludge_protocol::lighting::CurrentLighting;
    use unkludge_protocol::model;

    use super::*;
    use crate::state::Upload;
    use crate::worker::Event;

    fn connected(mode: LightMode) -> State {
        let mut state = State::default();
        let mut settings = Settings::default();
        let model = model::find(0x0265).unwrap();
        state.apply(Event::Connected { model }, &mut settings);
        state.apply(
            Event::Lighting(CurrentLighting {
                mode,
                brightness: model.lighting.brightness.first().copied(),
                speed: Some(model.lighting.speed_min),
                color: Some(ColorSetting::Slot(1)),
                sleep: SleepSetting::Known(SleepTime::ALL[1]),
            }),
            &mut settings,
        );
        state
    }

    fn layout_of(state: &State) -> layout::Frame {
        layout::Frame::new(state.banners().len(), state.preview_height())
    }

    fn chosen(rows: &[Choice]) -> Vec<String> {
        rows.iter()
            .filter(|row| row.on)
            .map(|row| row.label.to_string())
            .collect()
    }

    #[test]
    fn the_window_is_as_tall_as_the_layout_says() {
        let state = State::default();
        let layout = layout_of(&state);
        assert_eq!(frame(&state, &layout).height, layout.height);
    }

    #[test]
    fn the_preview_has_the_screens_true_size() {
        let state = connected(LightMode::Off);
        let screen = state.model().unwrap().screen.unwrap();
        let preview = frame(&state, &layout_of(&state)).preview;
        assert_eq!(preview.w, f32::from(screen.width));
        assert_eq!(preview.h, f32::from(screen.height));
    }

    #[test]
    fn every_effect_row_stands_for_its_effect_and_the_first_is_off() {
        let state = connected(LightMode::Effect(2));
        let rows = effects(&state, &layout_of(&state));
        let model = state.model().unwrap();
        assert_eq!(rows.len(), model.lighting.effects.len() + 1);
        assert_eq!(effect_at(&state, 0), Some(ModeChoice::Off));
        for (at, effect) in model.lighting.effects.iter().enumerate() {
            assert_eq!(
                effect_at(&state, at as i32 + 1),
                Some(ModeChoice::Effect(effect.id))
            );
        }
        assert_eq!(effect_at(&state, rows.len() as i32), None);
        assert_eq!(effect_at(&state, -1), None);
    }

    #[test]
    fn the_choice_that_shows_as_chosen_is_the_keyboards() {
        let state = connected(LightMode::Effect(2));
        assert_eq!(
            chosen(&effects(&state, &layout_of(&state))),
            [t().effect_name(&state.model().unwrap().lighting.effects[1])]
        );
        let off = connected(LightMode::Off);
        assert_eq!(chosen(&effects(&off, &layout_of(&off))), [t().off]);
        assert_eq!(
            chosen(&sleeps(&state, &layout_of(&state))),
            [t().sleep_label(SleepTime::ALL[1])]
        );
    }

    #[test]
    fn every_choice_has_a_name_that_makes_sense_on_its_own() {
        let state = connected(LightMode::Effect(2));
        let layout = layout_of(&state);
        let all = [
            fits(&state, &layout),
            effects(&state, &layout),
            sleeps(&state, &layout),
        ];
        for row in all.iter().flatten() {
            assert!(row.name.contains(row.label.as_str()), "{}", row.name);
            assert!(row.name.len() > row.label.len(), "{}", row.name);
        }
        for swatch in swatches(&state, &Settings::default()) {
            assert!(!swatch.name.is_empty());
        }
    }

    #[test]
    fn rows_that_stand_for_something_read_back() {
        for (at, fit) in FITS.iter().enumerate() {
            assert_eq!(fit_at(at as i32), Some(*fit));
        }
        assert_eq!(fit_at(FITS.len() as i32), None);
        for (at, time) in SleepTime::ALL.iter().enumerate() {
            assert_eq!(sleep_at(at as i32), Some(*time));
        }
        assert_eq!(sleep_at(-1), None);
    }

    #[test]
    fn nothing_about_the_lights_can_be_used_during_an_upload() {
        let mut state = connected(LightMode::Effect(2));
        assert!(lights(&state).enabled);
        state.upload = Upload::Starting { cancelling: false };
        let view = lights(&state);
        assert!(!view.enabled);
        assert!(view.has_effect, "still shown");
    }

    #[test]
    fn with_the_lights_off_theres_a_note_instead_of_controls() {
        let view = lights(&connected(LightMode::Off));
        assert!(!view.has_effect && !view.has_brightness && !view.has_speed);
        assert_eq!(view.note, t().lights_off_note);
    }

    #[test]
    fn the_screen_says_when_its_cancelling_and_can_no_longer_cancel_again() {
        let mut state = connected(LightMode::Off);
        state.upload = Upload::Running {
            done: 1,
            total: 10,
            remaining: std::time::Duration::from_secs(1),
            cancelling: true,
        };
        let view = screen(&state, true);
        assert!(view.cancelling);
        assert!(!view.idle);
        assert!(!view.can_upload);
        assert_eq!(view.notes, t().cancelling);
        assert_eq!(view.notes_tone, Tone::Warning);
    }

    #[test]
    fn where_files_cant_be_dropped_the_hint_doesnt_ask_to() {
        let state = connected(LightMode::Off);
        let droppable = screen(&state, true);
        assert_eq!(droppable.hint_main, t().drop_hint);
        assert!(droppable.asks);
        let view = screen(&state, false);
        assert_eq!(view.hint_main, t().linux.click_hint);
        assert_eq!(view.hint_more, "");
        assert!(view.asks);
        assert!(!view.preview_name.contains(t().drop_hint));
    }

    #[test]
    fn access_is_only_offered_when_the_system_said_no() {
        let mut state = State::default();
        assert!(!empty(&state, false, false).can_grant);
        state.link = Link::Blocked(Error::Hid("denied".to_string()));
        let view = empty(&state, false, false);
        assert!(view.can_grant);
        assert_eq!(view.grant_why, t().linux.grant_why);
        state.link = Link::Blocked(Error::Unsupported(0x002A));
        assert!(!empty(&state, false, false).can_grant);
        assert_eq!(empty(&state, false, false).grant_why, "");
    }

    #[test]
    fn with_the_rule_already_there_it_says_to_plug_the_keyboard_in_again() {
        let mut state = State::default();
        state.link = Link::Blocked(Error::Hid("denied".to_string()));
        let view = empty(&state, false, true);
        assert!(!view.can_grant);
        assert_eq!(view.grant_why, t().linux.access_still_denied);
    }

    #[test]
    fn an_unsupported_keyboard_says_so() {
        let mut state = State::default();
        state.link = Link::Blocked(Error::Unsupported(0x002A));
        assert_eq!(empty(&state, false, false).headline, t().unsupported_title);
        assert!(header(&state, &Session::Real).report);
    }

    #[test]
    fn the_foot_of_the_window_says_when_its_a_demo_and_has_nothing_better_to_say() {
        let demo = Session::Demo(crate::args::DemoArgs::default());
        let mut state = State::default();
        assert_eq!(footer(&state, &Session::Real, false, true).message, "");
        let view = footer(&state, &demo, false, true);
        assert_eq!(view.message, t().demo_name);
        assert!(view.demo);
        state.message = Some(state::Note::new("hello", state::Tone::Good));
        let view = footer(&state, &demo, true, false);
        assert_eq!(view.message, "hello");
        assert!(!view.demo);
        assert_eq!(view.tone, Tone::Good);
        assert!(view.autostart);
        assert!(!view.auto_clock);
    }

    #[test]
    fn colour_codes_read_back_and_nonsense_doesnt() {
        let rgb = Rgb::new(0x12, 0xAB, 0xFF);
        assert_eq!(color_code(rgb), "#12ABFF");
        assert_eq!(parse_color_code("#12ABFF"), Some(rgb));
        assert_eq!(parse_color_code("12abff"), Some(rgb));
        assert_eq!(parse_color_code(" #12abff "), Some(rgb));
        for wrong in ["", "#", "#12AB", "#12ABFF00", "#GGGGGG", "ññññ", "#ñ1234"] {
            assert_eq!(parse_color_code(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn only_a_models_first_custom_colour_warns() {
        assert_eq!(
            color_dialog(Rgb::new(1, 2, 3), "#010203", true).warning,
            t().first_custom_warning
        );
        let quiet = color_dialog(Rgb::new(1, 2, 3), "#010203", false);
        assert_eq!(quiet.warning, "");
        assert_eq!(quiet.red, 1);
        assert_eq!(quiet.blue, 3);
    }
}
