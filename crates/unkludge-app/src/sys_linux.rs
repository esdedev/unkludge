//! Linux: where files go, the language, starting with the session, one copy at a time, and the udev
//! rule that gives access to keyboards.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const APP_NAME: &str = "unkludge";

/// In the binary, so that it can be installed from the plain binary or an AppImage too.
const UDEV_RULE: &str = include_str!("../../../packaging/linux/70-unkludge.rules");
const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/70-unkludge.rules";

/// Why [`grant_access`] didn't work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantError {
    /// There's no pkexec on this system.
    NoPkexec,
    /// The user closed the dialog, isn't allowed, or there's no polkit agent to ask.
    Cancelled,
    /// A command failed as root.
    Failed,
}

/// The commands run as root. They're fixed text and nothing from the user or the environment. Any
/// failure exits with 3, which can't be taken for pkexec's own 126 and 127.
fn grant_script() -> String {
    let rule = UDEV_RULE.replace('\r', "");
    format!(
        "{{ umask 022 && printf '%s' '{rule}' > {UDEV_RULE_PATH} && \
         udevadm control --reload-rules && \
         udevadm trigger --subsystem-match=hidraw --action=change; }} || exit 3"
    )
}

/// Installs the udev rule as root, with one password prompt, and has udev apply it, so that the
/// keyboard opens without unplugging it. Waits for the user, so don't call this from the interface's
/// thread.
pub fn grant_access() -> Result<(), GrantError> {
    let status = std::process::Command::new("pkexec")
        .args(["/bin/sh", "-c", &grant_script()])
        .status()
        .map_err(|_| GrantError::NoPkexec)?;
    match status.code() {
        Some(0) => Ok(()),
        // 126: the dialog was closed. 127: not allowed, or no polkit agent.
        Some(126 | 127) => Err(GrantError::Cancelled),
        _ => Err(GrantError::Failed),
    }
}

/// An environment variable holding an absolute path. The XDG rules treat anything else as not set.
fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// `~/.config/unkludge`
pub fn data_dir() -> Option<PathBuf> {
    let config = env_dir("XDG_CONFIG_HOME").or_else(|| Some(env_dir("HOME")?.join(".config")))?;
    Some(config.join(APP_NAME))
}

fn locale_is_spanish(locale: &str) -> bool {
    locale.starts_with("es")
}

pub fn is_spanish() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .is_some_and(|value| locale_is_spanish(&value))
}

/// RK's app doesn't exist for Linux.
pub fn rk_app_running() -> bool {
    crate::rk_app::running()
}

/// Opens a URL, a file or a folder with its default program.
pub fn open(target: &Path) {
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

// Starting with the session

fn autostart_file() -> Option<PathBuf> {
    Some(
        data_dir()?
            .parent()?
            .join("autostart")
            .join(format!("{APP_NAME}.desktop")),
    )
}

/// A path quoted for a `.desktop` file's `Exec` key. Within the quotes `"`, `` ` ``, `$` and `\` take
/// a backslash, and as the value is also a string, those backslashes are written twice.
fn exec_quote(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for c in path.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The program that an `Exec=` line starts: its first argument, quoted or not.
fn exec_program(line: &str) -> Option<PathBuf> {
    let value = line.strip_prefix("Exec=")?;
    let Some(quoted) = value.strip_prefix('"') else {
        return value.split_whitespace().next().map(PathBuf::from);
    };
    // First the string level, `\\` for `\`, then the quoting level, `\x` for `x`.
    let mut unescaped = String::new();
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => unescaped.push(chars.next()?),
            _ => unescaped.push(c),
        }
    }
    let mut path = String::new();
    let mut chars = unescaped.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => path.push(chars.next()?),
            '"' => return Some(PathBuf::from(path)),
            _ => path.push(c),
        }
    }
    None
}

fn exec_line(exe: &Path) -> String {
    format!("Exec={} --tray", exec_quote(exe))
}

/// What goes in the session's startup entry for this program.
fn autostart_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Unkludge\n\
         Comment=Screen, clock and lights of Royal Kludge keyboards\n\
         Icon={APP_NAME}\n\
         {}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_line(exe)
    )
}

/// What to start to get this copy: the AppImage when running from one, as the program itself is in
/// a temporary mount, and otherwise the program. `$APPIMAGE` only counts if this program is inside
/// `$APPDIR`, as anything another AppImage starts inherits the variable.
fn app_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(chosen_exe(exe, env_dir("APPIMAGE"), env_dir("APPDIR")))
}

fn chosen_exe(exe: PathBuf, image: Option<PathBuf>, appdir: Option<PathBuf>) -> PathBuf {
    match (image, appdir) {
        (Some(image), Some(dir)) if exe.starts_with(&dir) => image,
        _ => exe,
    }
}

/// A Unkludge AppImage. They have the version in the name, which changes when upgrading.
fn is_unkludge_appimage(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("AppImage"))
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().to_lowercase().starts_with(APP_NAME))
}

fn entry_program(file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(exec_program)
}

fn enabled_in(file: &Path, exe: &Path) -> bool {
    let line = exec_line(exe);
    std::fs::read_to_string(file).is_ok_and(|text| text.lines().any(|l| l == line))
}

fn set_in(file: &Path, on: bool, exe: &Path) {
    if !on {
        let _ = std::fs::remove_file(file);
        return;
    }
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let _ = std::fs::write(file, autostart_entry(exe));
}

/// See [`refresh_autostart`].
fn refresh_in(file: &Path, exe: &Path, force: bool) {
    let Some(target) = entry_program(file) else {
        return;
    };
    let other_appimage = is_unkludge_appimage(exe) && is_unkludge_appimage(&target);
    if target != exe && (force || !target.exists() || other_appimage) {
        set_in(file, true, exe);
    }
}

pub fn autostart_enabled() -> bool {
    match (autostart_file(), app_exe()) {
        (Some(file), Some(exe)) => enabled_in(&file, &exe),
        _ => false,
    }
}

pub fn set_autostart(on: bool) {
    if cfg!(test) {
        panic!("tests must not change the real autostart entry");
    }
    if let (Some(file), Some(exe)) = (autostart_file(), app_exe()) {
        set_in(&file, on, &exe);
    }
}

/// If the app starts with the session but the entry points at another program, points it at this
/// one: always with `force`, and otherwise when that one no longer exists or is another Unkludge
/// AppImage. With no entry this does nothing, as the user turned it off.
pub fn refresh_autostart(force: bool) {
    if cfg!(test) {
        panic!("tests must not change the real autostart entry");
    }
    if let (Some(file), Some(exe)) = (autostart_file(), app_exe()) {
        refresh_in(&file, &exe, force);
    }
}

// One copy at a time

fn runtime_dir() -> PathBuf {
    env_dir("XDG_RUNTIME_DIR").unwrap_or_else(std::env::temp_dir)
}

fn show_socket() -> PathBuf {
    runtime_dir().join(format!("{APP_NAME}.sock"))
}

/// How trying to be the only copy went.
#[derive(Debug)]
enum Lock {
    /// This copy is the one, as long as the file stays open.
    Held(File),
    /// Another copy is.
    Taken,
    /// There's no telling, which counts as a yes: two copies are better than none.
    Untried,
}

fn take_lock(dir: &Path) -> Lock {
    let _ = std::fs::create_dir_all(dir);
    let Ok(file) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("app.lock"))
    else {
        return Lock::Untried;
    };
    // SAFETY: the descriptor comes from a file that's open here. The lock goes away with the file.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Lock::Held(file)
    } else {
        Lock::Taken
    }
}

/// Whether this is the only copy running. If another one is, it gets asked to show its window, and
/// this one should quit. The lock lasts until the process ends, however it ends.
pub fn claim_single_instance() -> bool {
    static LOCK: OnceLock<File> = OnceLock::new();
    let Some(dir) = data_dir() else {
        return true;
    };
    match take_lock(&dir) {
        Lock::Held(file) => {
            let _ = LOCK.set(file);
            true
        }
        Lock::Untried => true,
        Lock::Taken => {
            ask_to_show(&show_socket());
            false
        }
    }
}

fn ask_to_show(socket: &Path) {
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let _ = stream.write_all(b"show");
    }
}

/// Calls `show` whenever another copy that started asks for the window. Only for the copy that
/// [`claim_single_instance`] said yes to, as it replaces a socket left behind.
pub fn on_show_request(show: impl Fn() + Send + 'static) {
    listen_for_show(&show_socket(), show);
}

fn listen_for_show(socket: &Path, show: impl Fn() + Send + 'static) {
    let _ = std::fs::remove_file(socket);
    let Ok(listener) = UnixListener::bind(socket) else {
        return;
    };
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0; 8];
            if stream.read(&mut buf).is_ok_and(|read| read > 0) {
                show();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("unkludge-sys-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_rule_is_the_one_that_gives_access_to_this_vendor() {
        let lines: Vec<&str> = UDEV_RULE
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .collect();
        assert_eq!(
            lines,
            [r#"KERNEL=="hidraw*", ATTRS{idVendor}=="258a", TAG+="uaccess""#]
        );
        assert!(!UDEV_RULE.contains('\r'));
    }

    #[test]
    fn the_root_script_is_fixed_text() {
        assert!(!UDEV_RULE.contains('\''), "the rule goes in single quotes");
        let script = grant_script();
        assert!(script.contains(r#"ATTRS{idVendor}=="258a""#));
        assert!(script.contains("> /etc/udev/rules.d/70-unkludge.rules &&"));
        assert!(
            !script.contains('$') && !script.contains('`'),
            "nothing the shell would expand"
        );
        let syntax = std::process::Command::new("sh")
            .args(["-n", "-c", &script])
            .status()
            .unwrap();
        assert!(syntax.success(), "the root script has a syntax error");
    }

    #[test]
    fn a_failure_in_the_root_script_isnt_a_pkexec_code() {
        // Runs for real, as this user, but it can't get past the first command.
        let script = grant_script().replace(UDEV_RULE_PATH, "/nonexistent-dir/rule");
        assert!(script.contains("/nonexistent-dir/rule"));
        let code = std::process::Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap()
            .code();
        assert_eq!(code, Some(3));
    }

    #[test]
    fn spanish_locales_are_recognised() {
        assert!(locale_is_spanish("es_ES.UTF-8"));
        assert!(locale_is_spanish("es"));
        assert!(!locale_is_spanish("en_US.UTF-8"));
        assert!(!locale_is_spanish("C"));
    }

    #[test]
    fn the_data_folder_is_the_apps_own() {
        // Without HOME or XDG_CONFIG_HOME there's no answer, which CI won't have.
        if let Some(dir) = data_dir() {
            assert_eq!(dir.file_name().unwrap(), "unkludge");
        }
    }

    #[test]
    fn exec_quoting_round_trips() {
        for path in [
            "/home/ana/.local/bin/unkludge-app",
            "/home/Ana María/bin/unkludge-app",
            "/tmp/a\"b$c`d\\e/unkludge-app",
        ] {
            let line = exec_line(Path::new(path));
            assert_eq!(exec_program(&line), Some(PathBuf::from(path)), "{line}");
        }
        assert_eq!(exec_quote(Path::new("/a b")), "\"/a b\"");
        assert_eq!(
            exec_program("Exec=/usr/bin/unkludge-app --tray"),
            Some(PathBuf::from("/usr/bin/unkludge-app"))
        );
        assert_eq!(exec_program("Name=Unkludge"), None);
        assert_eq!(exec_program("Exec=\"unterminated"), None);
    }

    #[test]
    fn the_entry_starts_this_program_in_the_tray() {
        let entry = autostart_entry(Path::new("/opt/My Apps/unkludge-app"));
        let lines: Vec<&str> = entry.lines().collect();
        assert_eq!(lines[0], "[Desktop Entry]");
        assert!(lines.contains(&"Type=Application"));
        assert!(lines.contains(&"Name=Unkludge"));
        assert!(lines.contains(&"Icon=unkludge"));
        assert!(lines.contains(&"Exec=\"/opt/My Apps/unkludge-app\" --tray"));
        assert!(lines.contains(&"Terminal=false"));
        assert!(lines.contains(&"X-GNOME-Autostart-enabled=true"));
    }

    #[test]
    fn only_a_unkludge_appimage_counts_as_one() {
        assert!(is_unkludge_appimage(Path::new(
            "/home/a/Apps/Unkludge-0.2.0-x86_64.AppImage"
        )));
        assert!(!is_unkludge_appimage(Path::new(
            "/home/a/Apps/VSCodium.AppImage"
        )));
        assert!(!is_unkludge_appimage(Path::new("/usr/bin/unkludge-app")));
    }

    #[test]
    fn the_appimage_is_only_believed_from_inside_its_mount() {
        let image = PathBuf::from("/home/a/Unkludge.AppImage");
        let appdir = PathBuf::from("/tmp/.mount_Unkludge");
        let inside = PathBuf::from("/tmp/.mount_Unkludge/usr/bin/unkludge-app");
        let outside = PathBuf::from("/usr/bin/unkludge-app");
        assert_eq!(
            chosen_exe(inside.clone(), Some(image.clone()), Some(appdir.clone())),
            image
        );
        assert_eq!(
            chosen_exe(outside.clone(), Some(image.clone()), Some(appdir)),
            outside
        );
        assert_eq!(chosen_exe(inside.clone(), Some(image), None), inside);
    }

    #[test]
    fn turning_it_on_and_off_uses_the_entry() {
        let dir = TempDir::new("on-off");
        let file = dir.0.join("autostart").join("unkludge.desktop");
        let exe = Path::new("/opt/unkludge-app");
        assert!(!enabled_in(&file, exe));

        set_in(&file, true, exe);
        assert!(enabled_in(&file, exe));
        assert_eq!(entry_program(&file), Some(exe.to_path_buf()));
        assert!(!enabled_in(&file, Path::new("/opt/other")));

        set_in(&file, false, exe);
        assert!(!enabled_in(&file, exe));
        assert!(!file.exists());
    }

    #[test]
    fn refreshing_moves_the_entry_only_when_it_should() {
        let dir = TempDir::new("refresh");
        let file = dir.0.join("unkludge.desktop");
        let existing = dir.0.join("old-copy");
        std::fs::write(&existing, "").unwrap();
        let exe = Path::new("/opt/unkludge-app");

        // No entry: the user turned it off.
        refresh_in(&file, exe, true);
        assert!(!file.exists());

        // The other copy is there, so it stays, unless forced.
        set_in(&file, true, &existing);
        refresh_in(&file, exe, false);
        assert_eq!(entry_program(&file), Some(existing.clone()));
        refresh_in(&file, exe, true);
        assert_eq!(entry_program(&file), Some(exe.to_path_buf()));

        // The other copy is gone.
        set_in(&file, true, &dir.0.join("gone"));
        refresh_in(&file, exe, false);
        assert_eq!(entry_program(&file), Some(exe.to_path_buf()));

        // An upgrade from one AppImage to the next, with the old one still around.
        let old_image = dir.0.join("Unkludge-0.1.0.AppImage");
        std::fs::write(&old_image, "").unwrap();
        let new_image = dir.0.join("Unkludge-0.2.0.AppImage");
        set_in(&file, true, &old_image);
        refresh_in(&file, &new_image, false);
        assert_eq!(entry_program(&file), Some(new_image));
    }

    #[test]
    fn the_first_claim_wins_until_it_lets_go() {
        let dir = TempDir::new("lock");
        let first = take_lock(&dir.0);
        assert!(matches!(first, Lock::Held(_)), "{first:?}");
        assert!(matches!(take_lock(&dir.0), Lock::Taken));
        drop(first);
        assert!(matches!(take_lock(&dir.0), Lock::Held(_)));
    }

    #[test]
    fn a_folder_that_cant_be_used_doesnt_stop_the_app() {
        let dir = TempDir::new("no-folder");
        let not_a_folder = dir.0.join("file");
        std::fs::write(&not_a_folder, "").unwrap();
        assert!(matches!(take_lock(&not_a_folder), Lock::Untried));
    }

    #[test]
    fn another_copy_can_ask_for_the_window() {
        let dir = TempDir::new("socket");
        let socket = dir.0.join("test.sock");
        // One left behind by a copy that crashed doesn't get in the way.
        std::fs::write(&socket, "").unwrap();
        let (shown_tx, shown) = channel();
        listen_for_show(&socket, move || {
            let _ = shown_tx.send(());
        });
        ask_to_show(&socket);
        assert_eq!(shown.recv_timeout(Duration::from_secs(10)), Ok(()));
    }

    #[test]
    fn asking_with_no_copy_listening_does_nothing() {
        let dir = TempDir::new("no-socket");
        ask_to_show(&dir.0.join("nobody.sock"));
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
    fn the_rk_app_never_runs_here() {
        assert!(!rk_app_running());
    }
}
