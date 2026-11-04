//! The real program on Linux being told to end: with no window and no keyboard, as a scripted demo
//! has neither, but with its signals as they really are. What a demo doesn't do is look at a keyboard,
//! which `unkludge-cli`'s tests of the same signals do through an upload.
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

fn bit(signal: i32) -> u64 {
    1 << (signal - 1)
}

/// A demo that would stay for a minute, waiting to take a picture.
fn start_demo(name: &str) -> (Child, PathBuf) {
    let folder =
        std::env::temp_dir().join(format!("unkludge-app-signals-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_unkludge-app"))
        .args(["--demo", "--demo-state", "no-keyboard", "--demo-shot"])
        .arg(folder.join("picture.png"))
        .args(["--demo-shot-delay", "60000", "--demo-exit-after", "60000"])
        .env("XDG_CONFIG_HOME", &folder)
        .env("XDG_RUNTIME_DIR", &folder)
        .env("XDG_DATA_HOME", &folder)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    (child, folder)
}

fn thread_masks(pid: u32) -> Vec<u64> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|task| {
            // While the watcher waits for signals the system shows them as not blocked on it, as
            // that's how waiting for them works.
            let name = std::fs::read_to_string(task.path().join("comm")).ok()?;
            if name.trim() == "signals" {
                return None;
            }
            let status = std::fs::read_to_string(task.path().join("status")).ok()?;
            let hex = status.lines().find_map(|l| l.strip_prefix("SigBlk:"))?;
            u64::from_str_radix(hex.trim(), 16).ok()
        })
        .collect()
}

/// Waits for the worker, which is the last thing starting up starts. So all there's going to be are
/// there by then, and the signal goes to a program that's settled.
fn wait_until_up(child: &Child) -> Vec<u64> {
    let started = Instant::now();
    loop {
        let masks = thread_masks(child.id());
        if masks.len() >= 2 {
            return masks;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never came up: {masks:x?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn kill(child: &Child, signal: i32) {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
}

/// How it ended, or kills it and says it hadn't.
fn wait_for_end(child: &mut Child) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(25) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("it's still running");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn is_told(signals: &[i32], name: &str) {
    let (mut child, folder) = start_demo(name);
    wait_until_up(&child);
    for &signal in signals {
        kill(&child, signal);
    }
    let status = wait_for_end(&mut child);
    assert_eq!(status.code(), Some(0), "{status:?}");
    // It quit in its usual way, which saves the settings.
    assert!(folder.join("unkludge/settings-demo.txt").exists());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn every_thread_has_the_signals_blocked() {
    let (mut child, folder) = start_demo("masks");
    let masks = wait_until_up(&child);
    let wanted = bit(SIGHUP) | bit(SIGINT) | bit(SIGTERM);
    assert!(
        masks.iter().all(|mask| mask & wanted == wanted),
        "{masks:x?}"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn sigterm_quits_it_in_its_usual_way() {
    is_told(&[SIGTERM], "term");
}

#[test]
fn two_sighups_close_together_quit_it_in_its_usual_way() {
    is_told(&[SIGHUP, SIGHUP], "hups");
}

#[test]
fn sigterm_then_sighup_quit_it_in_its_usual_way() {
    is_told(&[SIGTERM, SIGHUP], "term-hup");
}

#[test]
fn ctrl_c_quits_it_in_its_usual_way() {
    is_told(&[SIGINT], "int");
}
