//! The settings, in a text file of `key=value` lines. Whatever can't be read in it is skipped, so a
//! damaged file costs the settings at worst, never a crash. The app writes UTF-8, and takes what a
//! text editor may save instead: UTF-8 with a BOM and UTF-16 with one.
//!
//! What matters most here is the automatic clock, as it writes to the keyboard unasked. Someone who
//! turned it off in this file mustn't get it back: a value this doesn't know means off, and so does a
//! file that's there and can't be made sense of, which also stays as it is.
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
/// of ours, so it counts as unreadable.
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

/// The real settings file's name in the data folder.
pub const FILE_NAME: &str = "settings.txt";

impl Settings {
    /// With no data folder there's nowhere to remember a first launch, so this doesn't count as one.
    pub fn load_at(file: Option<&Path>) -> Settings {
        match file {
            Some(file) => Settings::load_from(file),
            None => Settings {
                existed: true,
                ..Settings::default()
            },
        }
    }

    /// Never fails. A file with a line that's damaged loses that line. One that can't be read, or has
    /// nothing in it this knows, or a folder that can't be made for it, gives the defaults with the
    /// automatic clock off, and keeps the reason in [`Settings::read_error`]. All of those count as
    /// having existed, so that they don't look like a first launch and turn autostart on again.
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
        Settings::from_text(&decode(&bytes))
    }

    fn unreadable(error: &io::Error) -> Settings {
        crate::log::error(&format!("the settings can't be read: {error}"));
        Settings {
            auto_clock: false,
            existed: true,
            read_error: Some(error.to_string()),
            ..Settings::default()
        }
    }

    /// Nothing written in the text, or at least one setting from it. Anything else isn't a file of
    /// these settings, whatever it is, and it might be one this just can't read.
    fn from_text(text: &str) -> Settings {
        let mut settings = Settings {
            existed: true,
            ..Settings::default()
        };
        let mut known = false;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key == AUTO_CLOCK {
                known = true;
                settings.auto_clock = auto_clock_value(value);
            } else if key == TRAY_NOTE_SHOWN {
                known = true;
                settings.tray_note_shown = value == "1";
            } else if let Some(pid) = key.strip_prefix(PALETTES_PREFIX) {
                known = true;
                if let Some(pid) = parse_pid(pid)
                    && let Some(palettes) =
                        decode_hex(value).and_then(|bytes| Palettes::from_bytes(&bytes).ok())
                {
                    settings.palettes.insert(pid, palettes);
                }
            }
        }
        if !known && !text.trim().is_empty() {
            return Settings::unreadable(&io::Error::other("it has no setting this knows"));
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

/// The file's contents. One past the limit is an error.
fn read_limited(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::other("it's far too large to be this file"));
    }
    Ok(bytes)
}

/// Text, going by the BOM if there is one, and as UTF-8 if not. Bytes that make no sense in those
/// come out as replacement characters, and only cost the line they're on.
fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        other => String::from_utf8_lossy(other).into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let (units, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = units.iter().map(|&pair| unit(pair)).collect();
    String::from_utf16_lossy(&units)
}

/// Only `1` turns the automatic clock on, as it writes to the keyboard unasked.
fn auto_clock_value(value: &str) -> bool {
    match value {
        "1" => true,
        "0" => false,
        other => {
            crate::log::error(&format!(
                "{AUTO_CLOCK} has the value {other:?}, which isn't 0 or 1, so it counts as off"
            ));
            false
        }
    }
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
/// a build folder isn't installed, and neither is a debug build wherever it's been put.
pub fn starts_with_system_on_first_launch(exe: &Path) -> bool {
    !cfg!(debug_assertions) && looks_installed(exe)
}

fn looks_installed(exe: &Path) -> bool {
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

    fn utf16(text: &str, bom: [u8; 2], unit: fn(u16) -> [u8; 2]) -> Vec<u8> {
        let mut bytes = bom.to_vec();
        bytes.extend(text.encode_utf16().flat_map(unit));
        bytes
    }

    /// What's on disk after loading a file like this and quitting.
    fn after_a_save(dir: &TempDir, bytes: &[u8]) -> (Settings, String) {
        std::fs::write(dir.file(), bytes).unwrap();
        let loaded = Settings::load_from(&dir.file());
        loaded.save_to(&dir.file()).unwrap();
        let text = std::fs::read_to_string(dir.file()).unwrap();
        (loaded, text)
    }

    #[test]
    fn turning_the_automatic_clock_off_survives_what_an_editor_may_save() {
        let dir = TempDir::new("opt-out");
        let line = "auto_clock=0\r\n";
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend(line.as_bytes());
        for bytes in [
            line.as_bytes().to_vec(),
            bom,
            utf16(line, [0xFF, 0xFE], u16::to_le_bytes),
            utf16(line, [0xFE, 0xFF], u16::to_be_bytes),
        ] {
            let (loaded, text) = after_a_save(&dir, &bytes);
            assert!(!loaded.auto_clock, "{bytes:02X?}");
            assert!(loaded.read_error.is_none(), "{bytes:02X?}");
            assert_eq!(text, "auto_clock=0\n", "{bytes:02X?}");
        }
    }

    #[test]
    fn a_value_this_doesnt_know_turns_the_automatic_clock_off() {
        let dir = TempDir::new("off-word");
        for value in ["off", "no", "false", "maybe", "", "2", "01"] {
            let (loaded, text) = after_a_save(&dir, format!("auto_clock={value}\n").as_bytes());
            assert!(!loaded.auto_clock, "{value:?}");
            assert_eq!(text, "auto_clock=0\n", "{value:?}");
        }
    }

    #[test]
    fn a_later_line_about_the_clock_replaces_an_earlier_one() {
        assert!(Settings::from_text("auto_clock=0\nauto_clock=1\n").auto_clock);
        assert!(!Settings::from_text("auto_clock=1\nauto_clock=off\n").auto_clock);
    }

    #[test]
    fn utf16_settings_keep_everything_else_too() {
        let good = "00".repeat(PALETTES_LEN);
        let text = format!("auto_clock=1\ntray_note_shown=1\npalettes.0265={good}\n");
        let dir = TempDir::new("utf16-all");
        std::fs::write(dir.file(), utf16(&text, [0xFF, 0xFE], u16::to_le_bytes)).unwrap();
        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.auto_clock);
        assert!(loaded.tray_note_shown);
        assert!(loaded.palettes.contains_key(&0x0265));
    }

    #[test]
    fn half_a_character_at_the_end_of_utf16_costs_nothing_else() {
        let mut bytes = utf16("auto_clock=0\n", [0xFF, 0xFE], u16::to_le_bytes);
        bytes.push(0x41);
        assert!(!Settings::from_text(&decode(&bytes)).auto_clock);
    }

    #[test]
    fn a_file_with_nothing_known_in_it_is_unreadable_and_stays_as_it_is() {
        let dir = TempDir::new("unknown");
        let cases: [&[u8]; 5] = [
            // Binary rubbish, starting with what looks like a BOM.
            &[0xFF, 0xFE, 0x00, b'=', 0x80, b'\n', 0xC3],
            b"no equals sign",
            b"future_setting=1\n",
            // UTF-16 with no BOM, as nothing says what it is.
            b"a\0u\0t\0o\0_\0c\0l\0o\0c\0k\0=\x000\0",
            b"AUTO_CLOCK=0\n",
        ];
        for bytes in cases {
            std::fs::write(dir.file(), bytes).unwrap();
            let loaded = Settings::load_from(&dir.file());
            assert!(loaded.existed, "autostart must not be turned on again");
            assert!(loaded.read_error.is_some(), "{bytes:02X?}");
            assert!(!loaded.auto_clock, "{bytes:02X?}");
            assert!(loaded.save_to(&dir.file()).is_err(), "{bytes:02X?}");
            assert_eq!(std::fs::read(dir.file()).unwrap(), bytes);
        }
    }

    #[test]
    fn an_empty_file_is_just_the_defaults() {
        let dir = TempDir::new("empty");
        for bytes in [&b""[..], b" \r\n\n", &[0xEF, 0xBB, 0xBF], &[0xFF, 0xFE]] {
            std::fs::write(dir.file(), bytes).unwrap();
            let loaded = Settings::load_from(&dir.file());
            assert!(loaded.existed, "{bytes:02X?}");
            assert!(loaded.read_error.is_none(), "{bytes:02X?}");
            assert!(loaded.auto_clock, "{bytes:02X?}");
        }
    }

    #[test]
    fn a_folder_in_place_of_the_file_is_unreadable() {
        let dir = TempDir::new("folder");
        std::fs::create_dir(dir.file()).unwrap();
        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.existed);
        assert!(!loaded.auto_clock);
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
    fn a_file_past_the_limit_is_unreadable_and_stays_as_it_is() {
        let dir = TempDir::new("too-big");
        let mut text = String::from("auto_clock=0\n");
        text.push_str(&"#".repeat(MAX_FILE_BYTES as usize));
        std::fs::write(dir.file(), &text).unwrap();
        let loaded = Settings::load_from(&dir.file());
        assert!(loaded.existed);
        assert!(!loaded.auto_clock);
        assert!(loaded.read_error.is_some());
        assert!(loaded.save_to(&dir.file()).is_err());
        assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), text);
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
        assert!(!settings.auto_clock);
        assert!(settings.read_error.is_none());
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
    fn a_debug_build_doesnt_start_with_the_system_from_anywhere() {
        // The tests run as a debug build or a release one, and the answer must follow.
        let installed = Path::new("C:/Program Files/Unkludge/unkludge-app.exe");
        assert_eq!(
            starts_with_system_on_first_launch(installed),
            !cfg!(debug_assertions)
        );
        assert!(!starts_with_system_on_first_launch(Path::new(
            "C:/unkludge/target/release/unkludge-app.exe"
        )));
    }

    #[test]
    fn a_program_in_a_build_folder_doesnt_look_installed() {
        for path in [
            "/opt/unkludge/unkludge-app",
            "C:/Program Files/Unkludge/unkludge-app.exe",
            "/home/a/kl-target/debug/unkludge-app",
            "/home/a/my-target-practice/unkludge-app",
        ] {
            assert!(looks_installed(Path::new(path)), "{path}");
        }
        for path in [
            "/home/a/unkludge/target/release/unkludge-app",
            "C:/unkludge/target/debug/unkludge-app.exe",
        ] {
            assert!(!looks_installed(Path::new(path)), "{path}");
        }
    }
}
