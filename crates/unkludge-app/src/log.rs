//! Errors with nowhere else to go: to a console if there is one, and to a file in the data folder,
//! as on Windows the app has no console. Nothing personal goes in, so no paths.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// How big a file gets before it starts over. A problem that keeps coming can't fill the disk, and
/// this is far more than anyone needs to read.
const MAX_FILE_BYTES: u64 = 256 * 1024;

pub fn error(text: &str) {
    eprintln!("unkludge: {text}");
    // Tests don't touch the real data folder.
    if !cfg!(test)
        && let Some(dir) = crate::sys::data_dir()
    {
        let _ = append_entry(&dir, "unkludge.log", text);
    }
}

/// For a problem that can come up over and over, as on every paint: it's told when it comes, and
/// again only if it changes or goes away first.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
pub struct Repeating {
    last: Option<String>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Repeating {
    pub fn error(&mut self, text: &str) {
        if self.is_news(text) {
            error(text);
        }
    }

    /// It went away.
    pub fn clear(&mut self) {
        self.last = None;
    }

    fn is_news(&mut self, text: &str) -> bool {
        if self.last.as_deref() == Some(text) {
            return false;
        }
        self.last = Some(text.to_string());
        true
    }
}

/// Adds to a file in `dir`, making them if needed. A file that has grown past the limit becomes
/// `<name>.old`, in place of the one before, and this one starts over.
pub fn append_entry(dir: &Path, file_name: &str, text: &str) -> io::Result<()> {
    append_limited(dir, file_name, text, MAX_FILE_BYTES)
}

fn append_limited(dir: &Path, file_name: &str, text: &str, limit: u64) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name);
    if std::fs::metadata(&path).is_ok_and(|about| about.len() >= limit) {
        // If this fails the entry still goes in, and the next one tries again.
        let _ = std::fs::rename(&path, old_path(&path));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{text}\n----")
}

fn old_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("unkludge-log-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn entries_add_up_in_a_file_in_a_folder_made_for_it() {
        let dir = temp_dir("adds-up");
        let folder = dir.join("deeper");
        append_entry(&folder, "some.log", "first").unwrap();
        append_entry(&folder, "some.log", "second").unwrap();
        let text = std::fs::read_to_string(folder.join("some.log")).unwrap();
        assert_eq!(text.replace("\r\n", "\n"), "first\n----\nsecond\n----\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_file_starts_over_keeping_one_old_one() {
        let dir = temp_dir("rotates");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .unwrap()
                .replace("\r\n", "\n")
        };
        append_limited(&dir, "some.log", "first", 8).unwrap();
        // The file is at the limit now, so the next entry starts a new one.
        append_limited(&dir, "some.log", "second", 8).unwrap();
        assert_eq!(read("some.log"), "second\n----\n");
        assert_eq!(read("some.log.old"), "first\n----\n");
        // And replaces the old one the next time.
        append_limited(&dir, "some.log", "third", 8).unwrap();
        assert_eq!(read("some.log"), "third\n----\n");
        assert_eq!(read("some.log.old"), "second\n----\n");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nothing_starts_over_before_the_limit() {
        let dir = temp_dir("under-limit");
        append_limited(&dir, "some.log", "first", 1000).unwrap();
        append_limited(&dir, "some.log", "second", 1000).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_real_limit_is_small_next_to_a_disk_and_large_next_to_an_entry() {
        assert_eq!(MAX_FILE_BYTES, 256 * 1024);
    }

    #[test]
    fn a_problem_that_keeps_coming_is_told_once() {
        let mut repeating = Repeating::default();
        assert!(repeating.is_news("it broke"));
        assert!(!repeating.is_news("it broke"));
        assert!(!repeating.is_news("it broke"));
        assert!(repeating.is_news("it broke differently"));
        assert!(repeating.is_news("it broke"));
        repeating.clear();
        assert!(repeating.is_news("it broke"));
    }
}
