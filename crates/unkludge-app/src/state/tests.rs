use super::*;
use unkludge_protocol::model;
use unkludge_protocol::screen::LoadOptions;

fn s85() -> &'static Model {
    model::find(0x0265).unwrap()
}

fn connected() -> (State, Settings) {
    let mut state = State::default();
    let mut settings = Settings::default();
    state.apply(Event::Connected { model: s85() }, &mut settings);
    (state, settings)
}

fn lighting(mode: LightMode) -> CurrentLighting {
    CurrentLighting {
        mode,
        brightness: Some(8),
        speed: Some(2),
        color: Some(ColorSetting::Slot(1)),
        sleep: SleepSetting::Known(SleepTime::Min10),
    }
}

fn with_lights(mode: LightMode) -> (State, Settings) {
    let (mut state, mut settings) = connected();
    state.apply(Event::Lighting(lighting(mode)), &mut settings);
    (state, settings)
}

/// A GIF of as many frames as asked for, for the model's screen.
fn image(model: &'static Model, frames: usize, delay: u16) -> LoadedImage {
    let bytes = crate::demo::gif_bytes(frames, delay);
    let animation = Animation::from_bytes(
        &bytes,
        &model.screen.unwrap(),
        LoadOptions {
            truncate: true,
            ..LoadOptions::default()
        },
    )
    .unwrap();
    let plan = animation.plan(model).unwrap();
    LoadedImage {
        path: PathBuf::from("a.gif"),
        animation: Arc::new(animation),
        plan,
        estimate: Duration::from_secs(40),
    }
}

fn with_image(frames: usize, delay: u16) -> (State, Settings) {
    let (mut state, mut settings) = connected();
    state.choose_file(PathBuf::from("a.gif"));
    let image = image(s85(), frames, delay);
    state.apply(Event::ImageLoaded(image), &mut settings);
    (state, settings)
}

fn running(state: &mut State, settings: &mut Settings) {
    state.start_upload().unwrap();
    state.apply(
        Event::UploadStarted {
            total: 100,
            estimate: Duration::from_secs(40),
        },
        settings,
    );
}

#[test]
fn it_starts_out_looking() {
    let state = State::default();
    assert_eq!(state.link, Link::Searching);
    assert_eq!(state.status().tone, Tone::Neutral);
    assert_eq!(state.model(), None);
}

#[test]
fn connecting_shows_the_model_and_the_lights_when_they_come() {
    let (state, _) = connected();
    assert_eq!(state.model(), Some(s85()));
    assert_eq!(state.status().tone, Tone::Good);
    assert!(!state.light_controls().enabled);
    let (state, _) = with_lights(LightMode::Effect(2));
    assert!(state.light_controls().enabled);
}

#[test]
fn every_supported_model_is_experimental_for_now() {
    let (state, _) = connected();
    assert!(state.is_experimental());
    assert!(!State::default().is_experimental());
}

#[test]
fn losing_the_keyboard_drops_what_depended_on_it_and_keeps_the_file() {
    let (mut state, mut settings) = with_image(1, 100);
    state.apply(Event::Lighting(lighting(LightMode::Off)), &mut settings);
    state.apply(Event::Disconnected, &mut settings);
    assert_eq!(state.link, Link::Absent);
    assert_eq!(state.lighting, None);
    let file = state.file.as_ref().unwrap();
    assert_eq!(file.path, PathBuf::from("a.gif"));
    assert_eq!(file.loaded, None);
    assert!(!state.can_upload());
}

#[test]
fn a_keyboard_coming_back_decodes_the_file_for_it() {
    let (mut state, mut settings) = with_image(1, 100);
    state.apply(Event::Disconnected, &mut settings);
    let reaction = state.apply(
        Event::Connected {
            model: model::find(0x01A2).unwrap(),
        },
        &mut settings,
    );
    assert!(matches!(
        reaction.cmds.as_slice(),
        [Cmd::LoadImage { path, .. }] if path == Path::new("a.gif")
    ));
    assert!(state.file.as_ref().unwrap().loading);
}

#[test]
fn a_file_chosen_with_no_keyboard_waits_for_one() {
    let mut state = State::default();
    let mut settings = Settings::default();
    assert!(state.choose_file(PathBuf::from("b.png")).is_none());
    assert!(!state.file.as_ref().unwrap().loading);
    let reaction = state.apply(Event::Connected { model: s85() }, &mut settings);
    assert_eq!(reaction.cmds.len(), 1);
}

#[test]
fn a_blocked_keyboard_says_so() {
    let mut state = State::default();
    state.apply(
        Event::CannotOpen(Error::Hid("denied".to_string())),
        &mut Settings::default(),
    );
    assert_eq!(state.status().tone, Tone::Bad);
    assert!(matches!(state.link, Link::Blocked(_)));
}

#[test]
fn a_file_needs_decoding_before_it_can_go_up() {
    let (mut state, _) = connected();
    assert!(!state.can_upload());
    let cmd = state.choose_file(PathBuf::from("a.gif")).unwrap();
    assert!(matches!(cmd, Cmd::LoadImage { .. }));
    assert!(!state.can_upload());
    assert!(state.start_upload().is_none());
    assert_eq!(state.file_detail(), t().reading_file);
}

#[test]
fn a_new_file_forgets_the_earlier_delay_and_news() {
    let (mut state, _) = with_image(4, 100);
    state.delay = Some(50);
    state.outcome = Some(Note::new("old news", Tone::Good));
    state.choose_file(PathBuf::from("b.gif"));
    assert_eq!(state.delay, None);
    assert_eq!(state.outcome, None);
}

#[test]
fn a_file_that_fails_goes_away_and_says_why() {
    let (mut state, mut settings) = connected();
    state.choose_file(PathBuf::from("bad.gif"));
    state.apply(
        Event::ImageFailed(Error::Image("not a picture".to_string())),
        &mut settings,
    );
    assert_eq!(state.file, None);
    let notes = state.screen_notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].tone, Tone::Bad);
    assert!(notes[0].text.contains("not a picture"));
    assert!(notes[0].text.contains(t().image_failed));
}

#[test]
fn changing_the_fit_reloads_with_the_new_one() {
    let (mut state, _) = with_image(1, 100);
    assert!(state.set_fit(Fit::Cover).is_none(), "already that");
    assert!(matches!(
        state.set_fit(Fit::Stretch),
        Some(Cmd::LoadImage {
            fit: Fit::Stretch,
            ..
        })
    ));
    assert!(state.file.as_ref().unwrap().loading);
}

#[test]
fn with_no_file_the_fit_is_remembered_for_the_next_one() {
    let (mut state, _) = connected();
    assert!(state.set_fit(Fit::Contain).is_none());
    assert!(matches!(
        state.choose_file(PathBuf::from("a.gif")),
        Some(Cmd::LoadImage {
            fit: Fit::Contain,
            ..
        })
    ));
}

#[test]
fn stepping_the_delay_reloads_an_animation_with_it() {
    let (mut state, _) = with_image(4, 100);
    assert!(matches!(
        state.step_delay(true),
        Some(Cmd::LoadImage {
            delay: Some(120),
            ..
        })
    ));
}

#[test]
fn a_still_image_has_no_delay_to_step() {
    let (mut state, _) = with_image(1, 100);
    assert!(state.step_delay(true).is_none());
    assert_eq!(state.delay, None);
}

#[test]
fn delays_step_through_the_list_from_anywhere() {
    assert_eq!(next_delay(100, true), 120);
    assert_eq!(next_delay(100, false), 80);
    assert_eq!(next_delay(110, true), 120);
    assert_eq!(next_delay(110, false), 100);
    assert_eq!(next_delay(2000, true), 2000);
    assert_eq!(next_delay(20, false), 20);
    assert_eq!(next_delay(5, true), 20);
    assert_eq!(next_delay(60000, false), 2000);
}

#[test]
fn an_upload_goes_from_starting_to_running_to_done() {
    let (mut state, mut settings) = with_image(1, 100);
    assert!(state.can_upload());
    assert!(matches!(state.start_upload(), Some(Cmd::UploadScreen(_))));
    assert_eq!(state.upload, Upload::Starting { cancelling: false });
    assert!(state.start_upload().is_none(), "only one at a time");

    state.apply(
        Event::UploadStarted {
            total: 100,
            estimate: Duration::from_secs(40),
        },
        &mut settings,
    );
    state.apply(
        Event::Progress {
            done: 25,
            total: 100,
            remaining: Duration::from_secs(30),
        },
        &mut settings,
    );
    assert_eq!(state.upload.share(), 0.25);
    assert!(!state.can_upload());

    state.apply(Event::UploadDone, &mut settings);
    assert!(state.upload.is_idle());
    assert_eq!(state.outcome.as_ref().unwrap().tone, Tone::Good);
    assert!(state.can_upload());
}

#[test]
fn the_lights_are_locked_for_the_whole_upload() {
    let (mut state, mut settings) = with_image(1, 100);
    state.apply(
        Event::Lighting(lighting(LightMode::Effect(2))),
        &mut settings,
    );
    assert!(state.set_mode(ModeChoice::Off).is_some());
    running(&mut state, &mut settings);
    assert!(!state.light_controls().enabled);
    assert!(state.set_mode(ModeChoice::Off).is_none());
    assert!(state.adjust_brightness(0).is_none());
    state.apply(Event::UploadDone, &mut settings);
    assert!(state.light_controls().enabled);
}

#[test]
fn cancelling_lasts_until_the_end_event_and_cant_be_asked_twice() {
    let (mut state, mut settings) = with_image(1, 100);
    assert!(state.cancel_upload().is_none(), "nothing to cancel");
    running(&mut state, &mut settings);
    assert!(matches!(state.cancel_upload(), Some(Cmd::CancelUpload)));
    state.apply(Event::Cancelling, &mut settings);
    assert!(state.upload.is_cancelling());
    assert!(state.cancel_upload().is_none());
    assert_eq!(state.screen_notes()[0].text, t().cancelling);

    // Progress can still arrive, and doesn't undo it.
    state.apply(
        Event::Progress {
            done: 30,
            total: 100,
            remaining: Duration::from_secs(20),
        },
        &mut settings,
    );
    assert!(state.upload.is_cancelling());

    state.apply(Event::UploadCancelled, &mut settings);
    assert!(state.upload.is_idle());
    assert_eq!(state.outcome.as_ref().unwrap().text, t().upload_cancelled);
}

#[test]
fn a_cancel_can_lose_the_race_to_a_finished_upload() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(Event::Cancelling, &mut settings);
    state.apply(Event::UploadDone, &mut settings);
    assert!(state.upload.is_idle());
    assert_eq!(state.outcome.as_ref().unwrap().text, t().upload_done);
}

#[test]
fn an_unconfirmed_end_stays_the_news_after_done() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(Event::UploadUnconfirmed, &mut settings);
    state.apply(Event::UploadDone, &mut settings);
    let outcome = state.outcome.as_ref().unwrap();
    assert_eq!(outcome.text, t().upload_unconfirmed);
    assert_eq!(outcome.tone, Tone::Warning);
}

#[test]
fn a_failed_upload_says_why_and_can_be_tried_again() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(
        Event::UploadFailed(Error::NoAck { frame: 1, idx: 2 }),
        &mut settings,
    );
    let outcome = state.outcome.as_ref().unwrap();
    assert_eq!(outcome.tone, Tone::Bad);
    assert!(outcome.text.contains(t().upload_failed));
    assert!(state.can_upload());
}

#[test]
fn an_upload_that_is_refused_never_counts_as_running() {
    let (mut state, mut settings) = with_image(1, 100);
    state.start_upload().unwrap();
    state.apply(Event::Error(Problem::UnusableConfig), &mut settings);
    assert!(state.upload.is_idle());
    assert_eq!(state.message.as_ref().unwrap().text, t().unusable_config);
}

#[test]
fn a_problem_during_an_upload_doesnt_end_it() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(Event::Error(Problem::Busy), &mut settings);
    assert!(matches!(state.upload, Upload::Running { .. }));
    assert_eq!(state.message.as_ref().unwrap().tone, Tone::Bad);
}

#[test]
fn notes_say_when_frames_are_cut_or_thinned() {
    // Over the S85's limit of 60, and faster than its draw time of 180 ms.
    let (state, _) = with_image(70, 40);
    let notes = state.screen_notes();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].text, t().frames_cut(60));
    assert!(notes[1].text.contains("180"), "{}", notes[1].text);
    // A few frames at a slow speed go as they are.
    let (state, _) = with_image(4, 200);
    assert!(state.screen_notes().is_empty());
}

#[test]
fn upload_news_goes_in_place_of_the_frame_notes() {
    let (mut state, mut settings) = with_image(70, 40);
    running(&mut state, &mut settings);
    state.apply(Event::UploadDone, &mut settings);
    let notes = state.screen_notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].text, t().upload_done);
}

#[test]
fn a_file_is_shown_by_its_name_alone() {
    let (mut state, _) = connected();
    state.choose_file(PathBuf::from("C:/Users/someone/Pictures/cat.gif"));
    assert_eq!(state.file_name().as_deref(), Some("cat.gif"));
}

#[test]
fn controls_follow_the_effect() {
    // Solid has a brightness and colours, and no speed.
    let (state, _) = with_lights(LightMode::Effect(1));
    let controls = state.light_controls();
    assert_eq!(controls.brightness, Some((2, 6)));
    assert_eq!(controls.speed, None);
    assert!(controls.colors && controls.rainbow);
    assert_eq!(controls.color, Some(ColorSetting::Slot(1)));

    // Blossom has speed and brightness, with its own colours.
    let (state, _) = with_lights(LightMode::Effect(17));
    let controls = state.light_controls();
    assert_eq!(controls.speed, Some((2, 0, 5)));
    assert!(!controls.colors && !controls.rainbow);
}

#[test]
fn with_the_lights_off_only_the_sleep_time_remains() {
    for mode in [LightMode::Off, LightMode::Custom, LightMode::Unknown(40)] {
        let (state, _) = with_lights(mode);
        let controls = state.light_controls();
        assert!(controls.enabled);
        assert_eq!(controls.mode, Some(mode));
        assert_eq!(controls.effect, None);
        assert_eq!(controls.brightness, None);
        assert_eq!(controls.speed, None);
        assert!(!controls.colors);
        assert_eq!(controls.sleep, Some(lighting(mode).sleep));
    }
}

#[test]
fn a_brightness_the_list_lacks_shows_as_the_nearest_step() {
    assert_eq!(nearest_step(&[0, 4, 8, 12, 16, 20], 9), 2);
    assert_eq!(nearest_step(&[0, 4, 8, 12, 16, 20], 200), 5);
    assert_eq!(nearest_step(&[], 9), 0);
}

#[test]
fn a_moving_slider_shows_its_value_until_the_keyboard_answers() {
    let (mut state, mut settings) = with_lights(LightMode::Effect(2));
    let cmd = state.adjust_brightness(5).unwrap();
    assert!(matches!(
        cmd,
        Cmd::AdjustLighting(LightingChange {
            brightness: Some(20),
            ..
        })
    ));
    assert_eq!(state.light_controls().brightness, Some((5, 6)));
    assert!(
        state.adjust_brightness(5).is_none(),
        "no change, no command"
    );
    state.adjust_speed(4).unwrap();
    assert_eq!(state.light_controls().speed, Some((4, 0, 5)));

    let mut answer = lighting(LightMode::Effect(2));
    answer.brightness = Some(20);
    answer.speed = Some(4);
    state.apply(Event::Lighting(answer), &mut settings);
    assert_eq!(state.light_controls().brightness, Some((5, 6)));

    // And a refusal puts the slider back where the keyboard has it.
    state.adjust_brightness(0).unwrap();
    state.apply(Event::Error(Problem::Busy), &mut settings);
    assert_eq!(state.light_controls().brightness, Some((5, 6)));
}

#[test]
fn a_brightness_step_the_model_lacks_does_nothing() {
    let (mut state, _) = with_lights(LightMode::Effect(2));
    assert!(state.adjust_brightness(6).is_none());
}

#[test]
fn palettes_go_into_the_settings_and_get_saved() {
    let (mut state, mut settings) = with_lights(LightMode::Effect(2));
    assert!(state.custom_color_resets_others(&settings));
    let mut palettes = Palettes::default();
    palettes.set(2, 0, Rgb::new(1, 2, 3)).unwrap();
    let reaction = state.apply(
        Event::PalettesChanged {
            pid: s85().pid,
            palettes: Box::new(palettes.clone()),
        },
        &mut settings,
    );
    assert!(reaction.save_settings);
    assert_eq!(settings.palettes[&s85().pid], palettes);
    assert!(!state.custom_color_resets_others(&settings), "only once");
    assert_eq!(state.slot_colors(&settings)[0], Rgb::new(1, 2, 3));
}

#[test]
fn without_a_saved_copy_the_colours_shown_are_the_defaults() {
    let (state, settings) = with_lights(LightMode::Effect(2));
    let colors = state.slot_colors(&settings);
    assert_eq!(colors[0], Rgb::new(255, 0, 0));
    assert_eq!(colors[6], Rgb::new(255, 255, 255));
}

#[test]
fn warnings_stay_for_as_long_as_they_are_true() {
    let (mut state, mut settings) = connected();
    assert!(state.banners().is_empty());
    state.apply(Event::RkAppRunning(true), &mut settings);
    state.apply(Event::Hung, &mut settings);
    let texts: Vec<String> = state.banners().into_iter().map(|b| b.text).collect();
    assert_eq!(texts, [t().hung, t().rk_app_warning]);
    assert_eq!(state.status().text, t().not_responding);
    state.apply(Event::Responding, &mut settings);
    state.apply(Event::RkAppRunning(false), &mut settings);
    assert!(state.banners().is_empty());
    assert_eq!(state.status().text, t().connected);
}

#[test]
fn the_clock_being_set_is_passing_news() {
    let (mut state, mut settings) = connected();
    state.apply(Event::ClockSynced, &mut settings);
    let message = state.message.as_ref().unwrap();
    assert_eq!(message.text, t().clock_synced);
    assert!(message.fades());
    assert!(!Note::new("x", Tone::Bad).fades());
}

#[test]
fn a_report_is_handed_over() {
    let (mut state, mut settings) = connected();
    let reaction = state.apply(
        Event::ReportReady(ReportData {
            model: Some(s85()),
            config: Err("no".to_string()),
            errors: Vec::new(),
        }),
        &mut settings,
    );
    assert!(reaction.report.is_some());
}

#[test]
fn the_preview_makes_room_for_the_models_screen() {
    assert_eq!(State::default().preview_height(), 172);
    let mut state = State::default();
    state.apply(
        Event::Connected {
            model: model::find(0x01B8).unwrap(),
        },
        &mut Settings::default(),
    );
    assert_eq!(state.preview_height(), 240);
}

// What the code review found.

#[test]
fn a_cancel_before_the_worker_starts_the_upload_counts() {
    let (mut state, mut settings) = with_image(1, 100);
    state.start_upload().unwrap();
    state.apply(Event::Cancelling, &mut settings);
    assert!(state.upload.is_cancelling());
    assert!(state.cancel_upload().is_none(), "already asked");
    assert_eq!(state.screen_notes()[0].text, t().cancelling);
    state.apply(Event::UploadCancelled, &mut settings);
    assert!(state.upload.is_idle());
}

#[test]
fn a_cancel_stays_when_the_upload_starts_after_all() {
    let (mut state, mut settings) = with_image(1, 100);
    state.start_upload().unwrap();
    state.apply(Event::Cancelling, &mut settings);
    state.apply(
        Event::UploadStarted {
            total: 100,
            estimate: Duration::from_secs(40),
        },
        &mut settings,
    );
    assert!(state.upload.is_cancelling());
}

#[test]
fn a_bad_file_during_an_upload_doesnt_hide_how_the_upload_went() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.choose_file(PathBuf::from("bad.gif"));
    state.apply(
        Event::ImageFailed(Error::Image("not a picture".to_string())),
        &mut settings,
    );
    state.apply(Event::UploadDone, &mut settings);
    let texts: Vec<String> = state.screen_notes().into_iter().map(|n| n.text).collect();
    assert!(
        texts.iter().any(|text| text == t().upload_done),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("not a picture")),
        "{texts:?}"
    );
}

#[test]
fn an_upload_ending_after_an_unconfirmed_finish_says_only_that() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(Event::UploadUnconfirmed, &mut settings);
    state.apply(Event::UploadDone, &mut settings);
    state.start_upload().unwrap();
    state.apply(Event::UploadDone, &mut settings);
    assert_eq!(state.outcome.as_ref().unwrap().text, t().upload_done);
}

#[test]
fn losing_the_keyboard_ends_an_upload() {
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(Event::Disconnected, &mut settings);
    assert!(state.upload.is_idle());
    let (mut state, mut settings) = with_image(1, 100);
    running(&mut state, &mut settings);
    state.apply(
        Event::CannotOpen(Error::Hid("x".to_string())),
        &mut settings,
    );
    assert!(state.upload.is_idle());
}

#[test]
fn losing_the_keyboard_forgets_slider_values_on_their_way() {
    let (mut state, mut settings) = with_lights(LightMode::Effect(2));
    state.adjust_brightness(5).unwrap();
    state.apply(Event::Disconnected, &mut settings);
    state.apply(Event::Connected { model: s85() }, &mut settings);
    state.apply(
        Event::Lighting(lighting(LightMode::Effect(2))),
        &mut settings,
    );
    assert_eq!(state.light_controls().brightness, Some((2, 6)));
}

#[test]
fn a_speed_goes_as_an_adjustment_that_settles_not_at_once() {
    let (mut state, _) = with_lights(LightMode::Effect(2));
    assert!(matches!(
        state.adjust_speed(4),
        Some(Cmd::AdjustLighting(LightingChange {
            speed: Some(4),
            brightness: None,
            ..
        }))
    ));
}

#[test]
fn the_speed_it_already_is_sends_nothing() {
    let (mut state, _) = with_lights(LightMode::Effect(2));
    assert!(state.adjust_speed(2).is_none(), "as the keyboard has it");
    state.adjust_speed(4).unwrap();
    assert!(state.adjust_speed(4).is_none(), "as the slider has it");
}

#[test]
fn what_the_keyboard_says_replaces_what_a_slider_had_on_its_way() {
    let (mut state, mut settings) = with_lights(LightMode::Effect(2));
    state.adjust_brightness(5).unwrap();
    state.adjust_speed(4).unwrap();
    // Something else changed them meanwhile: the keyboard's word is the last one.
    state.apply(
        Event::Lighting(lighting(LightMode::Effect(2))),
        &mut settings,
    );
    assert_eq!(state.light_controls().brightness, Some((2, 6)));
    assert_eq!(state.light_controls().speed, Some((2, 0, 5)));
}

#[test]
fn a_slider_can_be_moved_only_while_the_lights_can_change() {
    let (mut state, mut settings) = with_image(1, 100);
    state.apply(
        Event::Lighting(lighting(LightMode::Effect(2))),
        &mut settings,
    );
    running(&mut state, &mut settings);
    assert!(state.adjust_brightness(5).is_none());
    assert!(state.adjust_speed(4).is_none());
    assert_eq!(state.light_controls().brightness, Some((2, 6)), "no change");
    assert_eq!(state.light_controls().speed, Some((2, 0, 5)), "no change");
}

#[test]
fn a_reload_keeps_the_old_picture_but_not_the_button() {
    let (mut state, _) = with_image(1, 100);
    assert!(state.can_upload());
    state.set_fit(Fit::Stretch).unwrap();
    assert!(state.animation().is_some(), "still showing");
    assert!(!state.can_upload(), "what would go up is on its way");
    assert!(state.start_upload().is_none());
}

#[test]
fn arrows_move_a_slider_one_step_the_way_they_say() {
    let (mut state, _) = with_lights(LightMode::Effect(2));
    state.nudge(Slider::Brightness, 1).unwrap();
    assert_eq!(state.light_controls().brightness, Some((3, 6)));
    state.nudge(Slider::Brightness, -1).unwrap();
    state.nudge(Slider::Brightness, -1).unwrap();
    assert_eq!(state.light_controls().brightness, Some((1, 6)));
    state.nudge(Slider::Speed, 1).unwrap();
    assert_eq!(state.light_controls().speed, Some((3, 0, 5)));
    state.nudge(Slider::Speed, -1).unwrap();
    assert_eq!(state.light_controls().speed, Some((2, 0, 5)));
}

#[test]
fn arrows_stop_at_a_sliders_ends_with_nothing_to_send() {
    let (mut state, _) = with_lights(LightMode::Effect(2));
    state.adjust_brightness(5).unwrap();
    assert!(state.nudge(Slider::Brightness, 1).is_none());
    state.adjust_speed(0).unwrap();
    assert!(state.nudge(Slider::Speed, -1).is_none());
}

#[test]
fn an_effect_without_a_slider_has_nothing_to_move() {
    // Solid has no speed.
    let (mut state, _) = with_lights(LightMode::Effect(1));
    assert!(state.nudge(Slider::Speed, 1).is_none());
    assert!(state.drag_slider(Slider::Speed, 50.0, 0.0, 100.0).is_none());
    let (mut state, _) = with_lights(LightMode::Off);
    assert!(state.nudge(Slider::Brightness, 1).is_none());
    assert!(
        state
            .drag_slider(Slider::Brightness, 50.0, 0.0, 100.0)
            .is_none()
    );
}

#[test]
fn dragging_picks_the_nearest_step_along_the_track() {
    let (mut state, _) = with_lights(LightMode::Effect(2));
    assert!(matches!(
        state.drag_slider(Slider::Brightness, 100.0, 0.0, 100.0),
        Some(Cmd::AdjustLighting(LightingChange {
            brightness: Some(20),
            ..
        }))
    ));
    assert_eq!(state.light_controls().brightness, Some((5, 6)));
    assert!(matches!(
        state.drag_slider(Slider::Speed, 0.0, 0.0, 100.0),
        Some(Cmd::AdjustLighting(LightingChange { speed: Some(0), .. }))
    ));
}

#[test]
fn dragging_a_disabled_slider_does_nothing() {
    let (mut state, mut settings) = with_image(1, 100);
    state.apply(
        Event::Lighting(lighting(LightMode::Effect(2))),
        &mut settings,
    );
    running(&mut state, &mut settings);
    assert!(
        state
            .drag_slider(Slider::Brightness, 100.0, 0.0, 100.0)
            .is_none()
    );
    assert!(
        state
            .drag_slider(Slider::Speed, 100.0, 0.0, 100.0)
            .is_none()
    );
    assert_eq!(state.light_controls().brightness, Some((2, 6)));
}

#[test]
fn a_sliders_value_reads_as_a_percentage() {
    assert_eq!(percent(0, 6), 0);
    assert_eq!(percent(3, 6), 60);
    assert_eq!(percent(5, 6), 100);
    assert_eq!(percent(0, 1), 0);
}

#[test]
fn the_preview_shows_the_frame_that_is_due_and_goes_around() {
    assert_eq!(preview_frame(0, 4, 100), 0);
    assert_eq!(preview_frame(99, 4, 100), 0);
    assert_eq!(preview_frame(100, 4, 100), 1);
    assert_eq!(preview_frame(399, 4, 100), 3);
    assert_eq!(preview_frame(400, 4, 100), 0);
    assert_eq!(preview_frame(5000, 1, 100), 0);
    assert_eq!(preview_frame(5000, 0, 100), 0);
    assert_eq!(
        preview_frame(5, 4, 0),
        1,
        "a delay of 0 doesn't divide by 0"
    );
}

#[test]
fn the_preview_doesnt_look_more_often_than_it_can_show() {
    assert_eq!(preview_interval(100), 100);
    assert_eq!(preview_interval(20), PREVIEW_MIN_MS);
}

#[test]
fn a_settings_file_that_cant_be_read_stays_on_show() {
    let settings = Settings {
        read_error: Some("no access".to_string()),
        ..Settings::default()
    };
    let state = State::with_settings(&settings);
    let banners = state.banners();
    assert_eq!(banners.len(), 1);
    assert_eq!(banners[0].text, t().settings_unreadable);
    assert_eq!(banners[0].tone, Tone::Warning);
    assert!(
        State::with_settings(&Settings::default())
            .banners()
            .is_empty()
    );
}

#[test]
fn a_file_chosen_with_no_keyboard_says_it_is_waiting() {
    let mut state = State::default();
    state.apply(Event::Disconnected, &mut Settings::default());
    let hint = state.empty_view().detail;
    state.choose_file(PathBuf::from("C:/somewhere/cat.gif"));
    let view = state.empty_view();
    assert_ne!(view.detail, hint);
    assert!(view.detail.contains("cat.gif"), "{}", view.detail);
    assert!(!view.detail.contains("somewhere"), "{}", view.detail);
}

#[test]
fn a_keyboard_that_cant_be_opened_doesnt_ask_to_connect_it() {
    let mut state = State::default();
    state.apply(
        Event::CannotOpen(Error::Hid("denied".to_string())),
        &mut Settings::default(),
    );
    let view = state.empty_view();
    assert_eq!(view.headline, t().cannot_open);
    assert_ne!(view.headline, t().connect_keyboard);
    assert!(view.detail.contains("denied"), "{}", view.detail);
    assert_eq!(view.tone, Tone::Bad);
    assert!(!state.offers_report());
}

#[test]
fn an_unsupported_keyboard_says_so_and_offers_a_report() {
    let mut state = State::default();
    state.apply(
        Event::CannotOpen(Error::Unsupported(0x002A)),
        &mut Settings::default(),
    );
    let view = state.empty_view();
    assert_eq!(view.headline, t().unsupported_title);
    assert!(view.detail.contains("002A"), "{}", view.detail);
    assert_eq!(view.tone, Tone::Warning);
    assert!(state.offers_report());
    assert_eq!(state.status().text, t().unsupported_short);
}

#[test]
fn looking_is_what_shows_before_the_worker_has_said() {
    assert_eq!(State::default().empty_view().detail, t().looking);
    assert_eq!(State::default().empty_view().headline, t().connect_keyboard);
}

#[test]
fn a_tested_or_connected_keyboard_needs_no_new_view_of_its_own() {
    let (state, _) = connected();
    assert!(state.offers_report(), "every model is experimental for now");
}
