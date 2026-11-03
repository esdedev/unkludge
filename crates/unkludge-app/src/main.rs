//! Unkludge's desktop app: a window and a tray icon on top of a thread that owns the keyboard.

#[path = "../../unkludge-cli/src/local_time.rs"]
mod local_time;
// The interface, which comes later, is what uses this.
#[allow(dead_code)]
mod worker;

fn main() {}
