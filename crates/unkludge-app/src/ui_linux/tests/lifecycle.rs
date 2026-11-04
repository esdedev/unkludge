//! Starting, going away and coming back, as far as that can go with no tray host and no display. What
//! starting reaches outside the window is [`FakeOutside`], so that no real worker can start here.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use ksni::blocking::Handle;

use super::*;
use crate::ui_linux::app::Outside;
use crate::ui_linux::tray::Tray;

#[derive(Default)]
struct FakeOutside {
    /// Whether the tray it spawns finds a host.
    tray: bool,
    tray_ids: RefCell<Vec<String>>,
    listened: Cell<bool>,
    hooked: Cell<bool>,
    /// Whether the window was up when the worker started, each time one did.
    workers: RefCell<Vec<bool>>,
    dir: TempDir,
}

impl FakeOutside {
    fn saved_settings(&self) -> bool {
        self.dir.path().join("settings-demo.txt").exists()
    }
}

impl Outside for FakeOutside {
    fn spawn_tray(&self, tray: Tray) -> Option<Handle<Tray>> {
        self.tray_ids.borrow_mut().push(tray.id.to_string());
        tray.online.store(self.tray, Ordering::Relaxed);
        None
    }

    fn listen_for_other_copies(&self) {
        self.listened.set(true);
    }

    fn hook_window(&self, _window: &AppWindow) {
        self.hooked.set(true);
    }

    fn start_worker(&self, app: &mut App) {
        self.workers
            .borrow_mut()
            .push(app.window.window().is_visible());
    }

    fn data_dir(&self) -> Option<PathBuf> {
        Some(self.dir.path().to_path_buf())
    }
}

fn demo() -> Session {
    Session::Demo(DemoArgs::default())
}

fn started(session: Session, start_hidden: bool, outside: &FakeOutside) -> AppWindow {
    shot::use_headless_platform();
    let window = AppWindow::new().unwrap();
    App::start(
        window.clone_strong(),
        Settings::default(),
        session,
        start_hidden,
        outside,
    );
    window
}

#[test]
fn a_demo_leaves_the_real_copys_socket_alone() {
    let outside = FakeOutside::default();
    let _window = started(demo(), false, &outside);
    assert!(!outside.listened.get());
    assert_eq!(*outside.tray_ids.borrow(), ["unkludge-demo"]);
    drop(take_app());
}

#[test]
fn the_real_app_listens_for_other_copies_and_has_a_tray_of_its_own_name() {
    let outside = FakeOutside::default();
    let _window = started(Session::Real, false, &outside);
    assert!(outside.listened.get());
    assert!(outside.hooked.get());
    assert_eq!(*outside.tray_ids.borrow(), ["unkludge"]);
    drop(take_app());
}

#[test]
fn a_scripted_demo_keeps_to_itself() {
    let outside = FakeOutside::default();
    let args = DemoArgs {
        shot: Some(outside.dir.path().join("a.png")),
        ..DemoArgs::default()
    };
    let window = started(Session::Demo(args), false, &outside);
    assert!(outside.tray_ids.borrow().is_empty());
    assert!(!outside.listened.get());
    assert!(!outside.hooked.get());
    assert_eq!(*outside.workers.borrow(), [true]);
    assert!(window.window().is_visible());
    drop(take_app());
}

#[test]
fn starting_normally_shows_the_window_before_the_worker_starts() {
    let outside = FakeOutside {
        tray: true,
        ..FakeOutside::default()
    };
    let window = started(demo(), false, &outside);
    assert_eq!(*outside.workers.borrow(), [true]);
    assert!(window.window().is_visible());
    assert_eq!(View::get(&window).get_footer().message, t().demo_name);
    with_app_ref(|app| assert!(app.watching_for_sleep()));
    drop(take_app());
}

#[test]
fn with_a_tray_a_start_in_the_tray_shows_nothing_and_the_worker_still_starts() {
    let outside = FakeOutside {
        tray: true,
        ..FakeOutside::default()
    };
    let window = started(demo(), true, &outside);
    assert!(!window.window().is_visible());
    assert_eq!(*outside.workers.borrow(), [false]);
    with_app_ref(|app| assert!(app.parked_at.is_none()));
    drop(take_app());
}

#[test]
fn with_no_tray_a_start_in_the_tray_waits_for_the_window_to_minimize_it() {
    let outside = FakeOutside::default();
    let window = started(demo(), true, &outside);
    assert!(window.window().is_visible());
    assert_eq!(View::get(&window).get_footer().message, t().linux.no_tray);
    with_app_ref(|app| assert!(app.parked_at.is_some()));
    // There's no window from the system here, and Slint would forget being asked and go on thinking
    // that it's minimized, which would leave any picture blank.
    assert!(!window.window().is_minimized());
    drop(take_app());
}

#[test]
fn the_computer_waking_up_sets_the_clock() {
    let fixture = Fixture::connected(None);
    let asleep = |seconds| Some(Duration::from_secs(seconds));
    with_app(move |app| app.look_for_wake_up(asleep(10)));
    with_app(move |app| app.look_for_wake_up(asleep(10)));
    assert!(!fixture.sent().contains(&"resumed"));
    with_app(move |app| app.look_for_wake_up(asleep(70)));
    assert_eq!(fixture.sent().last(), Some(&"resumed"));
    let sent = fixture.sent().len();
    with_app(move |app| app.look_for_wake_up(asleep(70)));
    assert_eq!(fixture.sent().len(), sent, "once");
}

// Going away and coming back.

fn long_ago() -> Option<Instant> {
    Instant::now().checked_sub(Duration::from_secs(5))
}

#[test]
fn the_first_close_with_a_tray_says_it_keeps_running_and_only_once() {
    let fixture = Fixture::connected(None);
    with_app(|app| app.tray_online.store(true, Ordering::Relaxed));
    let window = fixture.window.window();
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(window.is_visible());
    assert_eq!(View::get(&fixture.window).get_dialog(), DialogKind::Note);
    let saved = std::fs::read_to_string(fixture.dir.path().join("settings-demo.txt")).unwrap();
    assert!(saved.contains("tray_note_shown=1"), "{saved}");
    // Closing with the note up, and from then on, hides it.
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(!window.is_visible());
    assert_eq!(View::get(&fixture.window).get_dialog(), DialogKind::None);
    window.show().unwrap();
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(!window.is_visible());
}

#[test]
fn hiding_lets_go_of_the_picture_and_coming_back_shows_it_again() {
    let fixture = Fixture::connected(Some(4));
    let decoded = || {
        let mut decoded = 0;
        with_app_ref(|app| decoded = app.frames_decoded);
        decoded
    };
    let before = decoded();
    with_app(App::hide_window);
    with_app(App::show_window);
    assert_eq!(decoded(), before + 1);
    assert!(View::get(&fixture.window).get_screen().has_picture);
}

#[test]
fn a_window_that_comes_back_can_be_minimized_again() {
    let fixture = Fixture::connected(Some(4));
    let window = fixture.window.window();
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(window.is_minimized());
    with_app(|app| app.minimized_at = long_ago());
    with_app(App::window_seen);
    assert!(!window.is_minimized(), "seen, so it's back");
    with_app_ref(|app| assert!(app.minimized_at.is_none()));
    // It doesn't count as closing twice in a row, and it doesn't hide.
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(window.is_visible());
    assert!(window.is_minimized());
}

#[test]
fn input_already_on_its_way_when_minimizing_doesnt_count_as_being_back() {
    let fixture = Fixture::connected(None);
    with_app(App::minimize);
    with_app(App::window_seen);
    assert!(fixture.window.window().is_minimized());
}

#[test]
fn a_window_that_comes_back_shows_the_picture_it_couldnt_before() {
    let fixture = Fixture::connected(None);
    with_app(App::minimize);
    with_app(|app| {
        let _ = app.state.choose_file(PathBuf::from("a.gif"));
    });
    fixture.say(Event::ImageLoaded(image(4)));
    let decoded = || {
        let mut decoded = 0;
        with_app_ref(|app| decoded = app.frames_decoded);
        decoded
    };
    assert_eq!(decoded(), 0);
    with_app(|app| app.minimized_at = long_ago());
    with_app(App::window_seen);
    assert_eq!(decoded(), 1);
}

#[test]
fn a_window_the_user_brought_back_doesnt_go_into_a_tray_that_turns_up_later() {
    let fixture = Fixture::connected(None);
    with_app(|app| {
        app.parked_at = Some(Instant::now());
        app.minimize();
        app.minimized_at = long_ago();
    });
    with_app(App::window_seen);
    with_app(App::tray_came_online);
    assert!(fixture.window.window().is_visible());
}

#[test]
fn a_tray_turning_up_takes_in_a_window_parked_for_want_of_one() {
    let fixture = Fixture::connected(None);
    with_app(|app| {
        app.parked_at = Some(Instant::now());
        app.state.message = Some(crate::state::Note::new(
            t().linux.no_tray,
            crate::state::Tone::Warning,
        ));
    });
    with_app(App::tray_came_online);
    assert!(!fixture.window.window().is_visible());
    assert_ne!(
        View::get(&fixture.window).get_footer().message,
        t().linux.no_tray
    );
}

#[test]
fn a_tray_turning_up_leaves_a_window_thats_just_open_alone() {
    let fixture = Fixture::connected(None);
    with_app(App::tray_came_online);
    assert!(fixture.window.window().is_visible());
}

#[test]
fn a_tray_going_away_brings_back_a_window_hidden_in_it_and_fits_it() {
    let fixture = Fixture::connected(None);
    with_app(App::hide_window);
    with_app(|app| app.fit_tries = 0);
    with_app(App::tray_went_offline);
    assert!(fixture.window.window().is_visible());
    assert_eq!(
        View::get(&fixture.window).get_footer().message,
        t().linux.no_tray
    );
    with_app_ref(|app| assert_eq!(app.fit_tries, 1, "it asked for the window's size"));
}

#[test]
fn a_tray_going_away_leaves_an_open_window_alone() {
    let fixture = Fixture::connected(None);
    with_app(App::tray_went_offline);
    let window = fixture.window.window();
    assert!(window.is_visible());
    assert!(!window.is_minimized());
    assert_ne!(
        View::get(&fixture.window).get_footer().message,
        t().linux.no_tray
    );
}

#[test]
fn a_window_that_keeps_being_resized_doesnt_get_told_forever() {
    let _fixture = Fixture::connected(None);
    for _ in 0..50 {
        with_app(App::window_resized);
    }
    with_app_ref(|app| assert_eq!(app.size_refits, 5));
    // The layout changing starts it over.
    _fixture.say(Event::Hung);
    with_app_ref(|app| assert_eq!(app.size_refits, 0));
}

// What can get to a dialog, or through it.

#[test]
fn a_file_being_asked_for_leaves_nothing_usable_behind_it() {
    let fixture = Fixture::connected(Some(2));
    with_app(|app| {
        assert!(app.begin_choosing());
        assert!(!app.begin_choosing(), "one at a time");
    });
    let before = fixture.sent().len();
    fixture.click_upload();
    fixture.window.invoke_focus_upload();
    fixture.key(Key::Return);
    assert_eq!(fixture.sent().len(), before, "{:?}", fixture.sent());
    with_app(|app| app.file_picked(crate::ui_linux::dialogs::Picked::Cancelled, false));
    fixture.click_upload();
    assert_eq!(fixture.sent().last(), Some(&"upload"));
}

#[test]
fn a_slider_that_has_the_focus_doesnt_move_while_a_file_is_being_asked_for() {
    let fixture = Fixture::connected(None);
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(frame.speed.x + 8.0, frame.speed.y + frame.speed.h - 11.0);
    with_app(|app| {
        app.begin_choosing();
    });
    let before = fixture.sent().len();
    fixture.key(Key::RightArrow);
    assert_eq!(fixture.sent().len(), before, "{:?}", fixture.sent());
    with_app(|app| app.file_picked(crate::ui_linux::dialogs::Picked::Cancelled, false));
    fixture.key(Key::RightArrow);
    assert_eq!(fixture.sent().last(), Some(&"adjust lights"));
}

#[test]
fn a_file_chosen_gets_loaded() {
    let fixture = Fixture::connected(None);
    with_app(|app| {
        app.begin_choosing();
        app.file_picked(
            crate::ui_linux::dialogs::Picked::File(PathBuf::from("/a/b.gif")),
            false,
        );
    });
    assert_eq!(fixture.sent().last(), Some(&"load"));
    assert_eq!(View::get(&fixture.window).get_screen().file_name, "b.gif");
    assert!(!View::get(&fixture.window).get_modal());
}

#[test]
fn of_several_files_dropped_together_the_first_counts() {
    let fixture = Fixture::connected(None);
    let now = Instant::now();
    with_app(move |app| {
        app.file_dropped(PathBuf::from("/a/first.gif"), now);
        app.file_dropped(
            PathBuf::from("/a/second.gif"),
            now + Duration::from_millis(5),
        );
        app.file_dropped(
            PathBuf::from("/a/third.gif"),
            now + Duration::from_millis(9),
        );
    });
    assert_eq!(
        fixture.sent().iter().filter(|cmd| **cmd == "load").count(),
        1,
        "only one gets decoded"
    );
    assert_eq!(
        View::get(&fixture.window).get_screen().file_name,
        "first.gif"
    );
    // A drop later on is another drop.
    with_app(move |app| {
        app.file_dropped(PathBuf::from("/a/later.gif"), now + Duration::from_secs(5))
    });
    assert_eq!(
        View::get(&fixture.window).get_screen().file_name,
        "later.gif"
    );
}

#[test]
fn nothing_gets_dropped_through_a_dialog() {
    let fixture = Fixture::connected(None);
    with_app(|app| app.open_color_dialog(false));
    let now = Instant::now();
    with_app(move |app| app.file_dropped(PathBuf::from("/a/b.gif"), now));
    assert!(!View::get(&fixture.window).get_screen().has_file);
    assert!(!fixture.sent().contains(&"load"));
}

#[test]
fn with_a_program_asking_for_a_file_nothing_gets_dropped_either() {
    let fixture = Fixture::connected(None);
    with_app(|app| {
        app.begin_choosing();
    });
    let now = Instant::now();
    with_app(move |app| app.file_dropped(PathBuf::from("/a/b.gif"), now));
    assert!(!View::get(&fixture.window).get_screen().has_file);
}

// A temporary folder that this module's fake uses too.

#[test]
fn nothing_is_left_of_the_fixtures_folder() {
    let path = {
        let dir = TempDir::default();
        let path: &Path = dir.path();
        path.to_path_buf()
    };
    assert!(!path.exists());
}

#[test]
fn being_told_to_end_quits_the_usual_way() {
    let outside = FakeOutside {
        tray: true,
        ..FakeOutside::default()
    };
    let _window = started(demo(), false, &outside);
    with_app(|app| app.look_for_end(false));
    assert!(!outside.saved_settings(), "nothing asked for it yet");
    with_app(|app| app.look_for_end(true));
    assert!(outside.saved_settings());
    take_app();
}
