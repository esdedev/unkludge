//! What a screen reader sees, and whether what's on the window can be read at all.

use super::*;

/// Everything that's visible with one of these roles.
fn controls(window: &AppWindow) -> Vec<ElementHandle> {
    let roles = [
        AccessibleRole::Button,
        AccessibleRole::Switch,
        AccessibleRole::RadioButton,
        AccessibleRole::Slider,
    ];
    roles
        .iter()
        .flat_map(|role| {
            ElementQuery::from_root(window)
                .match_descendants()
                .match_accessible_role(*role)
                .find_all()
        })
        .collect()
}

#[test]
fn every_control_has_a_role_and_a_name_that_says_what_it_is() {
    let fixture = Fixture::connected(Some(2));
    let found = controls(&fixture.window);
    assert!(found.len() > 25, "{}", found.len());
    for control in &found {
        let label = control.accessible_label().unwrap_or_default();
        assert!(!label.is_empty(), "{:?}", control.accessible_role());
    }
    for name in [
        t().send_report.to_string(),
        t().choose_file.to_string(),
        t().upload.to_string(),
        t().sync_clock.to_string(),
        t().auto_clock_name.to_string(),
        t().start_with_system.to_string(),
        t().custom_color.to_string(),
        t().labelled(t().fit, t().fit_cover),
        t().labelled(t().effect, t().off),
        t().labelled(t().sleep, t().sleep_label(SleepTime::ALL[0])),
        t().color_slot(0),
        format!("{}: {}", t().frame_time, t().slower),
        format!("{}: {}", t().frame_time, t().faster),
    ] {
        assert!(!labelled(&fixture.window, &name).is_empty(), "{name}");
    }
}

#[test]
fn a_slider_tells_its_value_and_range_and_can_be_moved_by_a_screen_reader() {
    let fixture = Fixture::connected(None);
    let mut named = labelled(&fixture.window, t().brightness);
    assert_eq!(named.len(), 1, "read once");
    let brightness = named.pop().unwrap();
    assert_eq!(brightness.accessible_role(), Some(AccessibleRole::Slider));
    // A plain number and its range, or it isn't read as a number.
    assert_eq!(brightness.accessible_value().unwrap(), "0");
    assert_eq!(brightness.accessible_value_minimum(), Some(0.0));
    assert_eq!(brightness.accessible_value_maximum(), Some(100.0));
    let model = model::find(0x0265).unwrap();
    let steps = model.lighting.brightness.len() as f32 - 1.0;
    assert_eq!(brightness.accessible_value_step(), Some(100.0 / steps));
    let speed = labelled(&fixture.window, t().speed).pop().unwrap();
    assert_eq!(
        speed.accessible_value().unwrap(),
        model.lighting.speed_min.to_string()
    );
    assert_eq!(
        speed.accessible_value_minimum(),
        Some(f32::from(model.lighting.speed_min))
    );
    assert_eq!(
        speed.accessible_value_maximum(),
        Some(f32::from(model.lighting.speed_max))
    );
    brightness.invoke_accessible_increment_action();
    assert_eq!(fixture.sent().last(), Some(&"adjust lights"));
}

#[test]
fn choices_and_switches_say_whether_theyre_on_and_whether_they_can_be_used() {
    let fixture = Fixture::connected(Some(2));
    let on = t().labelled(t().sleep, t().sleep_label(SleepTime::ALL[1]));
    let off = t().labelled(t().sleep, t().sleep_label(SleepTime::ALL[2]));
    let chip = |name: &str| labelled(&fixture.window, name).pop().unwrap();
    assert_eq!(chip(&on).accessible_checked(), Some(true));
    assert_eq!(chip(&off).accessible_checked(), Some(false));
    assert_eq!(chip(&on).accessible_enabled(), Some(true));
    fixture.click_upload();
    assert_eq!(chip(&on).accessible_enabled(), Some(false));
    let clock = chip(t().auto_clock_name);
    assert_eq!(clock.accessible_role(), Some(AccessibleRole::Switch));
    assert_eq!(clock.accessible_checked(), Some(true));
}

#[test]
fn a_disabled_control_doesnt_answer_a_screen_reader_either() {
    let fixture = Fixture::connected(Some(2));
    let name = t().labelled(t().sleep, t().sleep_label(SleepTime::ALL[1]));
    let chip = labelled(&fixture.window, &name).pop().unwrap();
    fixture.click_upload();
    let before = fixture.sent().len();
    chip.invoke_accessible_default_action();
    assert_eq!(fixture.sent().len(), before);
}

#[test]
fn how_things_stand_is_there_as_text_for_a_screen_reader() {
    let fixture = Fixture::connected(Some(2));
    let status = labelled(&fixture.window, t().connected).pop().unwrap();
    assert_eq!(status.accessible_role(), Some(AccessibleRole::Text));
    fixture.click_upload();
    let progress = labelled(&fixture.window, t().uploading).pop().unwrap();
    assert_eq!(
        progress.accessible_role(),
        Some(AccessibleRole::ProgressIndicator)
    );
    assert_eq!(progress.accessible_value().unwrap(), "0 %");
}

// Lesson: contrast.

fn luminance(color: Color) -> f32 {
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.red()) + 0.7152 * channel(color.green()) + 0.0722 * channel(color.blue())
}

fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn contrast_is_enough_even_for_what_cant_be_used() {
    shot::use_headless_platform();
    let window = AppWindow::new().unwrap();
    let p = Palette::get(&window);
    let text = [
        ("text on card", p.get_text(), p.get_card()),
        ("silver on card", p.get_silver(), p.get_card()),
        ("muted on card", p.get_muted(), p.get_card()),
        ("muted on window", p.get_muted(), p.get_bg()),
        ("chip text", p.get_silver(), p.get_surface()),
        (
            "disabled text on chip",
            p.get_text_disabled(),
            p.get_surface(),
        ),
        ("disabled text on card", p.get_text_disabled(), p.get_card()),
        ("chosen chip", p.get_text(), p.get_accent()),
        (
            "chosen chip, disabled",
            p.get_text(),
            p.get_selected_disabled(),
        ),
        ("good", p.get_green(), p.get_card()),
        ("warning", p.get_amber(), p.get_card()),
        ("bad", p.get_red(), p.get_card()),
        ("link", p.get_accent_hi(), p.get_bg()),
        ("demo", p.get_cyan(), p.get_bg()),
    ];
    for (what, front, back) in text {
        let ratio = contrast(front, back);
        assert!(ratio >= 4.5, "{what}: {ratio}");
    }
    let tracks = [
        ("track", p.get_track(), p.get_card()),
        ("track, disabled", p.get_track_disabled(), p.get_card()),
        ("focus ring", p.get_accent_hi(), p.get_card()),
        ("text field's edge", p.get_field_border(), p.get_well()),
        (
            "text field's edge, outside",
            p.get_field_border(),
            p.get_card(),
        ),
    ];
    for (what, front, back) in tracks {
        let ratio = contrast(front, back);
        assert!(ratio >= 3.0, "{what}: {ratio}");
    }
}

#[test]
fn a_chosen_chip_stands_out_when_nothing_can_be_used() {
    shot::use_headless_platform();
    let window = AppWindow::new().unwrap();
    let p = Palette::get(&window);
    // Its fill against the others', and its edge against the card.
    assert!(contrast(p.get_selected_disabled(), p.get_surface()) >= 1.5);
    assert!(contrast(p.get_muted(), p.get_card()) >= 3.0);
}
