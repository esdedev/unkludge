//! Panics go to a file in the data folder, as the app has no console to show them on. Whatever was
//! handling them before still gets them, so they aren't lost if the file can't be written.

use std::backtrace::Backtrace;
use std::fmt::Display;
use std::path::PathBuf;

use crate::log::append_entry;

const FILE_NAME: &str = "crash.log";

/// Every panic from here on, on any thread, adds to the file. In release builds a panic then ends
/// the process.
pub fn install() {
    install_with(crate::sys::data_dir);
}

fn install_with(dir: impl Fn() -> Option<PathBuf> + Send + Sync + 'static) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = dir() {
            let entry = entry(info, &Backtrace::force_capture());
            let _ = append_entry(&dir, FILE_NAME, &entry);
        }
        previous(info);
    }));
}

fn entry(panic: &dyn Display, backtrace: &dyn Display) -> String {
    format!(
        "Unkludge {}\n{panic}\n\n{backtrace}",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::{Command, Output};

    const FOLDER_VARIABLE: &str = "UNKLUDGE_TEST_CRASH_FOLDER";
    const MESSAGE: &str = "a panic for the crash log's test";

    /// Panics for real. A panic hook is global, so this needs a process of its own, which the tests
    /// below start. With no folder given it does nothing.
    #[test]
    #[ignore = "started by the other tests here, in a process of its own"]
    fn a_process_that_panics() {
        if let Some(folder) = std::env::var_os(FOLDER_VARIABLE) {
            install_with(move || Some(PathBuf::from(&folder)));
            panic!("{MESSAGE}");
        }
    }

    fn panic_in_a_process(folder: &Path) -> Output {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash::tests::a_process_that_panics",
                "--ignored",
                "--nocapture",
            ])
            .env(FOLDER_VARIABLE, folder)
            .output()
            .unwrap()
    }

    fn temp_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("unkludge-crash-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn a_panic_goes_to_the_file_with_the_version_and_on_to_the_previous_hook() {
        let folder = temp_folder("written");
        for _ in 0..2 {
            let output = panic_in_a_process(&folder);
            assert!(!output.status.success());
            let printed = String::from_utf8_lossy(&output.stderr);
            assert!(printed.contains(MESSAGE), "{printed}");
        }
        let log = std::fs::read_to_string(folder.join("crash.log"))
            .unwrap()
            .replace("\r\n", "\n");
        let version = format!("Unkludge {}\n", env!("CARGO_PKG_VERSION"));
        assert_eq!(log.matches(&version).count(), 2, "{log}");
        assert_eq!(log.matches(MESSAGE).count(), 2, "{log}");
        assert_eq!(log.matches("\n----").count(), 2, "{log}");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_panic_that_cant_be_written_still_reaches_the_previous_hook() {
        let folder = temp_folder("unwritable");
        // A file where the folder should be.
        std::fs::write(&folder, "").unwrap();
        let output = panic_in_a_process(&folder);
        assert!(!output.status.success());
        let printed = String::from_utf8_lossy(&output.stderr);
        assert!(printed.contains(MESSAGE), "{printed}");
        std::fs::remove_file(&folder).unwrap();
    }

    #[test]
    fn an_entry_has_the_version_the_panic_and_the_backtrace() {
        assert_eq!(
            entry(&"the panic", &"the backtrace"),
            format!(
                "Unkludge {}\nthe panic\n\nthe backtrace",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}
