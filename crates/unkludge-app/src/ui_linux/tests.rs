//! The real markup and the real app, on the platform with no desktop, used as a keyboard and a mouse
//! would use them. Each test has a thread to itself, and with it a window and an app.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::{
    ColorChoice, ColorSetting, CurrentLighting, LightMode, Rgb, SleepSetting,
};
use unkludge_protocol::model;
use unkludge_protocol::screen::{Animation, LoadOptions};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{Color, ComponentHandle, Global, LogicalPosition, PhysicalSize, SharedString};

use super::app::{
    App, press, set_app, take_app, wanted_preview_timer, wire_for_tests as wire, with_app,
};
use super::{Actions, AppWindow, Area, DialogKind, Palette, View, shot};
use crate::args::DemoArgs;
use crate::i18n::t;
use crate::session::Session;
use crate::settings::Settings;
use crate::sys;
use crate::worker::{Cmd, Event, LoadedImage};

/// What a command is, for a list of what was sent.
pub fn name_of(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::SetLighting(_) => "set lights",
        Cmd::AdjustLighting(_) => "adjust lights",
        Cmd::SyncClock => "sync clock",
        Cmd::Resumed => "resumed",
        Cmd::LoadImage { .. } => "load",
        Cmd::UploadScreen(_) => "upload",
        Cmd::CancelUpload => "cancel",
        Cmd::Refresh => "refresh",
        Cmd::SetAutoClock(_) => "auto clock",
        Cmd::Report => "report",
    }
}

fn lighting(mode: LightMode) -> CurrentLighting {
    let model = model::find(0x0265).unwrap();
    CurrentLighting {
        mode,
        brightness: model.lighting.brightness.first().copied(),
        speed: Some(model.lighting.speed_min),
        color: Some(ColorSetting::Slot(1)),
        sleep: SleepSetting::Known(SleepTime::ALL[1]),
    }
}

fn image(frames: usize) -> LoadedImage {
    let model = model::find(0x0265).unwrap();
    let options = LoadOptions {
        truncate: true,
        ..LoadOptions::default()
    };
    let bytes = crate::demo::gif_bytes(frames, 200);
    let animation = Animation::from_bytes(&bytes, &model.screen.unwrap(), options).unwrap();
    LoadedImage {
        path: PathBuf::from("a.gif"),
        plan: animation.plan(model).unwrap(),
        animation: Arc::new(animation),
        estimate: Duration::from_secs(40),
    }
}

/// A folder of a test's own for what an app saves, gone at the end.
struct TempDir(PathBuf);

impl Default for TempDir {
    fn default() -> TempDir {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "unkludge-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A window and an app, as far as they can get with no worker. Events go in through the sender, the
/// way the worker's do.
struct Fixture {
    window: AppWindow,
    events: Sender<Event>,
    dir: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        shot::use_headless_platform();
        let window = AppWindow::new().unwrap();
        View::get(&window).set_t(super::view::labels());
        wire(&window);
        let dir = TempDir::default();
        let mut app = App::new(
            window.clone_strong(),
            Settings::default(),
            Session::Demo(DemoArgs::default()),
            Some(dir.path().to_path_buf()),
        );
        let (events, receiver) = channel();
        app.events = Some(receiver);
        app.refresh();
        set_app(app);
        window.window().set_size(PhysicalSize::new(900, 643));
        window.show().unwrap();
        Fixture {
            window,
            events,
            dir,
        }
    }

    /// Connected to a keyboard with these lights on, and with a file ready to go if asked.
    fn connected(frames: Option<usize>) -> Fixture {
        let fixture = Fixture::new();
        fixture.say(Event::Connected {
            model: model::find(0x0265).unwrap(),
        });
        fixture.say(Event::Lighting(lighting(LightMode::Effect(2))));
        if let Some(frames) = frames {
            with_app(|app| {
                let _ = app.state.choose_file(PathBuf::from("a.gif"));
            });
            fixture.say(Event::ImageLoaded(image(frames)));
        }
        fixture
    }

    fn say(&self, event: Event) {
        self.events.send(event).unwrap();
        with_app(App::drain_events);
    }

    fn sent(&self) -> Vec<&'static str> {
        let mut sent = Vec::new();
        with_app_ref(|app| sent = app.sent.borrow().clone());
        sent
    }

    fn key(&self, key: Key) {
        press(self.window.window(), key);
    }

    fn key_text(&self, text: &str) {
        let text = SharedString::from(text);
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }

    /// A key that's being held down, past its first press.
    fn key_repeat(&self, text: impl Into<SharedString>) {
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressRepeated { text: text.into() });
    }

    fn click(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        let window = self.window.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerExited);
        slint::platform::update_timers_and_animations();
    }

    fn click_upload(&self) {
        let action = View::get(&self.window).get_frame().action;
        self.click(action.x + 20.0, action.y + 10.0);
    }

    /// Lets what's waiting on a timer run.
    fn settle(&self) {
        slint::platform::update_timers_and_animations();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        drop(take_app());
    }
}

fn with_app_ref(f: impl FnOnce(&App)) {
    let app = take_app().expect("an app");
    f(&app);
    set_app(app);
}

fn labelled(window: &AppWindow, label: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_accessible_label(window, label).collect()
}

mod dialogs;
mod lifecycle;
mod screen_reader;

// Lesson: holding Space or Enter.

#[test]
fn holding_space_or_enter_does_it_once() {
    let fixture = Fixture::connected(None);
    // Clicking leaves the focus there, with nothing showing, and it's still that button.
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(frame.clock.x + frame.clock.w - 40.0, frame.clock.y + 20.0);
    assert_eq!(fixture.sent(), ["sync clock"]);
    fixture.key_repeat(" ");
    fixture.key_repeat(Key::Return);
    assert_eq!(fixture.sent(), ["sync clock"], "repeats do nothing");
    fixture.key_text(" ");
    assert_eq!(fixture.sent(), ["sync clock", "sync clock"]);
    fixture.key(Key::Return);
    assert_eq!(fixture.sent().len(), 3);
}

#[test]
fn holding_an_arrow_on_a_slider_keeps_going() {
    let fixture = Fixture::connected(None);
    let frame = View::get(&fixture.window).get_frame();
    let track = frame.speed.y + frame.speed.h - 11.0;
    fixture.click(frame.speed.x + 8.0, track);
    let before = fixture.sent().len();
    fixture.key_repeat(Key::RightArrow);
    fixture.key_repeat(Key::RightArrow);
    assert_eq!(fixture.sent().len(), before + 2);
}

// Lesson: no invisible focus on Upload.

#[test]
fn clicking_upload_leaves_no_focus_behind_for_space_to_find() {
    let fixture = Fixture::connected(Some(2));
    fixture.click_upload();
    assert_eq!(fixture.sent().last(), Some(&"upload"));
    fixture.say(Event::UploadDone);
    let before = fixture.sent().len();
    fixture.key_text(" ");
    fixture.key(Key::Return);
    assert_eq!(fixture.sent().len(), before, "{:?}", fixture.sent());
}

#[test]
fn clicking_upload_doesnt_keep_the_focus_even_when_nothing_came_of_it() {
    // Upload stays in sight and usable, which is when a focus left on it could be found.
    let fixture = Fixture::connected(Some(2));
    let log = log_actions(&fixture.window);
    fixture.click_upload();
    assert_eq!(*log.borrow(), ["upload"]);
    fixture.key_text(" ");
    fixture.key(Key::Return);
    assert_eq!(*log.borrow(), ["upload"]);
}

#[test]
fn clicking_upload_takes_the_focus_from_what_had_it() {
    let fixture = Fixture::connected(Some(2));
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(frame.clock.x + frame.clock.w - 40.0, frame.clock.y + 20.0);
    fixture.click_upload();
    let before = fixture.sent().len();
    fixture.key_text(" ");
    assert_eq!(fixture.sent().len(), before, "{:?}", fixture.sent());
}

#[test]
fn enter_on_upload_moves_the_focus_to_cancel_and_back_at_the_end() {
    let fixture = Fixture::connected(Some(2));
    fixture.window.invoke_focus_upload();
    fixture.key(Key::Return);
    assert_eq!(fixture.sent().last(), Some(&"upload"));
    // It's Cancel that Space finds now.
    fixture.key_text(" ");
    assert_eq!(fixture.sent().last(), Some(&"cancel"));
    fixture.say(Event::Cancelling);
    fixture.say(Event::UploadCancelled);
    // And back on Upload at the end.
    let before = fixture.sent().len();
    fixture.key(Key::Return);
    assert_eq!(fixture.sent().len(), before + 1);
    assert_eq!(fixture.sent().last(), Some(&"upload"));
}

#[test]
fn holding_enter_on_upload_doesnt_go_on_to_cancel_it() {
    let fixture = Fixture::connected(Some(2));
    fixture.window.invoke_focus_upload();
    fixture.key(Key::Return);
    fixture.key_repeat(Key::Return);
    fixture.key_repeat(Key::Return);
    assert_eq!(fixture.sent().last(), Some(&"upload"));
}

#[test]
fn losing_the_keyboard_leaves_no_focus_for_when_it_comes_back() {
    let fixture = Fixture::connected(None);
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(frame.clock.x + frame.clock.w - 40.0, frame.clock.y + 20.0);
    assert_eq!(fixture.sent(), ["sync clock"]);
    fixture.say(Event::Disconnected);
    fixture.say(Event::Connected {
        model: model::find(0x0265).unwrap(),
    });
    fixture.say(Event::Lighting(lighting(LightMode::Effect(2))));
    fixture.key_text(" ");
    assert_eq!(fixture.sent(), ["sync clock"]);
}

#[test]
fn a_click_on_nothing_leaves_nothing_with_the_focus() {
    let fixture = Fixture::connected(None);
    let effects = View::get(&fixture.window).get_effects();
    let chip = slint::Model::row_data(&effects, 3).unwrap().area;
    fixture.click(chip.x + 5.0, chip.y + 5.0);
    assert_eq!(fixture.sent(), ["set lights"]);
    // Between the header and the cards.
    fixture.click(450.0, 72.0);
    fixture.key_text(" ");
    assert_eq!(fixture.sent(), ["set lights"]);
}

#[test]
fn a_click_on_a_card_with_nothing_there_leaves_nothing_with_the_focus_either() {
    let fixture = Fixture::connected(None);
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(frame.clock.x + frame.clock.w - 40.0, frame.clock.y + 20.0);
    fixture.click(frame.screen.x + 5.0, frame.screen.y + 40.0);
    fixture.key_text(" ");
    assert_eq!(fixture.sent(), ["sync clock"]);
}

#[test]
fn a_slider_can_be_taken_all_along_its_row() {
    let fixture = Fixture::connected(None);
    let frame = View::get(&fixture.window).get_frame();
    let area = frame.brightness;
    let track = area.y + area.h - 11.0;
    // From the right edge, and above and below the track.
    fixture.click(area.x + area.w - 1.0, track);
    assert_eq!(fixture.sent().last(), Some(&"adjust lights"));
    let before = fixture.sent().len();
    fixture.click(area.x + 1.0, track - 11.0);
    assert_eq!(fixture.sent().len(), before + 1);
    let before = fixture.sent().len();
    fixture.click(area.x + area.w - 1.0, track + 10.0);
    assert_eq!(fixture.sent().len(), before + 1);
}

#[test]
fn clicking_cancel_leaves_no_focus_behind_either() {
    let fixture = Fixture::connected(Some(2));
    fixture.click_upload();
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(
        frame.action.x + frame.action.w - 20.0,
        frame.action.y + 10.0,
    );
    assert_eq!(fixture.sent().last(), Some(&"cancel"));
    fixture.say(Event::Cancelling);
    fixture.say(Event::UploadCancelled);
    let before = fixture.sent().len();
    fixture.key_text(" ");
    assert_eq!(fixture.sent().len(), before);
}

// Lesson: during an upload.

#[test]
fn during_an_upload_the_lights_cant_be_used() {
    let fixture = Fixture::connected(Some(2));
    let effect = View::get(&fixture.window).get_effects();
    let chip = slint::Model::row_data(&effect, 0).unwrap().area;
    fixture.click(chip.x + 5.0, chip.y + 5.0);
    assert_eq!(
        fixture.sent().last(),
        Some(&"set lights"),
        "it works before"
    );
    fixture.click_upload();
    let before = fixture.sent().len();
    fixture.click(chip.x + 5.0, chip.y + 5.0);
    let frame = View::get(&fixture.window).get_frame();
    fixture.click(
        frame.brightness.x + 40.0,
        frame.brightness.y + frame.brightness.h - 11.0,
    );
    assert_eq!(fixture.sent().len(), before, "{:?}", fixture.sent());
    let lights = View::get(&fixture.window).get_lights();
    assert!(!lights.enabled);
}

#[test]
fn cancelling_shows_and_cant_be_asked_for_twice_and_the_end_brings_things_back() {
    let fixture = Fixture::connected(Some(2));
    fixture.window.invoke_focus_upload();
    fixture.key(Key::Return);
    fixture.key_text(" ");
    fixture.say(Event::Cancelling);
    let screen = View::get(&fixture.window).get_screen();
    assert!(screen.cancelling);
    assert_eq!(screen.notes, t().cancelling);
    let before = fixture.sent().len();
    fixture.key_text(" ");
    assert_eq!(fixture.sent().len(), before, "Cancel is off now");
    fixture.say(Event::UploadCancelled);
    let screen = View::get(&fixture.window).get_screen();
    assert!(screen.idle && !screen.cancelling);
    assert_eq!(screen.notes, t().upload_cancelled);
    assert!(View::get(&fixture.window).get_lights().enabled);
}

// Lesson: what the keyboard being in trouble looks like.

#[test]
fn a_hung_keyboard_gets_a_banner_until_it_responds() {
    let fixture = Fixture::connected(None);
    let plain = View::get(&fixture.window).get_frame().height;
    fixture.say(Event::Hung);
    let view = View::get(&fixture.window);
    assert_eq!(view.get_header().status, t().not_responding);
    assert_eq!(slint::Model::row_count(&view.get_banners()), 1);
    assert!(view.get_frame().height > plain, "room for the banner");
    assert!(!labelled(&fixture.window, t().hung).is_empty());
    fixture.say(Event::Responding);
    assert_eq!(slint::Model::row_count(&view.get_banners()), 0);
    assert_eq!(view.get_frame().height, plain);
    assert_eq!(view.get_header().status, t().connected);
}

#[test]
fn settings_that_cant_be_read_get_a_banner() {
    shot::use_headless_platform();
    let window = AppWindow::new().unwrap();
    let settings = Settings {
        read_error: Some("no".to_string()),
        ..Settings::default()
    };
    let mut app = App::new(
        window.clone_strong(),
        settings,
        Session::Demo(DemoArgs::default()),
        None,
    );
    app.refresh();
    let banners = View::get(&window).get_banners();
    assert_eq!(
        slint::Model::row_data(&banners, 0).unwrap().text,
        t().settings_unreadable
    );
}

#[test]
fn a_keyboard_the_system_keeps_away_gets_the_button_and_a_demo_can_press_it() {
    let fixture = Fixture::new();
    fixture.say(Event::CannotOpen(unkludge_protocol::Error::Hid(
        "denied".to_string(),
    )));
    let view = View::get(&fixture.window);
    assert!(view.get_empty().can_grant);
    assert!(!labelled(&fixture.window, t().linux.grant_access).is_empty());
    with_app(|app| app.access_granted(Err(sys::GrantError::Cancelled)));
    assert_eq!(view.get_footer().message, t().linux.access_cancelled);
    with_app(|app| app.access_granted(Ok(())));
    assert_eq!(view.get_footer().message, t().linux.access_granted);
    assert_eq!(fixture.sent().last(), Some(&"refresh"));
}

#[test]
fn an_unsupported_keyboard_says_so_and_offers_a_report() {
    let fixture = Fixture::new();
    fixture.say(Event::CannotOpen(unkludge_protocol::Error::Unsupported(
        0x002A,
    )));
    let view = View::get(&fixture.window);
    assert_eq!(view.get_empty().headline, t().unsupported_title);
    assert!(!view.get_empty().can_grant);
    assert!(view.get_header().report);
    labelled(&fixture.window, t().send_report)[0].invoke_accessible_default_action();
    assert_eq!(fixture.sent(), ["report"]);
}

// Lesson: the preview.

#[test]
fn the_preview_doesnt_decode_while_nobody_can_see_it() {
    let fixture = Fixture::connected(Some(4));
    let count = || {
        let mut decoded = 0;
        with_app_ref(|app| decoded = app.frames_decoded);
        decoded
    };
    let shown = count();
    assert!(shown >= 1, "the first frame shows");
    fixture.window.hide().unwrap();
    std::thread::sleep(Duration::from_millis(250));
    with_app(App::update_preview);
    assert_eq!(count(), shown, "hidden");
    fixture.window.show().unwrap();
    with_app(App::update_preview);
    assert_eq!(count(), shown + 1);
    fixture.window.window().set_minimized(true);
    std::thread::sleep(Duration::from_millis(250));
    with_app(App::update_preview);
    assert_eq!(count(), shown + 1, "minimized");
}

#[test]
fn a_still_image_shows_once() {
    let fixture = Fixture::connected(Some(1));
    let mut decoded = 0;
    with_app_ref(|app| decoded = app.frames_decoded);
    assert_eq!(decoded, 1);
    with_app(App::update_preview);
    with_app_ref(|app| decoded = app.frames_decoded);
    assert_eq!(decoded, 1);
    assert!(View::get(&fixture.window).get_screen().has_picture);
}

#[test]
fn the_preview_only_looks_for_frames_when_theres_something_to_do() {
    let every = |delay| Some(Duration::from_millis(delay));
    assert_eq!(wanted_preview_timer(true, false, None), None);
    assert_eq!(wanted_preview_timer(true, false, Some((1, 100))), None);
    assert_eq!(wanted_preview_timer(false, false, Some((5, 100))), None);
    assert_eq!(
        wanted_preview_timer(true, false, Some((5, 100))),
        every(100)
    );
    assert_eq!(wanted_preview_timer(true, false, Some((5, 1))), every(33));
    assert_eq!(
        wanted_preview_timer(true, true, Some((5, 100))),
        every(1000)
    );
}

// Lesson: going away.

#[test]
fn with_no_tray_closing_minimizes_and_then_hides_if_that_didnt_work() {
    let fixture = Fixture::connected(None);
    let window = fixture.window.window();
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(window.is_minimized());
    assert!(window.is_visible());
    assert_eq!(
        View::get(&fixture.window).get_footer().message,
        t().linux.no_tray
    );
    window.dispatch_event(WindowEvent::CloseRequested);
    assert!(!window.is_visible());
}

// Going through it all with Tab.

#[test]
fn tab_goes_through_the_controls_in_order_skipping_what_isnt_there() {
    let fixture = Fixture::connected(Some(2));
    let report_before = fixture.sent().len();
    assert_eq!(report_before, 0);
    // The first thing is the header's link, as it's a model not tried yet.
    fixture.key(Key::Tab);
    fixture.key(Key::Return);
    assert_eq!(fixture.sent(), ["report"]);
    // Backwards from there goes around to the last thing, which is the switch at the foot, whose
    // answer shows in the window.
    fixture.key(Key::Backtab);
    let view = View::get(&fixture.window);
    let on = view.get_footer().autostart;
    fixture.key_text(" ");
    assert_eq!(view.get_footer().autostart, !on);
}

#[test]
fn arrows_move_among_chips_as_tab_does() {
    let fixture = Fixture::connected(None);
    let view = View::get(&fixture.window);
    let effects = view.get_effects();
    let chip = slint::Model::row_data(&effects, 3).unwrap().area;
    fixture.click(chip.x + 5.0, chip.y + 5.0);
    let before = fixture.sent().len();
    fixture.key(Key::RightArrow);
    fixture.settle();
    fixture.key_text(" ");
    assert_eq!(fixture.sent().len(), before + 1);
    assert_eq!(fixture.sent().last(), Some(&"set lights"));
}

#[test]
fn ctrl_q_quits_from_wherever_the_focus_is() {
    let fixture = Fixture::connected(None);
    let asked = Rc::new(Cell::new(0));
    let counter = Rc::clone(&asked);
    Actions::get(&fixture.window).on_quit(move || counter.set(counter.get() + 1));
    let control = SharedString::from(Key::Control);
    let window = fixture.window.window();
    // With nothing focused, and then with something that is.
    for tabs in [0, 3] {
        for _ in 0..tabs {
            fixture.key(Key::Tab);
        }
        window.dispatch_event(WindowEvent::KeyPressed {
            text: control.clone(),
        });
        fixture.key_text("q");
        window.dispatch_event(WindowEvent::KeyReleased {
            text: control.clone(),
        });
    }
    assert_eq!(asked.get(), 2);
    fixture.key_text("q");
    assert_eq!(asked.get(), 2, "a q by itself is nothing");
}

/// Everything the markup can ask for, written down instead of done.
fn log_actions(window: &AppWindow) -> Rc<RefCell<Vec<String>>> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let actions = Actions::get(window);
    macro_rules! logged {
        ($setter:ident, $name:literal $(, $arg:ident)*) => {{
            let log = Rc::clone(&log);
            actions.$setter(move |$($arg),*| {
                $(let _ = &$arg;)*
                log.borrow_mut().push($name.to_string());
            });
        }};
    }
    logged!(on_report, "report");
    logged!(on_choose_file, "choose file", by_keyboard);
    logged!(on_slower, "slower");
    logged!(on_faster, "faster");
    logged!(on_upload, "upload", by_keyboard);
    logged!(on_cancel, "cancel");
    logged!(on_sync_clock, "sync clock");
    logged!(on_toggle_auto_clock, "auto clock");
    logged!(on_pick_rainbow, "rainbow");
    logged!(on_custom_color, "custom colour", by_keyboard);
    logged!(on_toggle_autostart, "autostart");
    logged!(on_grant_access, "grant");
    {
        let log = Rc::clone(&log);
        actions.on_pick_fit(move |at| log.borrow_mut().push(format!("fit {at}")));
    }
    {
        let log = Rc::clone(&log);
        actions.on_pick_effect(move |at| log.borrow_mut().push(format!("effect {at}")));
    }
    {
        let log = Rc::clone(&log);
        actions.on_pick_slot(move |at| log.borrow_mut().push(format!("slot {at}")));
    }
    {
        let log = Rc::clone(&log);
        actions.on_pick_sleep(move |at| log.borrow_mut().push(format!("sleep {at}")));
    }
    log
}

/// What Enter does at each stop along the way, once around.
fn tab_stops(fixture: &Fixture) -> Vec<String> {
    let log = log_actions(&fixture.window);
    for _ in 0..80 {
        fixture.key(Key::Tab);
        fixture.key(Key::Return);
    }
    let stops = log.borrow().clone();
    let again = stops.iter().skip(1).position(|stop| *stop == stops[0]);
    stops[..again.map_or(stops.len(), |at| at + 1)].to_vec()
}

#[test]
fn tab_goes_through_a_connected_window_in_reading_order() {
    let fixture = Fixture::connected(Some(2));
    let stops = tab_stops(&fixture);
    let model = model::find(0x0265).unwrap();
    let mut expected: Vec<String> = [
        "report",
        "choose file",
        "choose file",
        "fit 0",
        "fit 1",
        "fit 2",
    ]
    .map(String::from)
    .to_vec();
    expected.extend(["faster", "slower", "upload"].map(String::from));
    expected.extend((0..=model.lighting.effects.len()).map(|at| format!("effect {at}")));
    // Sliders don't do anything with Enter, and they're next.
    expected.extend((0..7).map(|at| format!("slot {at}")));
    expected.push("rainbow".to_string());
    expected.push("custom colour".to_string());
    expected.extend((0..SleepTime::ALL.len()).map(|at| format!("sleep {at}")));
    expected.extend(["sync clock", "auto clock", "autostart"].map(String::from));
    assert_eq!(stops, expected);
}

#[test]
fn tab_skips_what_cant_be_used_during_an_upload() {
    let fixture = Fixture::connected(Some(2));
    fixture.click_upload();
    let stops = tab_stops(&fixture);
    assert_eq!(
        stops,
        [
            "report",
            "choose file",
            "choose file",
            "fit 0",
            "fit 1",
            "fit 2",
            "faster",
            "slower",
            "cancel",
            "sync clock",
            "auto clock",
            "autostart",
        ]
    );
}

#[test]
fn with_no_keyboard_there_isnt_much_to_tab_through() {
    let fixture = Fixture::new();
    fixture.say(Event::Disconnected);
    let log = log_actions(&fixture.window);
    for _ in 0..4 {
        fixture.key(Key::Tab);
        fixture.key(Key::Return);
    }
    assert!(
        log.borrow().iter().all(|stop| stop == "autostart"),
        "{log:?}"
    );
}
