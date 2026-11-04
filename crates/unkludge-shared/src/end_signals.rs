//! Being told to stop, on Linux. A signal handler won't do here: a thread waiting in `poll` when a
//! handler runs gets `EINTR`, and hidapi passes that up as an error, so a keyboard that was fine would
//! look gone halfway through an upload, with no abort sent. So the signals stay blocked on every
//! thread and a thread of their own waits for them, and nothing else ever notices them.
//!
//! What each one means:
//! - The first of any sets the flag, and whoever is uploading cancels.
//! - A second Ctrl+C ends the process at once, for someone who won't sit through a stalled keyboard.
//! - SIGTERM and SIGHUP come from the system, and more of them often follow by themselves: closing a
//!   terminal gives two SIGHUPs less than a millisecond apart, one from the kernel and one from the
//!   shell. So further ones do nothing. The first starts a timer, so that the process can't hang on
//!   however long something takes to stop.
//! - A signal the process started with ignored stays ignored, which is how `nohup` works.

use std::io::Write;
use std::mem::zeroed;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const WATCHED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// What to do about signals.
#[derive(Clone, Copy)]
pub struct Watch {
    /// Set by the first one.
    pub flag: &'static AtomicBool,
    /// Goes to stderr on the first Ctrl+C.
    pub first_interrupt: Option<&'static str>,
    /// How long after the first SIGTERM or SIGHUP the process ends anyway. Whole seconds.
    pub backstop: Duration,
}

/// From here on the signals are blocked on this thread and any it starts, so this has to come before
/// any others start, or one of those would take the signal and be ended by it. Only the first call
/// counts. If this fails they just end the process as usual.
pub fn watch(watch: Watch) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let set = block_watched();
    let started = std::thread::Builder::new()
        .name("signals".to_string())
        .spawn(move || wait_for_signals(set, watch));
    if started.is_err() {
        // SAFETY: as above.
        unsafe {
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, null_mut());
        }
    }
}

/// Blocks on this thread those of the signals that the process didn't start with ignored, and says
/// which those are. Public for tests, whose harness has a thread running before any test does.
pub fn block_watched() -> libc::sigset_t {
    // SAFETY: the set is initialized by `sigemptyset` before anything else reads it, and null means
    // the old mask isn't wanted.
    unsafe {
        let mut set: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut set);
        for signal in WATCHED {
            if !started_ignored(signal) {
                libc::sigaddset(&mut set, signal);
            }
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, null_mut());
        set
    }
}

/// Whether the process was started with this signal ignored.
fn started_ignored(signal: libc::c_int) -> bool {
    // SAFETY: null means nothing changes, and the structure is written by the call.
    unsafe {
        let mut old: libc::sigaction = zeroed();
        libc::sigaction(signal, std::ptr::null(), &mut old) == 0
            && old.sa_sigaction == libc::SIG_IGN
    }
}

fn wait_for_signals(set: libc::sigset_t, watch: Watch) {
    let mut seen = Seen::default();
    loop {
        let mut signal = 0;
        // SAFETY: both point to what's valid for the call.
        if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
            continue;
        }
        match seen.next(signal) {
            Step::Interrupted => {
                watch.flag.store(true, Ordering::SeqCst);
                if let Some(message) = watch.first_interrupt {
                    let _ = std::io::stderr().write_all(message.as_bytes());
                }
            }
            Step::ToldToEnd => {
                watch.flag.store(true, Ordering::SeqCst);
                start_backstop(watch.backstop);
            }
            Step::Nothing => {}
            Step::EndNow => end_now(signal),
        }
    }
}

/// What a signal leads to, given those before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The first Ctrl+C.
    Interrupted,
    /// The first SIGTERM or SIGHUP.
    ToldToEnd,
    Nothing,
    EndNow,
}

#[derive(Default)]
struct Seen {
    interrupted: bool,
    told_to_end: bool,
}

impl Seen {
    fn next(&mut self, signal: libc::c_int) -> Step {
        if signal == libc::SIGINT {
            if std::mem::replace(&mut self.interrupted, true) {
                Step::EndNow
            } else {
                Step::Interrupted
            }
        } else if std::mem::replace(&mut self.told_to_end, true) {
            Step::Nothing
        } else {
            Step::ToldToEnd
        }
    }
}

/// `SIGALRM` isn't one of those blocked, and its default ends the process.
fn start_backstop(after: Duration) {
    let seconds = u32::try_from(after.as_secs()).unwrap_or(u32::MAX).max(1);
    // SAFETY: plain calls with plain arguments.
    unsafe {
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::alarm(seconds);
    }
}

/// Ends the process the way this signal would have, so that whoever started it can see why.
fn end_now(signal: libc::c_int) -> ! {
    // SAFETY: the set is initialized by `sigemptyset` before anything else reads it. Unblocked on this
    // thread, with its default back, the signal raised here ends the process.
    unsafe {
        let mut set: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, signal);
        libc::signal(signal, libc::SIG_DFL);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, null_mut());
        libc::raise(signal);
        libc::_exit(128 + signal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    fn steps(signals: &[libc::c_int]) -> Vec<Step> {
        let mut seen = Seen::default();
        signals.iter().map(|&signal| seen.next(signal)).collect()
    }

    #[test]
    fn only_a_second_ctrl_c_ends_it() {
        assert_eq!(
            steps(&[libc::SIGINT, libc::SIGINT]),
            [Step::Interrupted, Step::EndNow]
        );
    }

    #[test]
    fn more_signals_from_the_system_do_nothing() {
        use Step::{Nothing, ToldToEnd};
        assert_eq!(steps(&[libc::SIGHUP, libc::SIGHUP]), [ToldToEnd, Nothing]);
        assert_eq!(steps(&[libc::SIGTERM, libc::SIGHUP]), [ToldToEnd, Nothing]);
        assert_eq!(
            steps(&[libc::SIGHUP, libc::SIGTERM, libc::SIGTERM]),
            [ToldToEnd, Nothing, Nothing]
        );
    }

    #[test]
    fn someone_at_the_keyboard_can_still_end_it_after_the_system_asked() {
        assert_eq!(
            steps(&[libc::SIGTERM, libc::SIGINT, libc::SIGINT]),
            [Step::ToldToEnd, Step::Interrupted, Step::EndNow]
        );
    }

    #[test]
    fn the_system_asking_after_a_ctrl_c_starts_the_timer_all_the_same() {
        assert_eq!(
            steps(&[libc::SIGINT, libc::SIGTERM]),
            [Step::Interrupted, Step::ToldToEnd]
        );
    }

    const HELPER_VARIABLE: &str = "UNKLUDGE_TEST_WATCHING";
    static HELPER_FLAG: AtomicBool = AtomicBool::new(false);

    /// The test harness has a main thread by the time a test runs, and a signal would go to that one.
    /// So in the helper this comes before it, as `watch` does in a real program's only thread.
    #[used]
    #[unsafe(link_section = ".init_array")]
    static BLOCK_EARLY: extern "C" fn() = block_early;

    extern "C" fn block_early() {
        // SAFETY: the name is a string that ends in NUL. Nothing else is running yet.
        if !unsafe { libc::getenv(c"UNKLUDGE_TEST_WATCHING".as_ptr()) }.is_null() {
            block_watched();
        }
    }

    /// Watches signals in a process of its own, as they go for the whole process. A thread it starts
    /// should have them blocked too. It stays until something ends it.
    #[test]
    #[ignore = "started by the tests below, in a process of its own"]
    fn a_process_watching_signals() {
        if std::env::var_os(HELPER_VARIABLE).is_none() {
            return;
        }
        watch(Watch {
            flag: &HELPER_FLAG,
            first_interrupt: None,
            backstop: Duration::from_secs(1),
        });
        std::thread::spawn(|| std::thread::sleep(Duration::from_secs(60)));
        std::thread::sleep(Duration::from_secs(60));
    }

    fn start_helper(hup_ignored: bool) -> Child {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "end_signals::tests::a_process_watching_signals",
                "--ignored",
                "--nocapture",
            ])
            .env(HELPER_VARIABLE, "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if hup_ignored {
            // SAFETY: only a call that's safe between fork and exec.
            unsafe {
                command.pre_exec(|| {
                    libc::signal(libc::SIGHUP, libc::SIG_IGN);
                    Ok(())
                });
            }
        }
        command.spawn().unwrap()
    }

    /// The threads to look at. While the watcher waits for signals the system shows them as not
    /// blocked on it, as that's how waiting for them works.
    fn others_than_the_watcher(pid: u32) -> Vec<String> {
        tasks(pid)
            .into_iter()
            .filter(|task| {
                let name = std::fs::read_to_string(format!("/proc/{pid}/task/{task}/comm"));
                name.is_ok_and(|name| name.trim() != "signals")
            })
            .collect()
    }

    /// A line of a thread's status in `/proc`, as a number.
    fn mask(pid: u32, task: &str, line: &str) -> u64 {
        let path = format!("/proc/{pid}/task/{task}/status");
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines()
            .find_map(|l| l.strip_prefix(line))
            .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
            .unwrap_or(0)
    }

    fn bit(signal: libc::c_int) -> u64 {
        1 << (signal - 1)
    }

    fn tasks(pid: u32) -> Vec<String> {
        std::fs::read_dir(format!("/proc/{pid}/task"))
            .map(|dir| {
                dir.flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Waits for the helper's own thread to have started, which is after `watch`: the harness's, the
    /// watcher's, and that one.
    fn wait_until_watching(pid: u32, wanted: u64) {
        let started = Instant::now();
        loop {
            let others = others_than_the_watcher(pid);
            if tasks(pid).len() >= 3
                && others
                    .iter()
                    .all(|task| mask(pid, task, "SigBlk:") & wanted == wanted)
            {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "never watching"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn kill(child: &Child, signal: libc::c_int) {
        // SAFETY: a plain call. The process is still there, as nothing has waited for it.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
    }

    #[test]
    fn the_timer_ends_a_process_that_doesnt_stop_by_itself() {
        let mut helper = start_helper(false);
        wait_until_watching(helper.id(), bit(libc::SIGTERM));
        kill(&helper, libc::SIGTERM);
        // Not the signal that asked: that one only sets the flag.
        let status = helper.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGALRM), "{status:?}");
    }

    #[test]
    fn a_signal_the_process_started_with_ignored_stays_ignored() {
        let mut helper = start_helper(true);
        let wanted = bit(libc::SIGTERM);
        wait_until_watching(helper.id(), wanted);
        let main_task = helper.id().to_string();
        let ignored = mask(helper.id(), &main_task, "SigIgn:");
        assert_ne!(ignored & bit(libc::SIGHUP), 0, "{ignored:x}");
        for task in tasks(helper.id()) {
            let blocked = mask(helper.id(), &task, "SigBlk:");
            assert_eq!(blocked & bit(libc::SIGHUP), 0, "{task}: {blocked:x}");
        }
        // It would set the timer off if it counted, which would end the process in a second.
        kill(&helper, libc::SIGHUP);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(helper.try_wait().unwrap().is_none(), "ended");
        helper.kill().unwrap();
        helper.wait().unwrap();
    }
}
