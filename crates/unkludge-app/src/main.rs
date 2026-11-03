//! Unkludge's desktop app: a window and a tray icon on top of a thread that owns the keyboard.

#[path = "../../unkludge-cli/src/local_time.rs"]
mod local_time;
// The interface, which comes later, is what uses these.
#[allow(dead_code)]
mod i18n;
#[allow(dead_code)]
mod report;
#[path = "../../unkludge-cli/src/report_lines.rs"]
mod report_lines;
#[allow(dead_code)]
mod settings;
#[allow(dead_code)]
mod worker;

fn main() {}
