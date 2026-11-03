//! Windows: where files go, the language, starting with Windows, and one copy at a time.

use std::ffi::{OsStr, c_void};
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
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Unkludge"))
}

pub fn is_spanish() -> bool {
    const LANG_SPANISH: u16 = 0x0A;
    // SAFETY: no arguments, and it can't fail.
    (unsafe { GetUserDefaultUILanguage() } & 0x3FF) == LANG_SPANISH
}

/// By the program's name, so a warning is all this is good for.
pub fn rk_app_running() -> bool {
    unkludge_shared::rk_app::running()
}

/// Opens a URL, a file or a folder with its default program.
pub fn open(target: &Path) {
    let target = wide(target.as_os_str());
    // SAFETY: the strings are NUL-terminated and outlive the call, which doesn't keep them.
    unsafe {
        ShellExecuteW(
            null_mut(),
            w!("open"),
            target.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        );
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

fn run_value() -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut size = size_of_val(&buf) as u32;
    // SAFETY: the strings are NUL-terminated, the buffer and its size in bytes go together, and the
    // type that isn't wanted may be null.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
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

fn approved_value() -> Option<Vec<u8>> {
    let mut buf = [0u8; 64];
    let mut size = buf.len() as u32;
    // SAFETY: as in `run_value`.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            APPROVED_KEY,
            RUN_VALUE,
            RRF_RT_REG_BINARY,
            null_mut(),
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    };
    (result == ERROR_SUCCESS).then(|| buf[..size as usize].to_vec())
}

fn write_run_value() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let value = wide(OsStr::new(&startup_command(&exe)));
    // SAFETY: the strings are NUL-terminated, and the size in bytes includes the NUL, as it should.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            REG_SZ,
            value.as_ptr().cast::<c_void>(),
            size_of_val(value.as_slice()) as u32,
        );
    }
}

fn write_approved() {
    // SAFETY: the strings are NUL-terminated, and the data and its size go together.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            APPROVED_KEY,
            RUN_VALUE,
            REG_BINARY,
            APPROVED_ON.as_ptr().cast::<c_void>(),
            APPROVED_ON.len() as u32,
        );
    }
}

/// Whether the app starts with Windows: this copy, and not disabled in Task Manager.
pub fn autostart_enabled() -> bool {
    let this_copy = match (run_value(), std::env::current_exe()) {
        (Some(command), Ok(exe)) => command == startup_command(&exe),
        _ => false,
    };
    this_copy && !approved_value().is_some_and(|value| approval_disabled(&value))
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
        let dir = data_dir().unwrap();
        assert_eq!(dir.file_name().unwrap(), "Unkludge");
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
