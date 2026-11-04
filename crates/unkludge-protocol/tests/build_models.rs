//! Tests for the build-script logic that turns `data/models/*.toml` into the compiled table.

#[path = "../build/models.rs"]
mod models;

use models::{parse_model, render};

const GOOD: &str = r#"
pid = 0x0265
name = "RK S85 ES"
protocol = "v2"
fw = 1
status = "experimental"
layout = "01FA"

[screen]
width = 320
height = 172
max_frames = 60
kind = 1
crc = false
draw_frame_time = 180

[lighting]
brightness = [0, 4, 8]
speed = { min = 0, max = 5, default = 1 }
sleep = true
effects = [
  { id = 1, name = "Solid", speed = false, brightness = true, rainbow = true, color = true },
]
"#;

const LAYOUT_TARGET: &str = r#"
pid = 0x01FA
name = "Target"
protocol = "v1"
fw = 0
status = "tested"

[lighting]
sleep = false
effects = []
"#;

fn parse_err(file: &str, text: &str) -> String {
    match parse_model(file, text) {
        Ok(_) => panic!("{file} should have been rejected"),
        Err(e) => e,
    }
}

#[test]
fn unknown_fields_are_ignored() {
    let text = format!(
        "{GOOD}
[wireless]
vid = 0x258A
pid = 0x0150

[[keys]]
id = 1
label = \"Esc\"
rect = [25, 34, 52, 64]
"
    );
    assert!(parse_model("0265.toml", &text).is_ok());
}

/// A model of the old protocol, which this version only knows the name of.
fn unsupported(text: &str) -> String {
    text.replace("fw = 1", "fw = 0").replace("\"v2\"", "\"v1\"")
}

#[test]
fn a_supported_model_that_hides_effects_is_rejected() {
    let text = GOOD.replace("sleep = true", "sleep = true\nled_mask = 3");
    let e = parse_err("0265.toml", &text);
    assert!(e.contains("0265.toml") && e.contains("led_mask"), "{e}");
}

#[test]
fn a_supported_model_with_effect_numbers_of_its_own_is_rejected() {
    let text = GOOD.replace("{ id = 1,", "{ id = 1, hw = 7,");
    let e = parse_err("0265.toml", &text);
    assert!(e.contains("0265.toml") && e.contains("hw = 7"), "{e}");
}

#[test]
fn zero_in_those_fields_changes_nothing() {
    let text = GOOD
        .replace("sleep = true", "sleep = true\nled_mask = 0")
        .replace("{ id = 1,", "{ id = 1, hw = 0,");
    assert!(parse_model("0265.toml", &text).is_ok());
}

#[test]
fn a_model_this_version_doesnt_support_can_have_them() {
    let masked = unsupported(&GOOD.replace("sleep = true", "sleep = true\nled_mask = 3"));
    assert!(parse_model("0265.toml", &masked).is_ok());
    let renumbered = unsupported(&GOOD.replace("{ id = 1,", "{ id = 1, hw = 7,"));
    assert!(parse_model("0265.toml", &renumbered).is_ok());
    // Nor does one with no screen, on the right protocol.
    let screen_at = GOOD.find("[screen]").unwrap();
    let lighting_at = GOOD.find("[lighting]").unwrap();
    let no_screen = format!("{}{}", &GOOD[..screen_at], &GOOD[lighting_at..])
        .replace("sleep = true", "sleep = true\nled_mask = 3");
    assert!(parse_model("0265.toml", &no_screen).is_ok());
}

#[test]
fn malformed_toml_names_the_file() {
    let e = parse_err("0265.toml", "pid = = 1");
    assert!(e.contains("0265.toml"), "{e}");
}

#[test]
fn a_missing_field_names_the_file_and_the_field() {
    let e = parse_err("0265.toml", &GOOD.replace("name = \"RK S85 ES\"\n", ""));
    assert!(e.contains("0265.toml") && e.contains("name"), "{e}");
}

#[test]
fn an_unknown_protocol_is_rejected() {
    let e = parse_err("0265.toml", &GOOD.replace("\"v2\"", "\"v9\""));
    assert!(e.contains("0265.toml") && e.contains("v9"), "{e}");
}

#[test]
fn a_file_name_that_differs_from_the_pid_is_rejected() {
    let e = parse_err("0266.toml", GOOD);
    assert!(e.contains("0266.toml") && e.contains("0265"), "{e}");
}

#[test]
fn fw_and_protocol_must_agree() {
    let e = parse_err("0265.toml", &GOOD.replace("fw = 1", "fw = 2"));
    assert!(e.contains("0265.toml") && e.contains("fw"), "{e}");
}

#[test]
fn a_screen_without_size_or_with_an_unknown_kind_is_rejected() {
    let e = parse_err("0265.toml", &GOOD.replace("width = 320", "width = 0"));
    assert!(e.contains("0265.toml") && e.contains("screen"), "{e}");
    let e = parse_err("0265.toml", &GOOD.replace("kind = 1", "kind = 3"));
    assert!(e.contains("0265.toml") && e.contains("kind"), "{e}");
}

#[test]
fn a_layout_that_is_not_a_pid_is_rejected() {
    let e = parse_err("0265.toml", &GOOD.replace("01FA", "xyz"));
    assert!(e.contains("0265.toml") && e.contains("xyz"), "{e}");
}

#[test]
fn render_needs_at_least_one_model() {
    assert!(render(Vec::new()).is_err());
}

#[test]
fn render_rejects_duplicate_pids() {
    let a = parse_model("01FA.toml", LAYOUT_TARGET).unwrap();
    let b = parse_model("01FA.toml", LAYOUT_TARGET).unwrap();
    let e = render(vec![a, b]).unwrap_err();
    assert!(e.contains("01FA"), "{e}");
}

#[test]
fn render_rejects_a_layout_with_no_model() {
    let a = parse_model("0265.toml", GOOD).unwrap();
    let e = render(vec![a]).unwrap_err();
    assert!(e.contains("0265") && e.contains("01FA"), "{e}");
}

#[test]
fn render_sorts_by_pid_and_shares_equal_effect_tables() {
    let a = parse_model("0265.toml", GOOD).unwrap();
    let b = parse_model("01FA.toml", LAYOUT_TARGET).unwrap();
    let c_text = GOOD
        .replace("0x0265", "0x0266")
        .replace("layout = \"01FA\"\n", "");
    let c = parse_model("0266.toml", &c_text).unwrap();
    let out = render(vec![a, b, c]).unwrap();

    assert!(out.find("pid: 0x01FA").unwrap() < out.find("pid: 0x0265").unwrap());
    assert!(out.find("pid: 0x0265").unwrap() < out.find("pid: 0x0266").unwrap());
    assert_eq!(out.matches("name: \"Solid\"").count(), 1);
    assert!(out.contains("layout: Some(0x01FA)"));
    assert!(out.contains("draw_frame_time: 180"));
    assert!(out.contains("status: Status::Tested"));
    assert!(out.contains("protocol: Protocol::V1"));
}
