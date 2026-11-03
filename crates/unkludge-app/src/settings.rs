//! The settings, in a text file of `key=value` lines. Whatever can't be read in it is skipped, so a
//! damaged file costs the settings at worst, never a crash.
//!
//! Whether the app starts with the system isn't here: the system's own entry says that, and the user
//! can also change it from outside the app.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use unkludge_protocol::lighting::Palettes;

const AUTO_CLOCK: &str = "auto_clock";
const TRAY_NOTE_SHOWN: &str = "tray_note_shown";
const PALETTES_PREFIX: &str = "palettes.";
/// Far more than these settings can ever take, for all the models there are. A larger file isn't one
/// of ours, and gets treated like any other damaged one.
const MAX_FILE_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Set the clock on connecting and on coming back from sleep.
    pub auto_clock: bool,
    /// The first time the window is closed, a note says that the app keeps running in the tray.
    pub tray_note_shown: bool,
    /// The last palettes sent to each model, by product ID, as a keyboard's can't be read back.
    pub palettes: BTreeMap<u16, Palettes>,
    /// Whether a file was there. If not, this is the first launch.
    pub existed: bool,
    /// Why the file that's there couldn't be read. It may be fine, with these settings being only
    /// the defaults, so while this is set saving refuses rather than replace it.
    pub read_error: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto_clock: true,
            tray_note_shown: false,
            palettes: BTreeMap::new(),
            existed: false,
            read_error: None,
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
    /// The real settings.
    pub fn load() -> Settings {
        Settings::load_at(path(FILE_NAME).as_deref())
    }

    /// With no data folder there's nowhere to remember a first launch, so this doesn't count as one.
    fn load_at(file: Option<&Path>) -> Settings {
        match file {
            Some(file) => Settings::load_from(file),
            None => Settings {
                existed: true,
                ..Settings::default()
            },
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&path(FILE_NAME).ok_or(io::ErrorKind::NotFound)?)
    }

    /// Never fails. A damaged file gives the defaults and gets replaced at the next save. One that
    /// can't be read, or a folder that can't be made for it, gives the defaults and keeps the reason
    /// in [`Settings::read_error`]. All of those count as having existed, so that they don't look
    /// like a first launch and turn autostart on again.
    pub fn load_from(path: &Path) -> Settings {
        let bytes = match read_limited(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return match path.parent().map(std::fs::create_dir_all) {
                    Some(Err(error)) => Settings::unreadable(&error),
                    _ => Settings::default(),
                };
            }
            Err(error) => return Settings::unreadable(&error),
        };
        Settings {
            existed: true,
            ..Settings::from_text(&String::from_utf8_lossy(&bytes))
        }
    }

    fn unreadable(error: &io::Error) -> Settings {
        crate::log::error(&format!("the settings can't be read: {error}"));
        Settings {
            existed: true,
            read_error: Some(error.to_string()),
            ..Settings::default()
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
            } else if key == TRAY_NOTE_SHOWN {
                settings.tray_note_shown = value == "1";
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
        if self.tray_note_shown {
            text += &format!("{TRAY_NOTE_SHOWN}=1\n");
        }
        for (pid, palettes) in &self.palettes {
            let hex = encode_hex(palettes.as_bytes());
            text += &format!("{PALETTES_PREFIX}{pid:04X}={hex}\n");
        }
        text
    }

    /// Replaces the file in one go, so that a crash or a full disk leaves either the old file or
    /// the new one, whole. Refuses if the old file couldn't be read when loading.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(why) = &self.read_error {
            return Err(io::Error::other(format!(
                "the settings file couldn't be read, so it stays as it is: {why}"
            )));
        }
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let temp = temp_path(path);
        let saved = self
            .write_file(&temp)
            .and_then(|()| std::fs::rename(&temp, path));
        if saved.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        saved
    }

    fn write_file(&self, path: &Path) -> io::Result<()> {
        let mut file = File::create(path)?;
        file.write_all(self.to_text().as_bytes())?;
        file.sync_all()
    }
}

/// The file's contents, or nothing at all if it's past the limit.
fn read_limited(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bytes.clear();
    }
    Ok(bytes)
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

fn all_hex(text: &str) -> bool {
    text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_pid(text: &str) -> Option<u16> {
    (text.len() == 4 && all_hex(text))
        .then(|| u16::from_str_radix(text, 16).ok())
        .flatten()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !all_hex(text) || !text.len().is_multiple_of(2) {
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
    !exe.components().any(|part| part.as_os_str() == "target")
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
    fn the_tray_note_is_remembered_once_shown() {
        let dir = TempDir::new("tray-note");
        Settings::default().save_to(&dir.file()).unwrap();
        assert!(!Settings::load_from(&dir.file()).tray_note_shown);
        let shown = Settings {
            tray_note_shown: true,
            ..Settings::default()
        };
        shown.save_to(&dir.file()).unwrap();
        assert!(Settings::load_from(&dir.file()).tray_note_shown);
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
        assert!(loaded.read_error.is_some());
    }

    #[test]
    fn a_file_that_cant_be_read_isnt_replaced_by_a_later_save() {
        let dir = TempDir::new("no-overwrite");
        // A folder stands in for a file with no read permission, which tests can't make everywhere.
        std::fs::create_dir(dir.file()).unwrap();
        std::fs::write(dir.file().join("what-was-there"), "kept").unwrap();
        let loaded = Settings::load_from(&dir.file());
        let error = loaded.save_to(&dir.file()).unwrap_err();
        assert!(error.to_string().contains("stays as it is"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.file().join("what-was-there")).unwrap(),
            "kept"
        );
        // It doesn't leave a temporary file, either.
        assert!(!dir.0.join("settings.txt.tmp").exists());
    }

    #[test]
    fn a_refusal_to_save_applies_to_any_path() {
        let dir = TempDir::new("refuses");
        let settings = Settings {
            read_error: Some("no access".to_string()),
            ..Settings::default()
        };
        let elsewhere = dir.0.join("other.txt");
        assert!(settings.save_to(&elsewhere).is_err());
        assert!(!elsewhere.exists());
    }

    #[test]
    fn a_folder_that_cant_be_made_isnt_a_first_launch() {
        let dir = TempDir::new("no-folder");
        let not_a_folder = dir.0.join("file");
        std::fs::write(&not_a_folder, "").unwrap();
        let loaded = Settings::load_from(&not_a_folder.join("settings.txt"));
        assert!(loaded.existed);
        assert!(loaded.read_error.is_some());
    }

    /// Here the file is simply missing, and it's making the folder that fails. Elsewhere the read
    /// itself says so, as the test above has it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_that_the_system_wont_make_isnt_a_first_launch() {
        let loaded = Settings::load_from(Path::new("/proc/unkludge-test/settings.txt"));
        assert!(loaded.existed);
        assert!(loaded.read_error.is_some());
    }

    #[test]
    fn with_no_data_folder_it_isnt_a_first_launch() {
        let loaded = Settings::load_at(None);
        assert!(loaded.existed);
        assert!(loaded.auto_clock);
        assert!(loaded.read_error.is_none());
    }

    #[test]
    fn a_path_is_loaded_from_when_there_is_one() {
        let dir = TempDir::new("load-at");
        std::fs::write(dir.file(), "auto_clock=0\n").unwrap();
        assert!(!Settings::load_at(Some(&dir.file())).auto_clock);
    }

    #[test]
    fn a_file_past_the_limit_is_damaged_and_gets_replaced() {
        let dir = TempDir::new("too-big");
        let mut text = String::from("auto_clock=0\n");
        text.push_str(&"#".repeat(MAX_FILE_BYTES as usize));
        std::fs::write(dir.file(), text).unwrap();
        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.existed);
        assert!(loaded.auto_clock, "none of it counts");
        assert!(loaded.read_error.is_none());
        loaded.save_to(&dir.file()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.file()).unwrap(),
            "auto_clock=1\n"
        );
    }

    #[test]
    fn a_file_right_at_the_limit_is_read() {
        let dir = TempDir::new("at-limit");
        let mut text = String::from("auto_clock=0\n");
        text.push_str(&"#".repeat(MAX_FILE_BYTES as usize - text.len()));
        std::fs::write(dir.file(), text).unwrap();
        assert!(!Settings::load_from(&dir.file()).auto_clock);
    }

    #[test]
    fn a_failed_save_leaves_no_temporary_file() {
        let dir = TempDir::new("no-temp");
        // Writing the temporary file works, and renaming it over a folder doesn't.
        std::fs::create_dir(dir.file()).unwrap();
        assert!(Settings::default().save_to(&dir.file()).is_err());
        assert!(!dir.0.join("settings.txt.tmp").exists());
    }

    #[test]
    fn a_plus_isnt_a_hex_digit() {
        let good = "00".repeat(PALETTES_LEN);
        let settings = Settings::from_text(&format!("palettes.+265={good}\n"));
        assert!(settings.palettes.is_empty());
        let plus_in_value = format!("+0{}", "00".repeat(PALETTES_LEN - 1));
        let settings = Settings::from_text(&format!("palettes.0265={plus_in_value}\n"));
        assert!(settings.palettes.is_empty());
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
        // Four bytes, the second pair of them starting partway through the `é`.
        let settings = Settings::from_text("palettes.0265=aé0\n");
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
            "/home/a/kl-target/debug/unkludge-app",
            "/home/a/my-target-practice/unkludge-app",
        ] {
            assert!(
                starts_with_system_on_first_launch(Path::new(path)),
                "{path}"
            );
        }
        for path in [
            "/home/a/unkludge/target/release/unkludge-app",
            "C:/unkludge/target/debug/unkludge-app.exe",
        ] {
            assert!(
                !starts_with_system_on_first_launch(Path::new(path)),
                "{path}"
            );
        }
    }
}
