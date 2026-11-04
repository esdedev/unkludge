//! What Unkludge's command-line tool and app have in common, none of which is about the protocol:
//! the local time for the clock, the official app's process, the wording of reports, and the lock
//! that keeps them off the keyboard at the same time.

pub mod device_lock;
#[cfg(not(windows))]
pub mod linux_dirs;
pub mod local_time;
pub mod report_lines;
pub mod rk_app;
