//! Errors with nowhere else to go: to a console if there is one, and to a file in the data folder,
//! as on Windows the app has no console. Nothing personal goes in, so no paths.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

pub fn error(text: &str) {
    eprintln!("unkludge: {text}");
    // Tests don't touch the real data folder.
    if !cfg!(test)
        && let Some(dir) = crate::sys::data_dir()
    {
        let _ = append_entry(&dir, "unkludge.log", text);
    }
}

/// Adds to a file in `dir`, making them if needed.
pub fn append_entry(dir: &Path, file_name: &str, text: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file_name))?;
    writeln!(file, "{text}\n----")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_add_up_in_a_file_in_a_folder_made_for_it() {
        let dir = std::env::temp_dir().join(format!("unkludge-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("deeper");
        append_entry(&folder, "some.log", "first").unwrap();
        append_entry(&folder, "some.log", "second").unwrap();
        let text = std::fs::read_to_string(folder.join("some.log")).unwrap();
        assert_eq!(text.replace("\r\n", "\n"), "first\n----\nsecond\n----\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
