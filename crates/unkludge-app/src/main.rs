//! Unkludge's desktop app: a window and a tray icon on top of a thread that owns the keyboard.

mod crash;
// The interface, which comes later, is what uses these.
#[allow(dead_code)]
mod i18n;
#[path = "../../unkludge-cli/src/local_time.rs"]
mod local_time;
#[allow(dead_code)]
mod report;
#[path = "../../unkludge-cli/src/report_lines.rs"]
mod report_lines;
#[path = "../../unkludge-cli/src/rk_app.rs"]
mod rk_app;
#[allow(dead_code)]
mod settings;
#[allow(dead_code)]
mod worker;

// One implementation per system.
#[allow(dead_code)]
#[cfg_attr(windows, path = "sys_windows.rs")]
#[cfg_attr(target_os = "linux", path = "sys_linux.rs")]
mod sys;

fn main() {
    crash::install();
}
