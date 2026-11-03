//! Panics go to a file in the data folder, as the app has no console to show them on.

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

/// Every panic from here on, on any thread, adds to `crash.log`. In release builds a panic then ends
/// the process.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(dir) = crate::sys::data_dir() {
            let backtrace = Backtrace::force_capture();
            let _ = append(&dir, &format!("{info}\n\n{backtrace}"));
        }
    }));
}

fn append(dir: &Path, text: &str) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("crash.log"))?;
    writeln!(file, "{text}\n----")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crashes_add_up_in_a_file_in_the_data_folder() {
        let dir = std::env::temp_dir().join(format!("unkludge-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        append(&dir, "first").unwrap();
        append(&dir, "second").unwrap();
        let text = std::fs::read_to_string(dir.join("crash.log")).unwrap();
        assert_eq!(text.replace("\r\n", "\n"), "first\n----\nsecond\n----\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
