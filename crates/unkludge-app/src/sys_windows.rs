//! Windows: where files go, the language, starting with Windows, and one copy at a time.

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError};
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows_sys::core::PCWSTR;
use windows_sys::w;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Unkludge");
/// The state of each `Run` entry in Task Manager. Windows may skip an entry that has no value here
/// at sign-in, so one gets written along with it.
const APPROVED_KEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");
/// What Task Manager writes when enabling an entry. When disabling it's 03 and the date.
const APPROVED_ON: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

const SINGLE_INSTANCE_MUTEX: &str = "Local\\Unkludge-App";

/// A NUL-terminated copy for system calls.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

/// `%APPDATA%\Unkludge`
pub fn data_dir() -> Option<PathBuf> {
    data_dir_in(std::env::var_os("APPDATA"))
}

fn data_dir_in(appdata: Option<OsString>) -> Option<PathBuf> {
    let appdata = appdata.filter(|value| !value.is_empty())?;
    Some(PathBuf::from(appdata).join("Unkludge"))
}

pub fn is_spanish() -> bool {
    // SAFETY: no arguments, and it can't fail.
    language_is_spanish(unsafe { GetUserDefaultUILanguage() })
}

/// The low bits of a language ID are the language, whatever the country in the high bits.
fn language_is_spanish(language: u16) -> bool {
    const LANG_SPANISH: u16 = 0x0A;
    language & 0x3FF == LANG_SPANISH
}

/// By the program's name, so a warning is all this is good for.
pub fn rk_app_running() -> bool {
    unkludge_shared::rk_app::running()
}

/// Opens a URL, a file or a folder with its default program.
pub fn open(target: &Path) -> io::Result<()> {
    let target = wide(target.as_os_str());
    // SAFETY: the strings are NUL-terminated and outlive the call, which doesn't keep them.
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            w!("open"),
            target.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    shell_result(result as isize)
}

/// The system says anything above 32 is success, and the rest are error codes.
fn shell_result(code: isize) -> io::Result<()> {
    if code > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the system couldn't open it: error {code}"
        )))
    }
}

// Starting with Windows

/// What the `Run` entry holds for this program: start hidden in the tray.
fn startup_command(exe: &Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// The program a `Run` entry starts.
fn command_program(command: &str) -> PathBuf {
    PathBuf::from(
        command
            .trim_start_matches('"')
            .split('"')
            .next()
            .unwrap_or_default(),
    )
}

/// Whether an entry should move to this copy. See [`refresh_autostart`].
fn needs_repointing(
    command: &str,
    exe: &Path,
    force: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> bool {
    let target = command_program(command);
    target != exe && (force || !exists(&target))
}

/// An odd first byte (03, 07) means disabled in Task Manager, and an even one (02, 06) enabled.
fn approval_disabled(value: &[u8]) -> bool {
    value.first().is_some_and(|byte| byte & 1 == 1)
}

fn read_string(key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut size = size_of_val(&buf) as u32;
    // SAFETY: the strings are NUL-terminated, the buffer and its size in bytes go together, and the
    // type that isn't wanted may be null.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    };
    (result == ERROR_SUCCESS).then(|| {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..len])
    })
}

fn read_binary(key: PCWSTR, value: PCWSTR) -> Option<Vec<u8>> {
    let mut buf = [0u8; 64];
    let mut size = buf.len() as u32;
    // SAFETY: as in `read_string`.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_BINARY,
            null_mut(),
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    };
    (result == ERROR_SUCCESS).then(|| buf[..size as usize].to_vec())
}

/// How much of a string to hand to the registry: all of it in bytes, with its NUL.
fn string_size(text: &[u16]) -> u32 {
    size_of_val(text) as u32
}

fn write_string(key: PCWSTR, value: PCWSTR, text: &str) {
    let text = wide(OsStr::new(text));
    // SAFETY: the strings are NUL-terminated, and the data and its size go together.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            REG_SZ,
            text.as_ptr().cast::<c_void>(),
            string_size(&text),
        );
    }
}

fn write_binary(key: PCWSTR, value: PCWSTR, data: &[u8]) {
    // SAFETY: the strings are NUL-terminated, and the data and its size go together.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            REG_BINARY,
            data.as_ptr().cast::<c_void>(),
            data.len() as u32,
        );
    }
}

fn run_value() -> Option<String> {
    read_string(RUN_KEY, RUN_VALUE)
}

fn approved_value() -> Option<Vec<u8>> {
    read_binary(APPROVED_KEY, RUN_VALUE)
}

fn write_run_value() {
    if let Ok(exe) = std::env::current_exe() {
        write_string(RUN_KEY, RUN_VALUE, &startup_command(&exe));
    }
}

fn write_approved() {
    write_binary(APPROVED_KEY, RUN_VALUE, &APPROVED_ON);
}

/// Whether the app starts with Windows: this copy, and not disabled in Task Manager.
pub fn autostart_enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    enabled_by(run_value().as_deref(), &exe, approved_value().as_deref())
}

fn enabled_by(command: Option<&str>, exe: &Path, approved: Option<&[u8]>) -> bool {
    let this_copy = command.is_some_and(|command| command == startup_command(exe));
    this_copy && !approved.is_some_and(approval_disabled)
}

pub fn set_autostart(on: bool) {
    if cfg!(test) {
        panic!("tests must not change the real autostart entry");
    }
    if on {
        write_run_value();
        write_approved();
    } else {
        // SAFETY: the strings are NUL-terminated.
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED_KEY, RUN_VALUE);
        }
    }
}

/// If the app starts with Windows but the entry points at another program, points it at this one:
/// always with `force`, and otherwise when that one no longer exists. A missing Task Manager state
/// gets written, and one the user disabled is respected. With no entry this does nothing, as the
/// user turned it off.
pub fn refresh_autostart(force: bool) {
    if cfg!(test) {
        panic!("tests must not change the real autostart entry");
    }
    let (Some(command), Ok(exe)) = (run_value(), std::env::current_exe()) else {
        return;
    };
    if needs_repointing(&command, &exe, force, &|path| path.exists()) {
        write_run_value();
    }
    if approved_value().is_none() {
        write_approved();
    }
}

// One copy at a time

/// Whether this is the only copy running. If it isn't, this one should quit, after asking the other
/// to show its window, which is up to the interface as it knows the window class.
pub fn claim_single_instance() -> bool {
    claim_mutex(SINGLE_INSTANCE_MUTEX)
}

/// If the mutex can't be made this also says yes: two copies are better than none. The handle stays
/// open, so that the mutex lasts until the process ends, however it ends.
fn claim_mutex(name: &str) -> bool {
    let name = wide(OsStr::new(name));
    // SAFETY: the name is NUL-terminated, null attributes mean the default ones, and the last error
    // is read at once, on this same thread.
    unsafe {
        let mutex = CreateMutexW(null(), 1, name.as_ptr());
        mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;

    #[test]
    fn the_startup_command_quotes_the_program_and_starts_in_the_tray() {
        let exe = Path::new(r"C:\Program Files\Unkludge\unkludge-app.exe");
        assert_eq!(
            startup_command(exe),
            r#""C:\Program Files\Unkludge\unkludge-app.exe" --tray"#
        );
    }

    #[test]
    fn the_program_comes_back_out_of_the_command() {
        let exe = Path::new(r"C:\Program Files\Unkludge\unkludge-app.exe");
        assert_eq!(command_program(&startup_command(exe)), exe);
        assert_eq!(command_program(r"C:\a.exe"), Path::new(r"C:\a.exe"));
        assert_eq!(command_program(""), Path::new(""));
    }

    #[test]
    fn the_entry_moves_to_this_copy_if_forced_or_if_the_other_is_gone() {
        let here = Path::new(r"C:\here\unkludge-app.exe");
        let there = startup_command(Path::new(r"C:\there\unkludge-app.exe"));
        let exists = |_: &Path| true;
        let gone = |_: &Path| false;
        assert!(needs_repointing(&there, here, true, &exists));
        assert!(needs_repointing(&there, here, false, &gone));
        assert!(!needs_repointing(&there, here, false, &exists));
        // Already right, even with force.
        let right = startup_command(here);
        assert!(!needs_repointing(&right, here, true, &gone));
    }

    #[test]
    fn approval_state_follows_task_manager() {
        assert!(!approval_disabled(&APPROVED_ON));
        assert!(!approval_disabled(&[6, 0, 0, 0]));
        assert!(approval_disabled(&[3, 0, 0, 0, 0x5A, 0x1C]));
        assert!(approval_disabled(&[7]));
        assert!(!approval_disabled(&[]));
    }

    #[test]
    fn the_data_folder_is_the_apps_own() {
        assert_eq!(
            data_dir_in(Some(OsString::from(r"C:\Users\a\AppData\Roaming"))),
            Some(PathBuf::from(r"C:\Users\a\AppData\Roaming\Unkludge"))
        );
    }

    #[test]
    fn an_empty_or_missing_appdata_means_no_data_folder() {
        assert_eq!(data_dir_in(None), None);
        assert_eq!(data_dir_in(Some(OsString::new())), None);
    }

    #[test]
    fn spanish_is_recognised_whatever_the_country() {
        // Spain, Mexico, Argentina.
        for language in [0x0C0A, 0x080A, 0x2C0A] {
            assert!(language_is_spanish(language), "{language:04X}");
        }
        // English, French, and one with the same low byte but a different language.
        for language in [0x0409, 0x040C, 0x010A] {
            assert!(!language_is_spanish(language), "{language:04X}");
        }
    }

    #[test]
    fn opening_succeeds_only_above_the_error_codes() {
        assert!(shell_result(42).is_ok());
        assert!(shell_result(33).is_ok());
        assert!(shell_result(32).is_err());
        let error = shell_result(2).unwrap_err();
        assert_eq!(error.to_string(), "the system couldn't open it: error 2");
        assert!(shell_result(0).is_err());
    }

    #[test]
    fn it_is_enabled_only_for_this_copy_and_not_disabled_in_task_manager() {
        let here = Path::new(r"C:\here\unkludge-app.exe");
        let this_copy = startup_command(here);
        let other = startup_command(Path::new(r"C:\there\unkludge-app.exe"));
        assert!(enabled_by(Some(&this_copy), here, None));
        assert!(enabled_by(Some(&this_copy), here, Some(&APPROVED_ON)));
        assert!(!enabled_by(Some(&this_copy), here, Some(&[3, 0])));
        assert!(!enabled_by(Some(&other), here, Some(&APPROVED_ON)));
        assert!(!enabled_by(None, here, Some(&APPROVED_ON)));
    }

    /// A key of the test's own, far from anything that starts programs, gone at the end.
    struct ScratchKey(Vec<u16>);

    impl ScratchKey {
        fn new(name: &str) -> ScratchKey {
            let path = format!("Software\\Unkludge-Test-{}-{name}", std::process::id());
            ScratchKey(wide(OsStr::new(&path)))
        }
    }

    impl Drop for ScratchKey {
        fn drop(&mut self) {
            // SAFETY: the name is NUL-terminated.
            unsafe {
                RegDeleteTreeW(HKEY_CURRENT_USER, self.0.as_ptr());
            }
        }
    }

    #[test]
    fn a_string_size_is_in_bytes_with_the_nul() {
        assert_eq!(string_size(&wide(OsStr::new("ab"))), 6);
    }

    #[test]
    fn a_long_string_comes_back_from_the_registry_whole() {
        let key = ScratchKey::new("string");
        let value = w!("Some value");
        // Longer than half the buffer, in characters that take more than a byte.
        let text = format!("\"C:\\Usuarios\\José\\{}\" --tray", "ñ".repeat(600));
        assert_eq!(read_string(key.0.as_ptr(), value), None);
        write_string(key.0.as_ptr(), value, &text);
        assert_eq!(read_string(key.0.as_ptr(), value), Some(text));
    }

    #[test]
    fn binary_values_come_back_from_the_registry_whole() {
        let key = ScratchKey::new("binary");
        let value = w!("Some value");
        assert_eq!(read_binary(key.0.as_ptr(), value), None);
        write_binary(key.0.as_ptr(), value, &APPROVED_ON);
        assert_eq!(
            read_binary(key.0.as_ptr(), value).as_deref(),
            Some(&APPROVED_ON[..])
        );
    }

    #[test]
    fn the_language_can_be_asked_for() {
        // The answer depends on the machine.
        let _ = is_spanish();
    }

    #[test]
    fn the_first_claim_wins() {
        let name = format!("Local\\Unkludge-Test-{}", std::process::id());
        assert!(claim_mutex(&name));
        assert!(!claim_mutex(&name));
    }

    #[test]
    #[should_panic(expected = "tests must not change")]
    fn a_test_cant_change_the_real_entry_by_accident() {
        set_autostart(true);
    }

    #[test]
    #[should_panic(expected = "tests must not change")]
    fn a_test_cant_refresh_the_real_entry_by_accident() {
        refresh_autostart(true);
    }

    #[test]
    fn looking_through_the_process_list_works() {
        let _ = rk_app_running();
    }
}
