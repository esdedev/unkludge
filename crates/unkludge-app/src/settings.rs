//! The settings, in a text file of `key=value` lines. Whatever can't be read in it is skipped, so a
//! damaged file costs the settings at worst, never a crash.
//!
//! Whether the app starts with the system isn't here: the system's own entry says that, and the user
//! can also change it from outside the app.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use unkludge_protocol::lighting::Palettes;

const AUTO_CLOCK: &str = "auto_clock";
const PALETTES_PREFIX: &str = "palettes.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Set the clock on connecting and on coming back from sleep.
    pub auto_clock: bool,
    /// The last palettes sent to each model, by product ID, as a keyboard's can't be read back.
    pub palettes: BTreeMap<u16, Palettes>,
    /// Whether a file was there. If not, this is the first launch.
    pub existed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_clock: true,
            palettes: BTreeMap::new(),
            existed: false,
        }
    }
}

/// The settings file's name in the data folder.
const FILE_NAME: &str = "settings.txt";

/// A file in the data folder. A demo should keep its settings in a file of its own, using this with
/// [`Settings::load_from`] and [`Settings::save_to`].
pub fn path(file_name: &str) -> Option<PathBuf> {
    Some(crate::sys::data_dir()?.join(file_name))
}

impl Settings {
    /// The real settings. With no data folder there's nowhere to remember a first launch, so this
    /// doesn't count as one.
    pub fn load() -> Settings {
        match path(FILE_NAME) {
            Some(file) => Settings::load_from(&file),
            None => Settings {
                existed: true,
                ..Settings::default()
            },
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&path(FILE_NAME).ok_or(io::ErrorKind::NotFound)?)
    }

    /// Never fails. A file that's there but can't be read still counts as having existed, so that
    /// a damaged file doesn't look like a first launch and turn autostart on again.
    pub fn load_from(path: &Path) -> Settings {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Settings::default(),
            Err(_) => Vec::new(),
        };
        Settings {
            existed: true,
            ..Settings::from_text(&String::from_utf8_lossy(&bytes))
        }
    }

    fn from_text(text: &str) -> Settings {
        let mut settings = Settings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key == AUTO_CLOCK {
                match value {
                    "1" => settings.auto_clock = true,
                    "0" => settings.auto_clock = false,
                    _ => {}
                }
            } else if let Some(pid) = key.strip_prefix(PALETTES_PREFIX).and_then(parse_pid)
                && let Some(palettes) =
                    decode_hex(value).and_then(|bytes| Palettes::from_bytes(&bytes).ok())
            {
                settings.palettes.insert(pid, palettes);
            }
        }
        settings
    }

    fn to_text(&self) -> String {
        let mut text = format!("{AUTO_CLOCK}={}\n", u8::from(self.auto_clock));
        for (pid, palettes) in &self.palettes {
            let hex = encode_hex(palettes.as_bytes());
            text += &format!("{PALETTES_PREFIX}{pid:04X}={hex}\n");
        }
        text
    }

    /// Replaces the file in one go, so that a crash or a full disk leaves either the old file or
    /// the new one, whole.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let temp = temp_path(path);
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(self.to_text().as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

fn parse_pid(text: &str) -> Option<u16> {
    (text.len() == 4)
        .then(|| u16::from_str_radix(text, 16).ok())
        .flatten()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

/// Whether the first launch of this program should make it start with the system. A copy run from
/// a build folder isn't installed, so it shouldn't.
pub fn starts_with_system_on_first_launch(exe: &Path) -> bool {
    !exe.components()
        .any(|part| part.as_os_str().to_string_lossy().ends_with("target"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::lighting::{PALETTES_LEN, Rgb};
    use std::path::PathBuf;

    /// A folder of its own for each test, gone at the end.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir()
                .join(format!("unkludge-settings-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("settings.txt")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn palettes(color: Rgb) -> Palettes {
        let mut palettes = Palettes::default();
        palettes.set(3, 2, color).unwrap();
        palettes
    }

    #[test]
    fn the_defaults_turn_the_automatic_clock_on() {
        let settings = Settings::default();
        assert!(settings.auto_clock);
        assert!(settings.palettes.is_empty());
        assert!(!settings.existed);
    }

    #[test]
    fn with_no_file_it_is_the_first_launch() {
        let dir = TempDir::new("no-file");
        let settings = Settings::load_from(&dir.file());
        assert!(!settings.existed);
        assert!(settings.auto_clock);
    }

    #[test]
    fn what_is_saved_comes_back() {
        let dir = TempDir::new("round-trip");
        let mut settings = Settings {
            auto_clock: false,
            ..Settings::default()
        };
        settings
            .palettes
            .insert(0x0265, palettes(Rgb::new(1, 2, 3)));
        settings
            .palettes
            .insert(0x01A2, palettes(Rgb::new(0xFF, 0x80, 0)));
        settings.save_to(&dir.file()).unwrap();

        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.existed);
        assert!(!loaded.auto_clock);
        assert_eq!(loaded.palettes, settings.palettes);
        assert_eq!(loaded.palettes[&0x0265].get(3, 2), Some(Rgb::new(1, 2, 3)));
    }

    #[test]
    fn the_file_is_plain_key_value_lines() {
        let mut settings = Settings::default();
        settings.palettes.insert(0x0265, Palettes::default());
        let text = settings.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "auto_clock=1");
        assert!(lines[1].starts_with("palettes.0265=000000"), "{}", lines[1]);
        assert_eq!(lines[1].len(), "palettes.0265=".len() + 2 * PALETTES_LEN);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn a_corrupt_file_gives_the_defaults() {
        let dir = TempDir::new("corrupt");
        std::fs::write(dir.file(), [0xFF, 0xFE, 0x00, b'=', 0x80, b'\n', 0xC3]).unwrap();
        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.existed, "autostart must not be turned on again");
        assert!(loaded.auto_clock);
        assert!(loaded.palettes.is_empty());
    }

    #[test]
    fn a_folder_in_place_of_the_file_gives_the_defaults() {
        let dir = TempDir::new("folder");
        std::fs::create_dir(dir.file()).unwrap();
        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.existed);
        assert!(loaded.auto_clock);
    }

    #[test]
    fn broken_lines_are_skipped_and_good_ones_kept() {
        let good = "00".repeat(PALETTES_LEN);
        let text = format!(
            "auto_clock=maybe\n\
             no equals sign\n\
             =\n\
             future_setting=1\n\
             palettes.0265=abc\n\
             palettes.0266=zz{}\n\
             palettes.xyz={good}\n\
             palettes.00001={good}\n\
             palettes.01A2={good}\r\n",
            "00".repeat(PALETTES_LEN - 1),
        );
        let settings = Settings::from_text(&text);
        assert!(settings.auto_clock);
        let pids: Vec<u16> = settings.palettes.keys().copied().collect();
        assert_eq!(pids, [0x01A2]);
    }

    #[test]
    fn palettes_with_a_first_entry_the_protocol_refuses_are_dropped() {
        let mut bytes = "00".repeat(PALETTES_LEN);
        bytes.replace_range(0..2, "FF");
        let settings = Settings::from_text(&format!("palettes.0265={bytes}\n"));
        assert!(settings.palettes.is_empty());
    }

    #[test]
    fn non_ascii_in_a_hex_value_is_only_a_bad_value() {
        let settings = Settings::from_text("palettes.0265=é0\n");
        assert!(settings.palettes.is_empty());
    }

    #[test]
    fn saving_replaces_the_file_and_leaves_nothing_else() {
        let dir = TempDir::new("replace");
        std::fs::write(dir.file(), "auto_clock=1\nold=line\n").unwrap();
        let settings = Settings {
            auto_clock: false,
            ..Settings::default()
        };
        settings.save_to(&dir.file()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.file()).unwrap(),
            "auto_clock=0\n"
        );
        let names: Vec<_> = std::fs::read_dir(&dir.0).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn saving_makes_the_folder() {
        let dir = TempDir::new("makes-folder");
        let file = dir.0.join("deep").join("er").join("settings.txt");
        Settings::default().save_to(&file).unwrap();
        assert!(Settings::load_from(&file).existed);
    }

    #[test]
    fn a_save_that_fails_leaves_the_old_file_alone() {
        let dir = TempDir::new("fails");
        std::fs::write(dir.file(), "auto_clock=0\n").unwrap();
        // The temporary file can't be created, as a folder has its name.
        std::fs::create_dir(dir.0.join("settings.txt.tmp")).unwrap();
        assert!(Settings::default().save_to(&dir.file()).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.file()).unwrap(),
            "auto_clock=0\n"
        );
    }

    #[test]
    fn a_first_launch_starts_with_the_system_unless_from_a_build_folder() {
        for path in [
            "/opt/unkludge/unkludge-app",
            "C:/Program Files/Unkludge/unkludge-app.exe",
        ] {
            assert!(
                starts_with_system_on_first_launch(Path::new(path)),
                "{path}"
            );
        }
        for path in [
            "/home/a/unkludge/target/release/unkludge-app",
            "/home/a/kl-target/debug/unkludge-app",
        ] {
            assert!(
                !starts_with_system_on_first_launch(Path::new(path)),
                "{path}"
            );
        }
    }
}
