//! Unkludge's desktop app: a window and a tray icon on top of a thread that owns the keyboard.

mod args;
mod crash;
// The interface, which comes later, is what uses these.
#[allow(dead_code)]
mod demo;
#[allow(dead_code)]
mod i18n;
#[allow(dead_code)]
mod icon;
#[allow(dead_code)]
mod layout;
#[allow(dead_code)]
mod log;
#[allow(dead_code)]
mod report;
#[allow(dead_code)]
mod settings;
#[allow(dead_code)]
mod state;
#[allow(dead_code)]
mod worker;

// For `build.rs`, and compiled here for its tests.
#[cfg(test)]
mod winres;

// One implementation per system.
#[allow(dead_code)]
#[cfg_attr(windows, path = "sys_windows.rs")]
#[cfg_attr(target_os = "linux", path = "sys_linux.rs")]
mod sys;

fn main() {
    crash::install();
    if let Err(problem) = args::parse(std::env::args().skip(1)) {
        log::error(&problem);
        std::process::exit(2);
    }
}
