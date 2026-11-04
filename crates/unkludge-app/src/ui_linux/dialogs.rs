//! Choosing a file. Slint has no dialog for it, and pulling in a toolkit for one isn't worth it when
//! desktops come with a program that does it: the window asks for the path itself when there's none.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

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

/// What a program for choosing a file said, if it could be run.
pub struct Answer {
    pub success: bool,
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

/// Tries each program in turn with `run`, and the first that runs gets to say.
fn pick_with(
    title: &str,
    kind: &str,
    mut run: impl FnMut(&str, &[OsString]) -> io::Result<Answer>,
) -> Picked {
    for (program, args) in programs(title, kind) {
        let Ok(answer) = run(program, &args) else {
            continue;
        };
        return match path_in(&answer.stdout).filter(|_| answer.success) {
            Some(path) => Picked::File(path),
            None => Picked::Cancelled,
        };
    }
    Picked::NoProgram
}

/// Asks for a file. Waits for the user, so don't call this from the interface's thread.
pub fn choose_image() -> Picked {
    pick_with(t().choose_title, t().image_files, |program, args| {
        Command::new(program)
            .args(args)
            .output()
            .map(|output| Answer {
                success: output.status.success(),
                stdout: output.stdout,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(success: bool, stdout: &[u8]) -> io::Result<Answer> {
        Ok(Answer {
            success,
            stdout: stdout.to_vec(),
        })
    }

    #[test]
    fn the_first_program_that_runs_gets_to_say() {
        let mut tried = Vec::new();
        let picked = pick_with("T", "K", |program, _| {
            tried.push(program.to_string());
            answer(true, b"/a/b.gif\n")
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
                answer(true, b"/c.png\n")
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
            answer(false, b"")
        });
        assert_eq!(picked, Picked::Cancelled);
        assert_eq!(tries, 1);
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
