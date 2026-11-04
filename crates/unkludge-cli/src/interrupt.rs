//! Being told to stop during an upload. Ctrl+C asks the upload to stop, so that the keyboard gets its
//! abort. The second is left to do what it always does, for when the keyboard has stalled and the
//! wait for an ack is more than someone wants to sit through.
//!
//! The system ending the program goes the same way, as far as it leaves time and sends word. On Linux
//! that's SIGTERM and SIGHUP, which `unkludge_shared::end_signals` deals with. On Windows it's closing
//! the console, with about 5 seconds to go. Windows ends a program like this one outright at logoff
//! and shutdown, as those events only reach services, so there's nothing to do then. A keyboard that
//! has stalled can take longer than the time left to get through its ack waits, and then it's left
//! unfinished, just as a process killed outright is.

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::{Duration, Instant};

static CANCEL: AtomicBool = AtomicBool::new(false);
static FINISHED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static INSTALLED: AtomicBool = AtomicBool::new(false);

const MESSAGE: &str = "\ncancelling… (Ctrl+C again to quit)\n";

/// Set by the first Ctrl+C after [`install`].
pub fn flag() -> &'static AtomicBool {
    &CANCEL
}

/// The upload has ended, so a console that's closing needn't wait for it any longer.
pub fn finished() {
    FINISHED.store(true, Ordering::SeqCst);
}

/// Whether [`finished`] has been called, for tests.
#[cfg(test)]
pub fn has_finished() -> bool {
    FINISHED.load(Ordering::SeqCst)
}

/// Whether [`install`] has been called, for tests.
#[cfg(test)]
pub fn is_installed() -> bool {
    INSTALLED.load(Ordering::SeqCst)
}

/// Whether this press is the first.
#[cfg(windows)]
fn first_press(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

/// How long a console that's closing waits for the upload to end. The system gives a program 5
/// seconds, and then ends it whatever it's doing.
#[cfg(windows)]
const CLOSING_WAIT: Duration = Duration::from_millis(4500);

/// From here on, Ctrl+C sets [`flag`]. If this fails, Ctrl+C just ends the process as usual.
#[cfg(windows)]
pub fn install() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    #[cfg(test)]
    INSTALLED.store(true, Ordering::SeqCst);
    // SAFETY: the handler is a function that lives as long as the process.
    unsafe {
        SetConsoleCtrlHandler(Some(on_console_event), 1);
    }
}

/// What a console event means here.
#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
enum ConsoleEvent {
    Press,
    /// The process ends when the handler returns, or when its time is up. Of these only closing the
    /// console reaches a program like this one, but what the others would need is the same.
    Closing,
    Other,
}

#[cfg(windows)]
fn console_event(event: u32) -> ConsoleEvent {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };

    match event {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => ConsoleEvent::Press,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => ConsoleEvent::Closing,
        _ => ConsoleEvent::Other,
    }
}

/// Asks the upload to stop, and holds off the process's end until it has, or until `longest`.
#[cfg(windows)]
fn stop_before_closing(cancel: &AtomicBool, finished: &AtomicBool, longest: Duration) {
    cancel.store(true, Ordering::SeqCst);
    let started = Instant::now();
    while !finished.load(Ordering::SeqCst) && started.elapsed() < longest {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Runs on a thread of its own, so it can print and wait as usual. Not handling an event hands it on
/// to the system's handler, which ends the process.
#[cfg(windows)]
unsafe extern "system" fn on_console_event(event: u32) -> windows_sys::core::BOOL {
    use std::io::Write;

    match console_event(event) {
        ConsoleEvent::Press if first_press(&CANCEL) => {
            let _ = std::io::stderr().write_all(MESSAGE.as_bytes());
            1
        }
        ConsoleEvent::Closing => {
            stop_before_closing(&CANCEL, &FINISHED, CLOSING_WAIT);
            1
        }
        ConsoleEvent::Press | ConsoleEvent::Other => 0,
    }
}

/// From here on, Ctrl+C, SIGTERM and SIGHUP set [`flag`]. If this fails, they just end the process
/// as usual. This process has no other threads yet.
#[cfg(not(windows))]
pub fn install() {
    use unkludge_shared::end_signals::{Watch, watch};

    /// Past the longest an abort can take on a stalled keyboard, which is 16 s.
    const BACKSTOP: std::time::Duration = std::time::Duration::from_secs(30);

    #[cfg(test)]
    INSTALLED.store(true, Ordering::SeqCst);
    watch(Watch {
        flag: &CANCEL,
        first_interrupt: Some(MESSAGE),
        backstop: BACKSTOP,
    });
}

#[cfg(all(test, windows))]
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
    #[test]
    fn the_handler_takes_the_first_ctrl_c_and_passes_on_the_rest() {
        use windows_sys::Win32::System::Console::CTRL_C_EVENT;

        // SAFETY: called as the system would call it.
        unsafe {
            assert_eq!(on_console_event(99), 0, "not one of these");
            assert!(!flag().load(Ordering::SeqCst));
            assert_eq!(on_console_event(CTRL_C_EVENT), 1, "the first");
            assert!(flag().load(Ordering::SeqCst));
            assert_eq!(on_console_event(CTRL_C_EVENT), 0, "the second");
        }
    }

    #[test]
    fn the_system_ending_things_counts_as_closing_and_ctrl_c_as_a_press() {
        use windows_sys::Win32::System::Console::{
            CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT,
            CTRL_SHUTDOWN_EVENT,
        };

        for event in [CTRL_C_EVENT, CTRL_BREAK_EVENT] {
            assert_eq!(console_event(event), ConsoleEvent::Press);
        }
        for event in [CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT] {
            assert_eq!(console_event(event), ConsoleEvent::Closing);
        }
        assert_eq!(console_event(99), ConsoleEvent::Other);
    }

    #[test]
    fn a_console_closing_cancels_the_upload_and_waits_for_it() {
        use std::sync::Arc;

        let cancel = AtomicBool::new(false);
        let finished = Arc::new(AtomicBool::new(false));
        let ending = Arc::clone(&finished);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            ending.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        stop_before_closing(&cancel, &finished, Duration::from_secs(30));
        assert!(cancel.load(Ordering::SeqCst));
        assert!(finished.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn a_console_closing_doesnt_wait_past_its_time() {
        let cancel = AtomicBool::new(false);
        let never = AtomicBool::new(false);
        let started = Instant::now();
        stop_before_closing(&cancel, &never, Duration::from_millis(30));
        assert!(cancel.load(Ordering::SeqCst));
        assert!(started.elapsed() >= Duration::from_millis(30));
    }
}
