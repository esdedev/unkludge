//! Noticing the official app, which polls the keyboard and sends its own settings back to it.

/// Whether the official app seems to be running. It goes by the program's name, which could in
/// principle belong to something else, so it's only good for a warning.
#[cfg(windows)]
pub fn running() -> bool {
    any_process(is_rk_app)
}

/// The app is Windows-only.
#[cfg(not(windows))]
pub fn running() -> bool {
    false
}

/// Whether a program with a name like this is running. It gets the name as the process list has it.
#[cfg(windows)]
fn any_process(wanted: impl Fn(&[u16]) -> bool) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot handle is checked before use and closed once. `entry` is a plain struct
    // of integers, valid when zeroed, with the size field the two calls require.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more && !found {
            found = wanted(&entry.szExeFile);
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

/// A NUL-terminated program name from the process list.
#[cfg(windows)]
fn name_of(exe_file: &[u16]) -> String {
    let len = exe_file
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(exe_file.len());
    String::from_utf16_lossy(&exe_file[..len])
}

/// Whether a program name from the process list is the official app's.
#[cfg(windows)]
fn is_rk_app(exe_file: &[u16]) -> bool {
    name_of(exe_file).eq_ignore_ascii_case("DeviceDriver.exe")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn name(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0, b'x'.into()]).collect()
    }

    #[test]
    fn the_name_is_compared_up_to_the_nul_ignoring_case() {
        assert!(is_rk_app(&name("DeviceDriver.exe")));
        assert!(is_rk_app(&name("devicedriver.EXE")));
        assert!(!is_rk_app(&name("MyDeviceDriver.exe")));
        assert!(!is_rk_app(&name("DeviceDriver.exe2")));
        assert!(!is_rk_app(&[]));
    }

    #[test]
    fn the_process_list_has_this_program_and_not_one_that_doesnt_exist() {
        let exe = std::env::current_exe().unwrap();
        let this = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(any_process(|found| name_of(found) == this), "{this}");
        assert!(!any_process(|found| name_of(found) == "no-such-program.exe"));
    }
}
