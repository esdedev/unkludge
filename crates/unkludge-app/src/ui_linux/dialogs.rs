//! Choosing a file. Slint has no dialog for it, and pulling in a toolkit for one isn't worth it when
//! desktops come with a program that does it: the window asks for the path itself when there's none.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::i18n::t;

/// The files the screen can show.
const PATTERNS: &str = "*.gif *.png *.jpg *.jpeg *.bmp *.webp";

/// How asking for a file went.
#[derive(Debug, PartialEq, Eq)]
pub enum Picked {
    File(PathBuf),
    /// The user changed their mind.
    Cancelled,
    /// No program for it is installed, so the window should ask itself.
    NoProgram,
}

/// What both programs exit with when the user closes them.
const EXIT_CANCELLED: i32 = 1;

/// What a program for choosing a file said, if it could be run.
pub struct Answer {
    /// Its exit code, if it wasn't killed.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
}

/// The programs to try, in order, each with its arguments.
fn programs(title: &str, kind: &str) -> [(&'static str, Vec<OsString>); 2] {
    let os = |text: String| OsString::from(text);
    [
        (
            "zenity",
            vec![
                os("--file-selection".to_string()),
                os(format!("--title={title}")),
                os(format!("--file-filter={kind} | {PATTERNS}")),
            ],
        ),
        (
            "kdialog",
            vec![
                os("--title".to_string()),
                os(title.to_string()),
                os("--getopenfilename".to_string()),
                os(".".to_string()),
                os(format!("{PATTERNS}|{kind}")),
            ],
        ),
    ]
}

/// The path a program printed. A path can have anything in it but a line break at its end.
fn path_in(stdout: &[u8]) -> Option<PathBuf> {
    let line = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    (!line.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(line)))
}

/// Tries each program in turn with `run`, and the first that has an answer gets to say. One that
/// can't run or that fails, as with no display it can use, leaves it to the next.
fn pick_with(
    title: &str,
    kind: &str,
    mut run: impl FnMut(&str, &[OsString]) -> io::Result<Answer>,
) -> Picked {
    for (program, args) in programs(title, kind) {
        let Ok(answer) = run(program, &args) else {
            continue;
        };
        match answer.code {
            Some(0) => {
                return path_in(&answer.stdout).map_or(Picked::Cancelled, Picked::File);
            }
            Some(EXIT_CANCELLED) => return Picked::Cancelled,
            _ => {}
        }
    }
    Picked::NoProgram
}

/// Asks for files, and can stop asking from another thread.
#[derive(Clone, Default)]
pub struct Picker {
    /// The process asking right now.
    running: Arc<AtomicU32>,
    stopped: Arc<AtomicBool>,
}

impl Picker {
    /// Asks for a file. Waits for the user, so don't call this from the interface's thread.
    pub fn choose_image(&self) -> Picked {
        pick_with(t().choose_title, t().image_files, |program, args| {
            self.run(program, args)
        })
    }

    fn run(&self, program: &str, args: &[OsString]) -> io::Result<Answer> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        self.running.store(child.id(), Ordering::SeqCst);
        let output = child.wait_with_output();
        self.running.store(0, Ordering::SeqCst);
        let output = output?;
        Ok(Answer {
            code: output.status.code(),
            stdout: output.stdout,
        })
    }

    /// Closes what's asking, if anything is.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let running = self.running.swap(0, Ordering::SeqCst);
        if running != 0 {
            // SAFETY: plain numbers. It's cleared as soon as the process has been waited for, so
            // the number is still that process's.
            unsafe {
                libc::kill(running as libc::pid_t, libc::SIGTERM);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(code: i32, stdout: &[u8]) -> io::Result<Answer> {
        Ok(Answer {
            code: Some(code),
            stdout: stdout.to_vec(),
        })
    }

    #[test]
    fn the_first_program_that_runs_gets_to_say() {
        let mut tried = Vec::new();
        let picked = pick_with("T", "K", |program, _| {
            tried.push(program.to_string());
            answer(0, b"/a/b.gif\n")
        });
        assert_eq!(picked, Picked::File(PathBuf::from("/a/b.gif")));
        assert_eq!(tried, ["zenity"]);
    }

    #[test]
    fn a_program_that_isnt_there_is_passed_over() {
        let mut tried = Vec::new();
        let picked = pick_with("T", "K", |program, _| {
            tried.push(program.to_string());
            if program == "zenity" {
                Err(io::ErrorKind::NotFound.into())
            } else {
                answer(0, b"/c.png\n")
            }
        });
        assert_eq!(picked, Picked::File(PathBuf::from("/c.png")));
        assert_eq!(tried, ["zenity", "kdialog"]);
    }

    #[test]
    fn with_no_program_the_window_has_to_ask() {
        let picked = pick_with("T", "K", |_, _| Err(io::ErrorKind::NotFound.into()));
        assert_eq!(picked, Picked::NoProgram);
    }

    #[test]
    fn closing_the_dialog_is_changing_ones_mind_and_doesnt_try_another_program() {
        let mut tries = 0;
        let picked = pick_with("T", "K", |_, _| {
            tries += 1;
            answer(1, b"")
        });
        assert_eq!(picked, Picked::Cancelled);
        assert_eq!(tries, 1);
    }

    #[test]
    fn a_program_that_fails_leaves_it_to_the_next() {
        let mut tried = Vec::new();
        let picked = pick_with("T", "K", |program, _| {
            tried.push(program.to_string());
            if program == "zenity" {
                answer(255, b"")
            } else {
                answer(0, b"/c.png\n")
            }
        });
        assert_eq!(picked, Picked::File(PathBuf::from("/c.png")));
        assert_eq!(tried, ["zenity", "kdialog"]);
    }

    #[test]
    fn with_every_program_failing_the_window_has_to_ask() {
        assert_eq!(
            pick_with("T", "K", |_, _| answer(5, b"")),
            Picked::NoProgram
        );
    }

    #[test]
    fn a_program_that_was_killed_is_no_answer() {
        let picked = pick_with("T", "K", |_, _| {
            Ok(Answer {
                code: None,
                stdout: Vec::new(),
            })
        });
        assert_eq!(picked, Picked::NoProgram);
    }

    #[test]
    fn saying_yes_with_nothing_chosen_is_changing_ones_mind() {
        assert_eq!(
            pick_with("T", "K", |_, _| answer(0, b"\n")),
            Picked::Cancelled
        );
    }

    #[test]
    fn stopping_closes_what_is_asking() {
        let picker = Picker::default();
        let asking = picker.clone();
        let started = std::time::Instant::now();
        let running = std::thread::spawn(move || asking.run("sleep", &[OsString::from("30")]));
        while picker.running.load(Ordering::SeqCst) == 0 {
            assert!(started.elapsed().as_secs() < 10, "it never started");
            std::thread::yield_now();
        }
        picker.stop();
        let answer = running.join().unwrap().unwrap();
        assert_eq!(answer.code, None, "killed");
        assert!(started.elapsed().as_secs() < 10);
    }

    #[test]
    fn once_stopped_nothing_else_gets_started() {
        let picker = Picker::default();
        picker.stop();
        assert!(picker.run("true", &[]).is_err());
    }

    #[test]
    fn a_path_can_have_spaces_and_bytes_that_arent_text() {
        assert_eq!(
            path_in(b"/home/ana/Mis fotos/\xFFgato.gif\n"),
            Some(PathBuf::from(OsStr::from_bytes(
                b"/home/ana/Mis fotos/\xFFgato.gif"
            )))
        );
        assert_eq!(path_in(b"\n"), None);
        assert_eq!(path_in(b""), None);
    }

    #[test]
    fn each_program_is_told_what_to_look_for() {
        let [(zenity, zenity_args), (kdialog, kdialog_args)] = programs("Elige", "Imágenes");
        assert_eq!(zenity, "zenity");
        assert!(zenity_args.contains(&OsString::from(format!(
            "--file-filter=Imágenes | {PATTERNS}"
        ))));
        assert!(zenity_args.contains(&OsString::from("--title=Elige")));
        assert_eq!(kdialog, "kdialog");
        assert!(kdialog_args.contains(&OsString::from(format!("{PATTERNS}|Imágenes"))));
    }
}
