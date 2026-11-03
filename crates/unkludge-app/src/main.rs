//! Unkludge's desktop app: a window and a tray icon on top of a thread that owns the keyboard.
//! `--tray` starts hidden in the tray, which is how it starts with the system. `--demo` shows the
//! interface with a keyboard that doesn't exist.

// No console window in a release build. A debug build keeps its console, for what it prints.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// The Linux interface comes later, and it's what will use most of these there.
#![cfg_attr(not(windows), allow(dead_code))]

mod args;
mod crash;
mod demo;
mod i18n;
mod icon;
mod layout;
mod log;
mod report;
mod session;
mod settings;
mod state;
mod worker;

// The app itself only needs the PNG writer, for the demo's pictures. The rest is for `build.rs`, and
// compiled here for its tests.
#[cfg(any(windows, test))]
#[cfg_attr(not(test), allow(dead_code))]
mod winres;

// One implementation per system.
#[cfg_attr(windows, path = "sys_windows.rs")]
#[cfg_attr(target_os = "linux", path = "sys_linux.rs")]
mod sys;

#[cfg(windows)]
mod ui_windows;

use args::{Args, Language};

fn main() {
    crash::install();
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        // A flag this doesn't know could be a mistyped --demo, so nothing starts.
        Err(problem) => {
            log::error(&problem);
            #[cfg(windows)]
            ui_windows::refuse_start(&problem);
            std::process::exit(2);
        }
    };
    if let Some(language) = args.language {
        i18n::choose(language == Language::Spanish);
    }
    run(args);
}

#[cfg(windows)]
fn run(args: Args) {
    ui_windows::run(args);
}

#[cfg(not(windows))]
fn run(_args: Args) {
    eprintln!("unkludge: the Linux interface comes in the next task. The CLI works meanwhile.");
}
