use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;
use crate::demo::{DemoState, DemoTransport};

/// Keeps a list of what was asked of it, and answers `enabled` with what it holds.
#[derive(Default)]
struct FakeAutostart {
    calls: RefCell<Vec<String>>,
    on: RefCell<bool>,
}

impl Autostart for FakeAutostart {
    fn set(&self, on: bool) {
        self.calls.borrow_mut().push(format!("set {on}"));
        *self.on.borrow_mut() = on;
    }

    fn enabled(&self) -> bool {
        self.calls.borrow_mut().push("enabled".to_string());
        *self.on.borrow()
    }

    fn refresh(&self, force: bool) {
        self.calls.borrow_mut().push(format!("refresh {force}"));
    }
}

impl FakeAutostart {
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

/// A folder of the test's own, gone at the end.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir =
            std::env::temp_dir().join(format!("unkludge-session-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn demo() -> Session {
    Session::new(Some(DemoArgs::default()))
}

fn data() -> ReportData {
    ReportData {
        model: model::find(0x0265),
        config: Err("no keyboard connected".to_string()),
        errors: Vec::new(),
    }
}

// Settings.

#[test]
fn no_demo_arguments_is_the_real_app() {
    assert_eq!(Session::new(None), Session::Real);
    assert!(!Session::Real.is_demo());
    assert!(demo().is_demo());
}

#[test]
fn a_demo_keeps_its_settings_apart() {
    let dir = TempDir::new("demo-apart");
    let settings = Settings {
        auto_clock: false,
        ..Settings::default()
    };
    demo().save_settings(&settings, Some(&dir.0)).unwrap();
    assert_eq!(dir.names(), ["settings-demo.txt"]);
}

#[test]
fn a_demo_doesnt_read_the_real_settings() {
    let dir = TempDir::new("demo-reads");
    std::fs::write(dir.0.join("settings.txt"), "auto_clock=0\n").unwrap();
    let loaded = demo().load_settings(Some(&dir.0), &FakeAutostart::default(), true);
    assert!(loaded.auto_clock, "the real file says off");
}

#[test]
fn the_real_app_uses_the_real_settings() {
    let dir = TempDir::new("real");
    std::fs::write(dir.0.join("settings.txt"), "auto_clock=0\n").unwrap();
    std::fs::write(dir.0.join("settings-demo.txt"), "auto_clock=1\n").unwrap();
    let loaded = Session::Real.load_settings(Some(&dir.0), &FakeAutostart::default(), true);
    assert!(!loaded.auto_clock);

    Session::Real
        .save_settings(&Settings::default(), Some(&dir.0))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.0.join("settings.txt")).unwrap(),
        "auto_clock=1\n"
    );
}

#[test]
fn saving_with_no_data_folder_is_an_error_and_nothing_else() {
    let error = demo()
        .save_settings(&Settings::default(), None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

// Starting with the system.

#[test]
fn a_demo_never_touches_the_systems_entry_on_loading() {
    let dir = TempDir::new("demo-entry");
    let autostart = FakeAutostart::default();
    // A first launch, of what looks like an installed copy.
    demo().load_settings(Some(&dir.0), &autostart, true);
    // And a later one.
    demo().load_settings(Some(&dir.0), &autostart, true);
    assert_eq!(autostart.calls(), Vec::<String>::new());
}

#[test]
fn a_demo_only_pretends_to_start_with_the_system() {
    let autostart = FakeAutostart::default();
    assert!(demo().set_autostart(true, &autostart));
    assert!(!demo().set_autostart(false, &autostart));
    assert!(demo().autostart_now(&autostart, true));
    assert!(!demo().autostart_now(&autostart, false));
    assert_eq!(autostart.calls(), Vec::<String>::new());
}

#[test]
fn the_real_app_asks_the_entry_and_believes_it() {
    let autostart = FakeAutostart::default();
    assert!(Session::Real.set_autostart(true, &autostart));
    assert_eq!(autostart.calls(), ["set true", "enabled"]);
    // Whatever it had last thought: the entry can change from outside.
    assert!(Session::Real.autostart_now(&autostart, false));
    *autostart.on.borrow_mut() = false;
    assert!(!Session::Real.autostart_now(&autostart, true));
}

#[test]
fn a_first_launch_of_an_installed_copy_starts_with_the_system_and_saves() {
    let dir = TempDir::new("first");
    let autostart = FakeAutostart::default();
    let settings = Session::Real.load_settings(Some(&dir.0), &autostart, true);
    assert!(!settings.existed);
    assert_eq!(autostart.calls(), ["set true"]);
    assert_eq!(
        dir.names(),
        ["settings.txt"],
        "so that the next isn't a first"
    );
}

#[test]
fn a_first_launch_of_a_build_doesnt_start_with_the_system() {
    let dir = TempDir::new("first-build");
    let autostart = FakeAutostart::default();
    Session::Real.load_settings(Some(&dir.0), &autostart, false);
    assert_eq!(autostart.calls(), Vec::<String>::new());
}

#[test]
fn a_later_launch_only_checks_where_the_entry_points() {
    let dir = TempDir::new("later");
    std::fs::write(dir.0.join("settings.txt"), "auto_clock=1\n").unwrap();
    let autostart = FakeAutostart::default();
    Session::Real.load_settings(Some(&dir.0), &autostart, true);
    assert_eq!(autostart.calls(), ["refresh false"]);
}

// One copy at a time.

#[test]
fn only_the_real_app_claims_the_single_instance() {
    assert!(Session::Real.claims_single_instance());
    assert!(!demo().claims_single_instance());
}

#[test]
fn a_copy_started_hidden_doesnt_bring_up_the_others_window() {
    assert!(Session::shows_other_copy(false));
    assert!(!Session::shows_other_copy(true));
}

#[test]
fn a_demo_is_scripted_only_with_somewhere_to_put_its_picture() {
    assert!(!demo().is_scripted());
    assert!(!Session::Real.is_scripted());
    let scripted = Session::new(Some(DemoArgs {
        shot: Some(PathBuf::from("a.png")),
        ..DemoArgs::default()
    }));
    assert!(scripted.is_scripted());
}

// The keyboard.

/// Counts how many times the real keyboard was asked for, and has none to give.
fn real_opener(
    counter: &Arc<AtomicUsize>,
) -> impl FnMut() -> Result<Device<DemoTransport>, Error> + Send + 'static {
    let counter = Arc::clone(counter);
    move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Err(Error::NotFound)
    }
}

/// The first few things the worker says.
fn next_events(events: &Receiver<Event>, how_many: usize) -> Vec<Event> {
    (0..how_many)
        .filter_map(|_| events.recv_timeout(Duration::from_secs(10)).ok())
        .collect()
}

#[test]
fn a_demo_never_opens_the_real_keyboard() {
    let opened = Arc::new(AtomicUsize::new(0));
    let running = demo()
        .spawn_worker(
            &Settings::default(),
            None,
            Arc::new(|| {}),
            real_opener(&opened),
            Box::new(|| panic!("a demo doesn't look for the real program")),
        )
        .unwrap();
    let events = next_events(&running.events, 3);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Connected { .. })),
        "{events:?}"
    );
    assert!(running.script.is_some());
    running.worker.shutdown();
    assert_eq!(opened.load(Ordering::SeqCst), 0);
}

#[test]
fn a_demo_shows_the_model_and_state_asked_for() {
    let session = Session::new(Some(DemoArgs {
        pid: 0x01A2,
        state: DemoState::RkApp,
        ..DemoArgs::default()
    }));
    let running = session
        .spawn_worker(
            &Settings::default(),
            None,
            Arc::new(|| {}),
            real_opener(&Arc::default()),
            Box::new(|| false),
        )
        .unwrap();
    let events = next_events(&running.events, 2);
    assert!(events.contains(&Event::RkAppRunning(true)), "{events:?}");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Connected { model } if model.pid == 0x01A2
        )),
        "{events:?}"
    );
    running.worker.shutdown();
}

#[test]
fn the_real_app_opens_what_it_is_given_to_open() {
    let opened = Arc::new(AtomicUsize::new(0));
    let running = Session::Real
        .spawn_worker(
            &Settings::default(),
            None,
            Arc::new(|| {}),
            real_opener(&opened),
            Box::new(|| false),
        )
        .unwrap();
    let events = next_events(&running.events, 2);
    assert!(events.contains(&Event::Disconnected), "{events:?}");
    assert!(running.script.is_none());
    running.worker.shutdown();
    assert!(opened.load(Ordering::SeqCst) > 0);
}

fn collection(pid: u16) -> HidCollection {
    HidCollection {
        pid,
        usage_page: 0xFF02,
        usage: 2,
        interface: 1,
    }
}

#[test]
fn a_device_this_version_cant_use_is_found_in_the_list() {
    // A model it knows of and doesn't support, and one it doesn't know at all.
    assert_eq!(unsupported_device(&[collection(0x002A)]), Some(0x002A));
    assert_eq!(unsupported_device(&[collection(0xFFFF)]), Some(0xFFFF));
}

#[test]
fn a_supported_keyboard_or_nothing_isnt_reported_as_unsupported() {
    assert_eq!(unsupported_device(&[]), None);
    assert_eq!(unsupported_device(&[collection(0x0265)]), None);
}

// Reports.

#[test]
fn a_demo_report_is_a_file_of_its_own() {
    let dir = TempDir::new("report-demo");
    std::fs::write(dir.0.join(report::FILE_NAME), "a real report").unwrap();
    let path = demo().save_report(&data(), Some(&dir.0)).unwrap();
    assert_eq!(
        path.file_name().unwrap().to_str(),
        Some(report::DEMO_FILE_NAME)
    );
    assert_ne!(report::FILE_NAME, report::DEMO_FILE_NAME);
    assert_eq!(
        std::fs::read_to_string(dir.0.join(report::FILE_NAME)).unwrap(),
        "a real report"
    );
}

#[test]
fn a_demo_report_doesnt_list_the_systems_devices() {
    let dir = TempDir::new("report-list");
    let path = demo().save_report(&data(), Some(&dir.0)).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("not listed in a demo"), "{text}");
}

#[test]
fn a_report_opens_its_folder_unless_scripted() {
    let dir = TempDir::new("report-open");
    let outcome = demo().report_ready(&data(), Some(&dir.0));
    assert_eq!(outcome.note.text, t().report_saved);
    assert_eq!(outcome.open, std::slice::from_ref(&dir.0));

    let scripted = Session::new(Some(DemoArgs {
        shot: Some(PathBuf::from("a.png")),
        ..DemoArgs::default()
    }));
    let outcome = scripted.report_ready(&data(), Some(&dir.0));
    assert_eq!(outcome.note.text, t().report_saved);
    assert!(outcome.open.is_empty());
}

#[test]
fn a_report_that_cant_be_saved_says_so_and_opens_nothing() {
    let outcome = demo().report_ready(&data(), None);
    assert!(outcome.note.text.contains(t().report_failed));
    assert_eq!(outcome.note.tone, Tone::Bad);
    assert!(outcome.open.is_empty());
}
