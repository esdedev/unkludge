//! Runs the real binary. Every test here is a dry run or `models`, which can't reach a keyboard.
//! Nothing in this file may run another command without `--dry-run`, as it would open whatever is
//! plugged in.

use std::path::PathBuf;
use std::process::{Command, Output};

fn unkludge(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_unkludge"))
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn models_lists_the_29_and_needs_no_keyboard() {
    let output = unkludge(&["models"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1 + 29);
    assert!(text.contains("0265 RK S85 ES"));
}

#[test]
fn a_dry_run_without_pid_is_an_error_that_says_so() {
    for command in ["info", "light", "clock", "report"] {
        let output = unkludge(&["--dry-run", command]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert!(output.stdout.is_empty(), "{command}");
        let text = stderr(&output);
        assert!(text.starts_with("error: "), "{text}");
        assert!(text.contains("--dry-run needs --pid"), "{text}");
    }
}

#[test]
fn a_dry_run_of_an_unsupported_model_is_an_error() {
    for pid in ["002A", "FFFE"] {
        let output = unkludge(&["--dry-run", "--pid", pid, "clock"]);
        assert_eq!(output.status.code(), Some(1), "{pid}");
        assert!(stderr(&output).contains("isn't supported"), "{pid}");
    }
}

#[test]
fn bad_arguments_exit_with_1_and_say_why_on_stderr() {
    for args in [
        &["--dry-run", "--pid", "nope", "clock"][..],
        &["--verbose", "models"][..],
        &["frobnicate"][..],
        &[][..],
    ] {
        let output = unkludge(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn help_and_version_are_not_errors() {
    for args in [
        &["--help"][..],
        &["light", "--help"][..],
        &["--version"][..],
    ] {
        let output = unkludge(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(!output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn light_with_no_options_prints_the_one_read() {
    let output = unkludge(&["--dry-run", "--pid", "0265", "light"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert_eq!(text.matches("09 ").count(), 1, "{text}");
    assert!(
        text.contains("09 84 00 00 01 00 80 00 (+512 zeros)"),
        "{text}"
    );
    assert!(text.contains("Reports: 1 x 0x84"), "{text}");
    assert!(text.contains("Mode: off"), "{text}");
}

#[test]
fn light_with_a_mode_prints_the_read_and_the_write() {
    let output = unkludge(&["--dry-run", "--pid", "0265", "light", "--mode", "2"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("09 84 00 00 01 00 80 00"), "{text}");
    assert!(text.contains("09 04 00 00 01 00 80 00"), "{text}");
    assert!(text.contains("Reports: 1 x 0x84, 1 x 0x04"), "{text}");
    assert!(text.contains("Mode: 2 (Breathing)"), "{text}");
}

#[test]
fn clock_prints_the_one_report() {
    let output = unkludge(&["clock", "--dry-run", "--pid", "0265"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("09 0B 00 00 01 00 0E 00 00 00 00"), "{text}");
    assert!(text.contains("Reports: 1 x 0x0B"), "{text}");
}

#[test]
fn timing_says_when_there_are_no_acks() {
    let output = unkludge(&["--dry-run", "--pid", "0265", "--timing", "clock"]);
    assert!(stdout(&output).contains("Acks: none"));
}

/// Writes a small image to a file of this test's own.
fn image_file(name: &str) -> PathBuf {
    use image::{ImageFormat, Rgba, RgbaImage};
    let path =
        std::env::temp_dir().join(format!("unkludge-cli-it-{}-{name}.png", std::process::id()));
    RgbaImage::from_pixel(32, 32, Rgba([255, 0, 0, 255]))
        .save_with_format(&path, ImageFormat::Png)
        .unwrap();
    path
}

#[test]
fn screen_prints_a_summary_not_every_chunk() {
    let path = image_file("summary");
    let output = unkludge(&[
        "--dry-run",
        "--pid",
        "0265",
        "--timing",
        "screen",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.lines().count() < 12, "{text}");
    assert!(
        text.contains("... 213 more chunks of frame 0 ..."),
        "{text}"
    );
    assert!(text.contains("Reports: 2 x 0x0D, 215 x 0x0C"), "{text}");
    assert!(text.contains("Acks: 217, "), "{text}");
}

#[test]
fn verbose_prints_every_chunk_in_full() {
    let path = image_file("verbose");
    let output = unkludge(&[
        "--dry-run",
        "--pid",
        "0265",
        "--verbose",
        "screen",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert_eq!(text.matches("09 0C 00 ").count(), 215, "chunks");
}

#[test]
fn a_model_with_two_collections_shows_which_is_which() {
    let path = image_file("two-collections");
    let output = unkludge(&[
        "--dry-run",
        "--pid",
        "01B8",
        "screen",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("main:   09 0D"), "{text}");
    assert!(text.contains("screen: 09 0D"), "{text}");
    assert!(text.contains("screen: 09 0C"), "{text}");
    assert!(!text.contains("main:   09 0C"), "{text}");
}
