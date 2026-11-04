//! Being told to stop during an upload. Ctrl+C asks the upload to stop, so that the keyboard gets its
//! abort. The second is left to do what it always does, for when the keyboard has stalled and the
//! wait for an ack is more than someone wants to sit through.
//!
//! The system ending the program goes the same way, as far as it leaves time. On Linux that's SIGTERM
//! and SIGHUP, and on Windows closing the console, logging off and shutting down. A keyboard that
//! has stalled can take longer than that to get through its ack waits, and then it's left unfinished,
//! just as a process killed outright is.

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::{Duration, Instant};

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

/// The upload has ended, so a console that's closing needn't wait for it any longer.
#[cfg(windows)]
pub fn finished() {
    FINISHED.store(true, Ordering::SeqCst);
}

/// Nothing is waiting for this here: a signal's handler has returned long before.
#[cfg(not(windows))]
pub fn finished() {}

#[cfg(windows)]
static FINISHED: AtomicBool = AtomicBool::new(false);

/// How long a console that's closing waits for the upload to end. The system gives a program about
/// 5 seconds on these events, and then ends it whatever it's doing.
#[cfg(windows)]
const CLOSING_WAIT: Duration = Duration::from_millis(4500);

/// From here on, Ctrl+C sets [`flag`]. If this fails, Ctrl+C just ends the process as usual.
#[cfg(windows)]
pub fn install() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

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
    /// The process ends when the handler returns, or when its time is up.
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
/// as usual.
#[cfg(not(windows))]
pub fn install() {
    // SAFETY: the handler only makes calls that are safe in a signal handler, and lives as long as
    // the process.
    unsafe {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
}

/// Puts this signal's default back at once, so that the next one ends the process. One that isn't the
/// first of any kind ends it now. Everything here must be safe in a signal handler: an atomic,
/// `signal`, `raise` and `write`, and nothing that allocates or locks.
#[cfg(not(windows))]
extern "C" fn on_signal(signal: libc::c_int) {
    let first = first_press(&CANCEL);
    // SAFETY: all three calls are safe in a signal handler, and the buffer is valid for its length.
    // This signal is blocked until the handler returns, which is when `raise` takes effect.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        if first {
            libc::write(2, MESSAGE.as_ptr().cast(), MESSAGE.len());
        } else {
            libc::raise(signal);
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
            assert_eq!(on_console_event(99), 0, "not one of these");
            assert!(!flag().load(Ordering::SeqCst));
            assert_eq!(on_console_event(CTRL_C_EVENT), 1, "the first");
            assert!(flag().load(Ordering::SeqCst));
            assert_eq!(on_console_event(CTRL_C_EVENT), 0, "the second");
            // With the upload over there's nothing to wait for.
            finished();
            assert_eq!(on_console_event(CTRL_CLOSE_EVENT), 1, "closing");
        }
    }

    #[cfg(windows)]
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

    #[cfg(windows)]
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

    #[cfg(windows)]
    #[test]
    fn a_console_closing_doesnt_wait_past_its_time() {
        let cancel = AtomicBool::new(false);
        let never = AtomicBool::new(false);
        let started = Instant::now();
        stop_before_closing(&cancel, &never, Duration::from_millis(30));
        assert!(cancel.load(Ordering::SeqCst));
        assert!(started.elapsed() >= Duration::from_millis(30));
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

    #[cfg(not(windows))]
    const SIGNAL_VARIABLE: &str = "UNKLUDGE_TEST_SIGNAL";

    /// Raises a signal for real, and then another. A process of its own, which the test below starts,
    /// as what comes of the second is that the process ends.
    #[cfg(not(windows))]
    #[test]
    #[ignore = "started by the test below, in a process of its own"]
    fn a_process_that_gets_signals() {
        let Some(signal) = std::env::var(SIGNAL_VARIABLE)
            .ok()
            .and_then(|text| text.parse::<libc::c_int>().ok())
        else {
            return;
        };
        install();
        // SAFETY: the handler is in place, and raising a signal has no other requirements.
        unsafe {
            assert_eq!(libc::raise(signal), 0);
            assert!(flag().load(Ordering::SeqCst), "the first only cancels");
            println!("the first was only a cancel");
            assert_eq!(libc::raise(signal), 0);
        }
        println!("still here after the second");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_system_ending_things_cancels_the_upload_first() {
        use std::os::unix::process::ExitStatusExt;

        for signal in [libc::SIGTERM, libc::SIGHUP] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "interrupt::tests::a_process_that_gets_signals",
                    "--ignored",
                    "--nocapture",
                ])
                .env(SIGNAL_VARIABLE, signal.to_string())
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.contains("only a cancel"), "{signal}: {text}");
            assert!(!text.contains("still here"), "{signal}: {text}");
            assert_eq!(output.status.signal(), Some(signal), "{text}");
        }
    }
}
