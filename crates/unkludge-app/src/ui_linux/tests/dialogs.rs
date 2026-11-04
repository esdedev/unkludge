//! The dialogs the window draws for itself, and what's behind them.

use super::*;

#[test]
fn work_that_comes_while_the_app_is_busy_isnt_lost() {
    let fixture = Fixture::connected(None);
    with_app(|_| {
        // The app is taken, by this.
        with_app(|app| app.send(Some(Cmd::Report)));
    });
    assert!(fixture.sent().is_empty(), "not yet");
    fixture.settle();
    assert_eq!(fixture.sent(), ["report"]);
}

#[test]
fn each_colour_slider_changes_its_own_colour() {
    let fixture = Fixture::connected(None);
    let view = View::get(&fixture.window);
    with_app(|app| app.open_color_dialog(false));
    with_app(|app| {
        app.color_channel(0, 10);
        app.color_channel(1, 20);
        app.color_channel(2, 30);
    });
    let values = || {
        let dialog = view.get_color_dialog();
        (dialog.red, dialog.green, dialog.blue)
    };
    assert_eq!(values(), (10, 20, 30));
    assert_eq!(view.get_color_dialog().code, "#0A141E");
    let slider = |name: &str| {
        let mut found = labelled(&fixture.window, name);
        assert_eq!(found.len(), 1, "{name}");
        found.pop().unwrap()
    };
    slider(t().linux.red).invoke_accessible_increment_action();
    assert_eq!(values(), (11, 20, 30));
    slider(t().linux.green).invoke_accessible_increment_action();
    assert_eq!(values(), (11, 21, 30));
    slider(t().linux.blue).invoke_accessible_decrement_action();
    assert_eq!(values(), (11, 21, 29));
    assert_eq!(slider(t().linux.green).accessible_value().unwrap(), "21");
    assert_eq!(
        slider(t().linux.blue).accessible_value_maximum(),
        Some(255.0)
    );
    with_app(App::color_ok);
    let change = {
        let mut last = None;
        with_app_ref(|app| last = app.sent_lighting.borrow().last().copied());
        last.unwrap()
    };
    assert_eq!(
        change.color,
        Some(ColorChoice::Custom(Rgb::new(11, 21, 29)))
    );
}

#[test]
fn tab_doesnt_get_behind_a_dialog() {
    let fixture = Fixture::connected(Some(2));
    with_app(|app| app.open_color_dialog(false));
    fixture.settle();
    let log = log_actions(&fixture.window);
    let stayed = Rc::new(Cell::new(0));
    let actions = Actions::get(&fixture.window);
    let count = Rc::clone(&stayed);
    actions.on_dialog_dismissed(move || count.set(count.get() + 1));
    let count = Rc::clone(&stayed);
    actions.on_color_ok(move || count.set(count.get() + 1));
    for _ in 0..60 {
        fixture.key(Key::Tab);
        fixture.key_text(" ");
        fixture.key(Key::Return);
    }
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
    assert!(stayed.get() > 0, "its own buttons do work");
}

#[test]
fn losing_the_keyboard_with_a_dialog_up_leaves_escape_working() {
    let fixture = Fixture::connected(None);
    let view = View::get(&fixture.window);
    with_app(|app| app.open_color_dialog(true));
    fixture.settle();
    fixture.say(Event::Disconnected);
    assert_eq!(view.get_dialog(), DialogKind::Color);
    fixture.key(Key::Escape);
    fixture.settle();
    assert_eq!(view.get_dialog(), DialogKind::None);
    // What opened it went out of sight with the keyboard, so nothing gets the focus now.
    fixture.say(Event::Connected {
        model: model::find(0x0265).unwrap(),
    });
    fixture.say(Event::Lighting(lighting(LightMode::Effect(2))));
    fixture.key(Key::Return);
    assert_eq!(view.get_dialog(), DialogKind::None);
}

#[test]
fn a_dialog_closed_after_losing_the_keyboard_doesnt_put_the_focus_on_what_went_away() {
    let fixture = Fixture::connected(None);
    let view = View::get(&fixture.window);
    with_app(|app| app.file_picked(crate::ui_linux::dialogs::Picked::NoProgram, true));
    fixture.say(Event::Disconnected);
    assert_eq!(view.get_dialog(), DialogKind::Path);
    with_app(App::dialog_dismissed);
    let log = log_actions(&fixture.window);
    fixture.say(Event::Connected {
        model: model::find(0x0265).unwrap(),
    });
    fixture.say(Event::Lighting(lighting(LightMode::Effect(2))));
    fixture.key(Key::Return);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

#[test]
fn the_colour_picker_sends_what_was_chosen() {
    let fixture = Fixture::connected(None);
    let view = View::get(&fixture.window);
    labelled(&fixture.window, t().custom_color)
        .pop()
        .unwrap()
        .invoke_accessible_default_action();
    fixture.settle();
    assert_eq!(view.get_dialog(), DialogKind::Color);
    assert_eq!(
        view.get_color_dialog().warning,
        t().first_custom_warning,
        "a model's first"
    );
    with_app(|app| app.color_code_edited("#102030"));
    assert_eq!(view.get_color_dialog().red, 0x10);
    assert_eq!(view.get_color_dialog().code, "#102030");
    with_app(|app| app.color_code_edited("#1020"));
    assert_eq!(view.get_color_dialog().red, 0x10, "not a whole code yet");
    assert_eq!(view.get_color_dialog().code, "#1020");
    with_app(|app| app.color_channel(0, 300));
    assert_eq!(view.get_color_dialog().red, 255);
    with_app(App::color_ok);
    assert_eq!(view.get_dialog(), DialogKind::None);
    assert_eq!(fixture.sent().last(), Some(&"set lights"));
}

#[test]
fn nothing_behind_a_dialog_can_be_used_and_escape_closes_it() {
    let fixture = Fixture::connected(None);
    let view = View::get(&fixture.window);
    labelled(&fixture.window, t().custom_color)
        .pop()
        .unwrap()
        .invoke_accessible_default_action();
    fixture.settle();
    let frame = view.get_frame();
    let before = fixture.sent().len();
    // Past the dialog's card, on Sync clock.
    fixture.click(frame.clock.x + frame.clock.w - 40.0, frame.clock.y + 20.0);
    assert_eq!(fixture.sent().len(), before);
    fixture.key(Key::Escape);
    fixture.settle();
    assert_eq!(view.get_dialog(), DialogKind::None);
    assert_eq!(fixture.sent().len(), before, "and nothing was sent");
}

#[test]
fn the_tray_note_goes_away_with_the_window() {
    let fixture = Fixture::connected(None);
    with_app(|app| app.open_dialog(DialogKind::Note, false));
    assert!(labelled(&fixture.window, t().tray_note_title).len() == 1);
    fixture.key(Key::Return);
    fixture.settle();
    assert_eq!(View::get(&fixture.window).get_dialog(), DialogKind::None);
    assert!(!fixture.window.window().is_visible());
}

#[test]
fn a_path_can_be_typed_when_no_program_can_ask_for_one() {
    let fixture = Fixture::connected(None);
    with_app(|app| app.file_picked(crate::ui_linux::dialogs::Picked::NoProgram, false));
    let view = View::get(&fixture.window);
    assert_eq!(view.get_dialog(), DialogKind::Path);
    with_app(|app| app.path_entered("   "));
    assert_eq!(view.get_dialog(), DialogKind::Path, "nothing typed yet");
    with_app(|app| app.path_entered(" /a/b.gif "));
    assert_eq!(view.get_dialog(), DialogKind::None);
    assert_eq!(fixture.sent().last(), Some(&"load"));
    assert_eq!(view.get_screen().file_name, "b.gif");
}

#[test]
fn a_file_being_chosen_does_nothing_when_the_user_changes_their_mind() {
    let fixture = Fixture::connected(None);
    with_app(|app| app.file_picked(crate::ui_linux::dialogs::Picked::Cancelled, false));
    assert!(!View::get(&fixture.window).get_screen().has_file);
    assert_eq!(View::get(&fixture.window).get_dialog(), DialogKind::None);
}
