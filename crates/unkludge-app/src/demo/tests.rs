use super::*;
use unkludge_protocol::config::{Config, SleepTime};
use unkludge_protocol::lighting::{
    self, ColorChoice, LightMode, LightingChange, ModeChoice, Palettes, Rgb, SleepSetting,
};
use unkludge_protocol::model;
use unkludge_protocol::screen::{Animation, LoadOptions};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn s85() -> &'static Model {
    model::find(0x0265).unwrap()
}

fn keyboard(state: DemoState) -> Arc<DemoKeyboard> {
    DemoKeyboard::new(s85(), state)
}

fn animation(model: &'static Model, frames: usize) -> Animation {
    Animation::from_bytes(
        &gif_bytes(frames, 200),
        &model.screen.unwrap(),
        LoadOptions::default(),
    )
    .unwrap()
}

#[test]
fn it_opens_for_every_supported_model_and_its_settings_read_well() {
    for model in model::supported() {
        let keyboard = DemoKeyboard::new(model, DemoState::Connected);
        let mut device = keyboard.open().unwrap();
        let current = lighting::read(&mut device).unwrap();
        assert_eq!(current.mode, LightMode::Effect(2), "{}", model.name);
        assert_eq!(current.sleep, SleepSetting::Known(SleepTime::Min10));
    }
}

#[test]
fn with_no_keyboard_there_is_nothing_to_open() {
    let keyboard = keyboard(DemoState::NoKeyboard);
    assert_eq!(keyboard.open().err(), Some(Error::NotFound));
    assert!(!keyboard.rk_app_running());
    assert!(DemoKeyboard::new(s85(), DemoState::NoKeyboardRk).rk_app_running());
}

#[test]
fn a_keyboard_this_version_cant_use_says_so() {
    let keyboard = keyboard(DemoState::Unsupported);
    assert_eq!(
        keyboard.open().err(),
        Some(Error::Unsupported(UNSUPPORTED_PID))
    );
    assert!(!model::find(UNSUPPORTED_PID).unwrap().is_supported());
}

#[test]
fn a_keyboard_with_no_usable_collection_says_so() {
    let keyboard = keyboard(DemoState::NoCollection);
    assert_eq!(
        keyboard.open().err(),
        Some(Error::NoUsableCollection(s85().pid))
    );
}

#[test]
fn a_keyboard_a_command_has_says_so() {
    let keyboard = keyboard(DemoState::InUse);
    assert_eq!(keyboard.open().err(), Some(Error::DeviceBusy));
}

#[test]
fn a_keyboard_that_cant_be_opened_says_so() {
    let keyboard = keyboard(DemoState::CannotOpen);
    assert!(matches!(keyboard.open(), Err(Error::Hid(_))));
}

#[test]
fn lighting_changes_are_what_it_reads_back_afterwards() {
    let keyboard = keyboard(DemoState::Connected);
    let mut device = keyboard.open().unwrap();
    let mut palettes = Palettes::default();
    let change = LightingChange {
        mode: Some(ModeChoice::Effect(5)),
        brightness: Some(20),
        sleep: Some(SleepTime::Min20),
        ..LightingChange::default()
    };
    lighting::apply(&mut device, &change, &mut palettes).unwrap();
    let current = lighting::read(&mut device).unwrap();
    assert_eq!(current.mode, LightMode::Effect(5));
    assert_eq!(current.brightness, Some(20));
    assert_eq!(current.sleep, SleepSetting::Known(SleepTime::Min20));

    // And it's still there on opening it again.
    let mut again = keyboard.open().unwrap();
    assert_eq!(
        lighting::read(&mut again).unwrap().mode,
        LightMode::Effect(5)
    );
    assert_eq!(Config::read(&mut again).unwrap().effect(), 5);
}

#[test]
fn a_custom_colour_keeps_the_palette_with_the_caller() {
    let keyboard = keyboard(DemoState::Connected);
    let mut device = keyboard.open().unwrap();
    let mut palettes = Palettes::default();
    let change = LightingChange {
        color: Some(ColorChoice::Custom(Rgb::new(1, 2, 3))),
        ..LightingChange::default()
    };
    lighting::apply(&mut device, &change, &mut palettes).unwrap();
    assert_eq!(palettes.get(2, 0), Some(Rgb::new(1, 2, 3)));
}

#[test]
fn its_states_show_as_the_lights_they_name() {
    let read = |state| {
        let keyboard = keyboard(state);
        let mut device = keyboard.open().unwrap();
        lighting::read(&mut device).unwrap()
    };
    assert_eq!(read(DemoState::LightsOff).mode, LightMode::Off);
    assert_eq!(read(DemoState::Solid).mode, LightMode::Effect(1));
    assert_eq!(read(DemoState::CustomMode).mode, LightMode::Custom);
    assert_eq!(
        read(DemoState::OtherSleep).sleep,
        SleepSetting::Unknown(0x1E)
    );
}

#[test]
fn an_upload_is_acknowledged_to_its_end_for_every_screen_kind() {
    for pid in [0x0265, 0x01A2, 0x01B8, 0x0187] {
        let model = model::find(pid).unwrap();
        let keyboard = DemoKeyboard::new(model, DemoState::Done);
        let mut device = keyboard.open().unwrap();
        let animation = animation(model, 1);
        let outcome = animation
            .upload(&mut device, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(outcome.finish_confirmed, "{}", model.name);
    }
}

#[test]
fn a_keyboard_told_to_can_leave_the_finish_unconfirmed() {
    let keyboard = keyboard(DemoState::Unconfirmed);
    let mut device = keyboard.open().unwrap();
    // Waits 8 s for the finish, as a real one would have it.
    let outcome = animation(s85(), 1)
        .upload(&mut device, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(!outcome.finish_confirmed);
}

#[test]
fn a_keyboard_told_to_refuses_chunks() {
    let keyboard = keyboard(DemoState::Failed);
    keyboard.refusing.store(true, Ordering::SeqCst);
    let mut device = keyboard.open().unwrap();
    let result = animation(s85(), 1).upload(&mut device, &AtomicBool::new(false), |_| {});
    assert!(matches!(result, Err(Error::Rejected { .. })), "{result:?}");
}

#[test]
fn its_pauses_are_short() {
    let keyboard = keyboard(DemoState::Connected);
    let start = Instant::now();
    DemoTransport::new(keyboard).sleep(Duration::from_secs(30));
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn every_state_has_a_name_that_reads_back() {
    assert_eq!(DemoState::names().len(), STATES.len());
    for (name, state) in STATES {
        assert_eq!(DemoState::parse(name), Some(state));
    }
    assert_eq!(DemoState::parse("nope"), None);
}

#[test]
fn the_gif_decodes_to_all_its_frames_at_its_delay() {
    for frames in [1, 5] {
        let animation = animation(s85(), frames);
        assert_eq!(animation.frames().len(), frames);
        assert_eq!(animation.delay_ms(), 200);
        assert!(!animation.truncated());
    }
}

#[test]
fn the_gif_moves_from_frame_to_frame() {
    let animation = animation(s85(), 4);
    assert_ne!(animation.frames()[0], animation.frames()[1]);
}

#[test]
fn the_gif_has_the_name_in_white() {
    let bright = |x, y| pixel(x, y, 0) == WHITE;
    assert!((0..GIF_WIDTH).any(|x| bright(x, GIF_HEIGHT / 2)));
    assert!(!bright(0, 0));
}

/// The whole way round, with no real device anywhere: the worker takes the demo's keyboard as
/// its own, and a change comes back as the keyboard reads it.
#[test]
fn the_worker_runs_on_it_as_on_a_real_keyboard() {
    let keyboard = keyboard(DemoState::Connected);
    let looking = Arc::clone(&keyboard);
    let (worker, events) = crate::worker::spawn(
        move || looking.open(),
        crate::worker::Options {
            auto_clock: true,
            palettes: Default::default(),
            rk_app_running: Box::new(|| false),
        },
        Arc::new(|| {}),
    );
    let next = || events.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut seen = next();
    while !matches!(seen, Event::Connected { .. }) {
        seen = next();
    }
    while !matches!(seen, Event::ClockSynced) {
        seen = next();
    }
    worker.send(crate::worker::Cmd::SetLighting(LightingChange {
        mode: Some(ModeChoice::Effect(5)),
        ..LightingChange::default()
    }));
    while !matches!(&seen, Event::Lighting(current) if current.mode == LightMode::Effect(5)) {
        seen = next();
    }
    worker.shutdown();
    assert_eq!(keyboard.config()[EFFECT], 5);
}

/// A folder of the test's own, gone at the end.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("unkludge-demo-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    /// The name the picture tries first.
    fn first_name(&self) -> PathBuf {
        let name = format!("unkludge-demo-{}-0.gif", std::process::id());
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// With nowhere to write, for the states that need no picture.
fn script(state: DemoState) -> Script {
    Script::new(state, keyboard(state), None)
}

fn lights() -> Event {
    Event::Lighting(lighting::CurrentLighting {
        mode: LightMode::Effect(2),
        brightness: Some(8),
        speed: Some(1),
        color: None,
        sleep: SleepSetting::Known(SleepTime::Min10),
    })
}

fn progress(done: usize) -> Event {
    Event::Progress {
        done,
        total: 100,
        remaining: Duration::from_secs(10),
    }
}

#[test]
fn a_plain_state_is_reached_with_the_lights() {
    let mut script = script(DemoState::Connected);
    assert!(!script.reached());
    assert_eq!(script.on_event(&lights()), []);
    assert!(script.reached());
}

#[test]
fn the_hung_state_is_pretended_once() {
    let mut script = script(DemoState::Hung);
    assert_eq!(script.on_event(&lights()), [Step::Pretend(Event::Hung)]);
    assert_eq!(script.on_event(&lights()), []);
}

#[test]
fn a_state_with_no_keyboard_is_reached_when_none_is_found() {
    let mut script = script(DemoState::NoKeyboard);
    script.on_event(&Event::Disconnected);
    assert!(script.reached());
}

#[test]
fn the_picture_goes_in_the_folder_it_is_given() {
    let dir = TempDir::new("folder");
    let path = write_gif(&dir.0, 1, 100).unwrap();
    assert_eq!(path.parent(), Some(dir.0.as_path()));
    assert_eq!(std::fs::read(&path).unwrap(), gif_bytes(1, 100));
}

#[test]
fn the_picture_makes_its_folder() {
    let dir = TempDir::new("makes-folder");
    let deeper = dir.0.join("deeper");
    assert!(write_gif(&deeper, 1, 100).unwrap().starts_with(&deeper));
}

#[test]
fn a_file_already_under_the_pictures_name_stays_as_it_is() {
    let dir = TempDir::new("existing");
    std::fs::write(dir.first_name(), "someone else's").unwrap();
    let path = write_gif(&dir.0, 1, 100).unwrap();
    assert_ne!(path, dir.first_name());
    assert_eq!(
        std::fs::read_to_string(dir.first_name()).unwrap(),
        "someone else's"
    );
}

#[cfg(unix)]
#[test]
fn a_link_under_the_pictures_name_isnt_written_through() {
    let dir = TempDir::new("link");
    let target = dir.0.join("target");
    std::fs::write(&target, "someone else's").unwrap();
    std::os::unix::fs::symlink(&target, dir.first_name()).unwrap();
    let path = write_gif(&dir.0, 1, 100).unwrap();
    assert_ne!(path, dir.first_name());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "someone else's");
}

#[cfg(unix)]
#[test]
fn a_link_with_nothing_behind_it_isnt_written_through_either() {
    let dir = TempDir::new("dangling");
    let target = dir.0.join("not-there-yet");
    std::os::unix::fs::symlink(&target, dir.first_name()).unwrap();
    write_gif(&dir.0, 1, 100).unwrap();
    assert!(!target.exists());
}

#[test]
fn with_every_name_taken_it_gives_up_and_leaves_them_alone() {
    let dir = TempDir::new("all-taken");
    for attempt in 0..GIF_NAMES {
        let name = format!("unkludge-demo-{}-{attempt}.gif", std::process::id());
        std::fs::write(dir.0.join(name), "taken").unwrap();
    }
    let error = write_gif(&dir.0, 1, 100).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read_to_string(dir.first_name()).unwrap(), "taken");
}

#[test]
fn with_nowhere_to_write_an_image_state_chooses_nothing() {
    let mut script = script(DemoState::Image);
    assert_eq!(script.on_event(&lights()), []);
    assert!(!script.reached());
}

#[test]
fn the_end_removes_only_the_file_it_made() {
    let dir = TempDir::new("only-its-own");
    std::fs::write(dir.first_name(), "someone else's").unwrap();
    let path = {
        let keyboard = keyboard(DemoState::Image);
        let mut script = Script::new(DemoState::Image, keyboard, Some(dir.0.clone()));
        let steps = script.on_event(&lights());
        let [Step::ChooseFile(path)] = steps.as_slice() else {
            panic!("{steps:?}");
        };
        path.clone()
    };
    assert!(!path.exists());
    assert!(dir.first_name().exists());
}

#[test]
fn an_image_state_chooses_its_file_once_and_removes_it_at_the_end() {
    let dir = TempDir::new("image-state");
    let path = {
        let keyboard = keyboard(DemoState::Image);
        let mut script = Script::new(DemoState::Image, keyboard, Some(dir.0.clone()));
        let steps = script.on_event(&lights());
        let [Step::ChooseFile(path)] = steps.as_slice() else {
            panic!("{steps:?}");
        };
        assert!(path.exists());
        assert_eq!(script.on_event(&lights()), []);
        assert!(!script.reached(), "not until it's decoded");
        path.clone()
    };
    assert!(!path.exists());
}

#[test]
fn the_uploading_state_slows_down_to_be_seen_partway() {
    let mut script = script(DemoState::Uploading);
    script.on_event(&progress(41));
    assert!(!script.reached());
    script.on_event(&progress(42));
    assert!(script.reached());
    assert_eq!(script.keyboard.chunk_ms.load(Ordering::SeqCst), 150);
}

#[test]
fn the_report_state_asks_once_and_waits_for_it() {
    let mut script = script(DemoState::Report);
    assert_eq!(script.on_event(&lights()), [Step::Report]);
    assert_eq!(script.on_event(&lights()), []);
    assert!(!script.reached());
    script.on_event(&Event::ReportReady(crate::worker::ReportData {
        model: None,
        config: Err("no".to_string()),
        errors: Vec::new(),
    }));
    assert!(script.reached());
}

#[test]
fn only_the_focus_state_presses_tab() {
    assert!(script(DemoState::Focus).tabs_before_shot() > 0);
    assert_eq!(script(DemoState::Connected).tabs_before_shot(), 0);
}

#[test]
fn a_stalled_keyboard_uses_up_the_wait_it_is_given() {
    let keyboard = keyboard(DemoState::Connected);
    keyboard.stalled.store(true, Ordering::SeqCst);
    let transport = DemoTransport::new(keyboard);
    let start = Instant::now();
    assert_eq!(transport.read_timeout(&mut [0; 8], 120), Ok(0));
    assert!(start.elapsed() >= Duration::from_millis(100));
}

#[test]
fn a_cancelling_state_stalls_the_keyboard_and_asks_to_cancel_partway() {
    let mut script = script(DemoState::Cancelling);
    assert_eq!(script.on_event(&progress(3)), []);
    assert_eq!(script.on_event(&progress(12)), [Step::Cancel]);
    assert!(script.keyboard.stalled.load(Ordering::SeqCst));
    assert_eq!(script.on_event(&progress(13)), []);
    assert!(!script.reached());
    script.on_event(&Event::Cancelling);
    assert!(script.reached());
}

#[test]
fn an_end_event_is_reached_for_the_states_that_run_to_the_end() {
    for end in [
        Event::UploadDone,
        Event::UploadCancelled,
        Event::UploadFailed(Error::StartNoAck),
    ] {
        let mut script = script(DemoState::Done);
        script.on_event(&end);
        assert!(script.reached());
    }
}
