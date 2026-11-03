//! Ctrl+C during an upload. The first press asks the upload to stop, so that the keyboard gets its
//! abort. The second is left to do what it always does, for when the keyboard has stalled and the
//! wait for an ack is more than someone wants to sit through.

use std::sync::atomic::{AtomicBool, Ordering};

static CANCEL: AtomicBool = AtomicBool::new(false);

const MESSAGE: &str = "\ncancelling… (Ctrl+C again to quit)\n";

/// Set by the first Ctrl+C after [`install`].
pub fn flag() -> &'static AtomicBool {
    &CANCEL
}

/// Whether this press is the first.
fn first_press(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

/// From here on, Ctrl+C sets [`flag`]. If this fails, Ctrl+C just ends the process as usual.
#[cfg(windows)]
pub fn install() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // SAFETY: the handler is a function that lives as long as the process.
    unsafe {
        SetConsoleCtrlHandler(Some(on_console_event), 1);
    }
}

/// Runs on a thread of its own, so it can print as usual. Not handling an event hands it on to the
/// system's handler, which ends the process.
#[cfg(windows)]
unsafe extern "system" fn on_console_event(event: u32) -> windows_sys::core::BOOL {
    use std::io::Write;
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};

    if (event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT) || !first_press(&CANCEL) {
        return 0;
    }
    let _ = std::io::stderr().write_all(MESSAGE.as_bytes());
    1
}

/// From here on, Ctrl+C sets [`flag`]. If this fails, Ctrl+C just ends the process as usual.
#[cfg(not(windows))]
pub fn install() {
    // SAFETY: the handler only makes calls that are safe in a signal handler, and lives as long as
    // the process.
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
    }
}

/// Puts the default back at once, so the next press ends the process. Everything here must be safe
/// in a signal handler: an atomic, `signal` and `write`, and nothing that allocates or locks.
#[cfg(not(windows))]
extern "C" fn on_sigint(_signal: libc::c_int) {
    if first_press(&CANCEL) {
        // SAFETY: both calls are safe in a signal handler, and the buffer is valid for its length.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::write(2, MESSAGE.as_ptr().cast(), MESSAGE.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_press_counts_as_first() {
        let flag = AtomicBool::new(false);
        assert!(first_press(&flag));
        assert!(flag.load(Ordering::SeqCst));
        assert!(!first_press(&flag));
        assert!(flag.load(Ordering::SeqCst));
    }

    /// Uses up the process's one first press, so it's the only test here that touches [`CANCEL`].
    #[cfg(windows)]
    #[test]
    fn the_handler_takes_the_first_ctrl_c_and_passes_on_the_rest() {
        use windows_sys::Win32::System::Console::{CTRL_C_EVENT, CTRL_CLOSE_EVENT};

        install();
        // SAFETY: called as the system would call it.
        unsafe {
            assert_eq!(on_console_event(CTRL_CLOSE_EVENT), 0, "not a Ctrl+C");
            assert!(!flag().load(Ordering::SeqCst));
            assert_eq!(on_console_event(CTRL_C_EVENT), 1, "the first");
            assert!(flag().load(Ordering::SeqCst));
            assert_eq!(on_console_event(CTRL_C_EVENT), 0, "the second");
        }
    }

    /// Uses up the process's one first press, so it's the only test here that touches [`CANCEL`].
    /// Only one signal gets raised: a second would end the test run, as it's meant to.
    #[cfg(not(windows))]
    #[test]
    fn the_first_signal_sets_the_flag_and_puts_the_default_back() {
        install();
        assert!(!flag().load(Ordering::SeqCst));
        // SAFETY: the handler is in place, and raising a signal has no other requirements.
        unsafe {
            assert_eq!(libc::raise(libc::SIGINT), 0);
            assert!(flag().load(Ordering::SeqCst));
            let current = libc::signal(libc::SIGINT, libc::SIG_DFL);
            assert_eq!(current, libc::SIG_DFL);
        }
    }
}
