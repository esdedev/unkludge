//! Real signals at a real upload, in processes of their own. There's no keyboard here, so the
//! keyboard is a stand-in that waits for its acks the way hidapi does: in `poll`, saying so as an error
//! if that gets interrupted. The rest is the same code as the real thing.

use std::cell::Cell;
use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;
use unkludge_protocol::command_byte::{CONFIG_READ, SCREEN_CONTROL, SCREEN_DATA};
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error, HEADER_LEN, REPORT_ID, model};

use crate::args::{Cli, Command};
use crate::drive;

const MARKER_VARIABLE: &str = "UNKLUDGE_TEST_MARKER";
const START: u8 = 1;
const ABORT: u8 = 2;

/// The test harness has a main thread by the time a test runs, and a signal would go to that one. So
/// in the helper this comes before it. A real `unkludge` has only the one thread when it installs.
#[used]
#[unsafe(link_section = ".init_array")]
static BLOCK_EARLY: extern "C" fn() = block_early;

extern "C" fn block_early() {
    // SAFETY: the name is a string that ends in NUL. Nothing else is running yet.
    if !unsafe { libc::getenv(c"UNKLUDGE_TEST_MARKER".as_ptr()) }.is_null() {
        unkludge_shared::end_signals::block_watched();
    }
}

fn note(marker: &Path, what: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(marker) {
        let _ = writeln!(file, "{what}");
    }
}

/// Answers as a keyboard would, a little slowly, and says what happened in a file.
struct PollingKeyboard {
    marker: PathBuf,
    /// Never written to, so that `poll` just waits.
    quiet: OwnedFd,
    _other_end: OwnedFd,
    config_requested: Cell<bool>,
    ack: Cell<Option<[u8; 8]>>,
    /// The start's ack takes its time, for a signal to land during the wait for it.
    ack_not_before: Cell<Option<Instant>>,
}

impl PollingKeyboard {
    fn new(marker: PathBuf) -> PollingKeyboard {
        let mut pipe = [0; 2];
        // SAFETY: the two descriptors are written by the call and owned from here on.
        let (quiet, other_end) = unsafe {
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
            (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1]))
        };
        PollingKeyboard {
            marker,
            quiet,
            _other_end: other_end,
            config_requested: Cell::new(false),
            ack: Cell::new(None),
            ack_not_before: Cell::new(None),
        }
    }
}

impl Transport for PollingKeyboard {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        match report[1] {
            CONFIG_READ => self.config_requested.set(true),
            SCREEN_DATA => {
                note(&self.marker, "uploading");
                self.ack
                    .set(Some([REPORT_ID, 0x0A, 0x06, 1, report[4], report[5], 0, 0]));
            }
            SCREEN_CONTROL => {
                match report[HEADER_LEN] >> 6 {
                    START => {
                        note(&self.marker, "starting");
                        self.ack_not_before
                            .set(Some(Instant::now() + Duration::from_millis(400)));
                    }
                    ABORT => note(&self.marker, "abort"),
                    _ => {}
                }
                self.ack.set(Some([REPORT_ID, 0x0A, 0x06, 1, 0, 0, 0, 0]));
            }
            _ => {}
        }
        Ok(())
    }

    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        if !self.config_requested.take() {
            return Err(Error::Hid("nothing was asked for".to_string()));
        }
        buf.fill(0);
        buf[..8].copy_from_slice(&[REPORT_ID, CONFIG_READ, 0, 0, 1, 0, 0x80, 0]);
        buf[8 + 126..8 + 128].copy_from_slice(&[0x5A, 0xA5]);
        Ok(buf.len())
    }

    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error> {
        let mut fds = libc::pollfd {
            fd: self.quiet.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: the structure and its count go together.
        if unsafe { libc::poll(&mut fds, 1, ms.min(20)) } == -1 {
            return Err(Error::Hid(std::io::Error::last_os_error().to_string()));
        }
        if self
            .ack_not_before
            .get()
            .is_some_and(|at| Instant::now() < at)
        {
            return Ok(0);
        }
        match self.ack.take() {
            Some(ack) => {
                buf[..ack.len()].copy_from_slice(&ack);
                Ok(ack.len())
            }
            None => Ok(0),
        }
    }
}

/// A GIF with enough frames for the upload to last far longer than any test.
fn long_gif(path: &Path) {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};

    let mut encoder = GifEncoder::new(File::create(path).unwrap());
    for value in 0..4 {
        let image = RgbaImage::from_pixel(8, 8, Rgba([value * 40, 0, 0, 255]));
        let delay = Delay::from_numer_denom_ms(200, 1);
        encoder
            .encode_frame(Frame::from_parts(image, 0, 0, delay))
            .unwrap();
    }
}

/// Runs an upload as `unkludge screen` does, and says how it ended.
#[test]
#[ignore = "started by the tests below, in a process of its own"]
fn an_upload_that_gets_told_to_stop() {
    let Some(marker) = std::env::var_os(MARKER_VARIABLE).map(PathBuf::from) else {
        return;
    };
    let gif = marker.with_extension("gif");
    long_gif(&gif);
    let cli = Cli::try_parse_from(["unkludge", "screen", gif.to_str().unwrap()]).unwrap();
    let Command::Keyboard(command) = &cli.command else {
        unreachable!();
    };
    let keyboard = PollingKeyboard::new(marker.clone());
    let device = Device::new(keyboard, None, model::find(0x0265).unwrap()).unwrap();
    let ended = match drive(device, &cli, command, &[]) {
        Ok(()) => "done".to_string(),
        Err(error) if error.to_string() == "cancelled" => "cancelled".to_string(),
        Err(error) => format!("failed: {error}"),
    };
    note(&marker, &ended);
    note(&marker, "end");
}

fn marker_for(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("unkludge-signals-{}-{name}.txt", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn helper_args() -> [&'static str; 4] {
    [
        "--exact",
        "signal_tests::an_upload_that_gets_told_to_stop",
        "--ignored",
        "--nocapture",
    ]
}

fn start_upload(marker: &Path, hup_ignored: bool) -> Child {
    let mut command = Process::new(std::env::current_exe().unwrap());
    command
        .args(helper_args())
        .env(MARKER_VARIABLE, marker)
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
    let child = command.spawn().unwrap();
    wait_for_note(marker, "uploading");
    child
}

fn notes(marker: &Path) -> String {
    std::fs::read_to_string(marker).unwrap_or_default()
}

fn wait_for_note(marker: &Path, what: &str) {
    let started = Instant::now();
    while !notes(marker).contains(what) {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "no {what:?} in {:?}",
            notes(marker)
                .lines()
                .filter(|line| *line != "uploading")
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn kill(child: &Child, signal: libc::c_int) {
    // SAFETY: a plain call. The process is still there, as nothing has waited for it.
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
}

/// How it ended. Anything that outlasts this has gone wrong, and it shouldn't hold up the run.
fn finish(mut child: Child) -> ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("still running");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[track_caller]
fn assert_cancelled_with_an_abort(marker: &Path) {
    let text = notes(marker);
    let ending: Vec<&str> = text
        .lines()
        .filter(|line| !["uploading", "starting"].contains(line))
        .collect();
    assert_eq!(ending, ["abort", "cancelled", "end"]);
    let _ = std::fs::remove_file(marker);
    let _ = std::fs::remove_file(marker.with_extension("gif"));
}

#[test]
fn ctrl_c_during_an_ack_wait_cancels_and_the_keyboard_gets_its_abort() {
    let marker = marker_for("ctrl-c");
    let upload = start_upload(&marker, false);
    kill(&upload, libc::SIGINT);
    assert!(finish(upload).success());
    assert_cancelled_with_an_abort(&marker);
}

#[test]
fn a_signal_during_the_starts_ack_wait_still_ends_with_an_abort() {
    let marker = marker_for("start");
    let mut command = Process::new(std::env::current_exe().unwrap());
    command
        .args(helper_args())
        .env(MARKER_VARIABLE, &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let upload = command.spawn().unwrap();
    wait_for_note(&marker, "starting");
    std::thread::sleep(Duration::from_millis(50));
    kill(&upload, libc::SIGTERM);
    assert!(finish(upload).success());
    assert_cancelled_with_an_abort(&marker);
}

#[test]
fn two_sighups_close_together_are_both_survived_and_the_upload_cancels() {
    let marker = marker_for("two-hups");
    let upload = start_upload(&marker, false);
    // As close together as a real one, and not so close that they'd count as one, as the same signal
    // twice before anything takes the first does.
    kill(&upload, libc::SIGHUP);
    std::thread::sleep(Duration::from_micros(300));
    kill(&upload, libc::SIGHUP);
    assert!(finish(upload).success());
    assert_cancelled_with_an_abort(&marker);
}

#[test]
fn sigterm_then_sighup_as_a_session_ending_may_send_is_survived_too() {
    let marker = marker_for("term-hup");
    let upload = start_upload(&marker, false);
    kill(&upload, libc::SIGTERM);
    kill(&upload, libc::SIGHUP);
    kill(&upload, libc::SIGTERM);
    assert!(finish(upload).success());
    assert_cancelled_with_an_abort(&marker);
}

#[test]
fn a_second_ctrl_c_ends_it_at_once() {
    let marker = marker_for("two-ctrl-c");
    let upload = start_upload(&marker, false);
    kill(&upload, libc::SIGINT);
    // Time to take the first, and not to get through the abort.
    std::thread::sleep(Duration::from_millis(30));
    kill(&upload, libc::SIGINT);
    let status = finish(upload);
    assert_eq!(status.signal(), Some(libc::SIGINT), "{status:?}");
    assert!(!notes(&marker).contains("end"), "{}", notes(&marker));
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(marker.with_extension("gif"));
}

#[test]
fn under_nohup_a_sighup_changes_nothing_and_sigterm_still_cancels() {
    let marker = marker_for("nohup");
    let upload = start_upload(&marker, true);
    kill(&upload, libc::SIGHUP);
    std::thread::sleep(Duration::from_millis(400));
    assert!(!notes(&marker).contains("abort"), "{}", notes(&marker));
    kill(&upload, libc::SIGTERM);
    assert!(finish(upload).success());
    assert_cancelled_with_an_abort(&marker);
}

/// A terminal of its own, whose far end closing is the real thing: the kernel hangs up on the shell
/// and what's running in it, and the shell passes that on.
fn open_terminal() -> (File, File) {
    // SAFETY: the name comes from the call and is copied at once, on the only thread that asks here,
    // and each descriptor is owned by one file from here on.
    unsafe {
        // Nothing else may keep this end open, or closing it here wouldn't close the terminal.
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        assert!(master >= 0);
        assert_eq!(libc::grantpt(master), 0);
        assert_eq!(libc::unlockpt(master), 0);
        let mut name = [0; 128];
        assert_eq!(
            libc::ptsname_r(master, name.as_mut_ptr(), name.len()),
            0,
            "no name"
        );
        let name = CStr::from_ptr(name.as_ptr());
        let slave = libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        );
        assert!(slave >= 0);
        (File::from_raw_fd(master), File::from_raw_fd(slave))
    }
}

/// Reads what the terminal shows, which a program printing to it would stop for otherwise.
fn drain(master: &mut File, until: impl Fn() -> bool) {
    let started = Instant::now();
    let mut buf = [0; 4096];
    while !until() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "never got there"
        );
        let mut fds = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: the structure and its count go together.
        if unsafe { libc::poll(&mut fds, 1, 20) } > 0 {
            let _ = master.read(&mut buf);
        }
    }
}

#[test]
fn closing_the_terminal_reaches_the_cancel_with_the_shell_passing_the_hangup_on() {
    if !Path::new("/bin/bash").exists() {
        eprintln!("no bash here, so nothing to pass the hangup on");
        return;
    }
    let marker = marker_for("pty");
    let (mut master, slave) = open_terminal();
    let mut shell = Process::new("/bin/bash");
    shell
        .args(["--norc", "--noprofile", "-i"])
        .env("PS1", "")
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave);
    // SAFETY: only calls that are safe between fork and exec.
    unsafe {
        shell.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            Ok(())
        });
    }
    let mut shell = {
        let spawned = shell.spawn().unwrap();
        drop(shell);
        spawned
    };
    let line = format!(
        "{MARKER_VARIABLE}='{}' '{}' {}\n",
        marker.display(),
        std::env::current_exe().unwrap().display(),
        helper_args().join(" ")
    );
    master.write_all(line.as_bytes()).unwrap();
    drain(&mut master, || notes(&marker).contains("uploading"));

    drop(master);
    wait_for_note(&marker, "end");
    let _ = shell.kill();
    let _ = shell.wait();
    assert_cancelled_with_an_abort(&marker);
}
