//! Where files go on Linux.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const APP_NAME: &str = "unkludge";

/// An environment variable holding an absolute path. The XDG rules treat anything else as not set.
pub fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// `~/.config/unkludge`, or `$XDG_CONFIG_HOME/unkludge` if that's set.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_in(env_dir("XDG_CONFIG_HOME"), env_dir("HOME"))
}

fn data_dir_in(config: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let config = config.or_else(|| Some(home?.join(".config")))?;
    Some(config.join(APP_NAME))
}

/// For what only lasts while a program runs, which is the app's socket: `$XDG_RUNTIME_DIR` when it's
/// this user's, as `su` can leave another user's in the environment. Otherwise the data folder, which
/// unlike the system's temporary folder no other user can put things in. Two programs with different
/// environments don't share this, so nothing that has to keep them apart can rely on it.
pub fn runtime_dir() -> Option<PathBuf> {
    // SAFETY: no arguments, and it can't fail.
    let uid = unsafe { libc::getuid() };
    runtime_dir_in(env_dir("XDG_RUNTIME_DIR"), data_dir(), uid)
}

fn runtime_dir_in(
    runtime: Option<PathBuf>,
    data: Option<PathBuf>,
    uid: libc::uid_t,
) -> Option<PathBuf> {
    runtime.filter(|dir| is_own_folder(dir, uid)).or(data)
}

fn is_own_folder(dir: &Path, uid: libc::uid_t) -> bool {
    std::fs::metadata(dir).is_ok_and(|about| about.is_dir() && about.uid() == uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn this_user() -> libc::uid_t {
        // SAFETY: no arguments, and it can't fail.
        unsafe { libc::getuid() }
    }

    #[test]
    fn the_data_folder_is_the_apps_own_under_the_config_folder() {
        assert_eq!(
            data_dir_in(Some("/x/config".into()), Some("/home/ana".into())),
            Some(PathBuf::from("/x/config/unkludge"))
        );
        assert_eq!(
            data_dir_in(None, Some("/home/ana".into())),
            Some(PathBuf::from("/home/ana/.config/unkludge"))
        );
        assert_eq!(data_dir_in(None, None), None);
    }

    #[test]
    fn the_runtime_folder_is_used_when_it_is_this_users() {
        let own = std::env::temp_dir().join(format!("unkludge-dirs-{}", std::process::id()));
        std::fs::create_dir_all(&own).unwrap();
        let data = PathBuf::from("/home/ana/.config/unkludge");
        let chosen = runtime_dir_in(Some(own.clone()), Some(data), this_user());
        assert_eq!(chosen, Some(own.clone()));
        std::fs::remove_dir(&own).unwrap();
    }

    #[test]
    fn without_a_runtime_folder_nothing_goes_in_a_folder_shared_by_all_users() {
        let data = PathBuf::from("/home/ana/.config/unkludge");
        assert_eq!(
            runtime_dir_in(None, Some(data.clone()), this_user()),
            Some(data)
        );
        assert_eq!(runtime_dir_in(None, None, this_user()), None);
    }

    #[test]
    fn another_users_runtime_folder_isnt_used() {
        let own = std::env::temp_dir().join(format!("unkludge-dirs-other-{}", std::process::id()));
        std::fs::create_dir_all(&own).unwrap();
        let data = PathBuf::from("/home/ana/.config/unkludge");
        let someone_else = this_user().wrapping_add(1);
        let chosen = runtime_dir_in(Some(own.clone()), Some(data.clone()), someone_else);
        assert_eq!(chosen, Some(data));
        std::fs::remove_dir(&own).unwrap();
    }

    #[test]
    fn a_runtime_folder_that_isnt_there_or_isnt_a_folder_isnt_used() {
        let data = PathBuf::from("/home/ana/.config/unkludge");
        let missing = PathBuf::from("/nonexistent/unkludge-runtime");
        assert_eq!(
            runtime_dir_in(Some(missing), Some(data.clone()), this_user()),
            Some(data.clone())
        );
        let file = std::env::temp_dir().join(format!("unkludge-dirs-file-{}", std::process::id()));
        std::fs::write(&file, "").unwrap();
        let chosen = runtime_dir_in(Some(file.clone()), Some(data.clone()), this_user());
        std::fs::remove_file(&file).unwrap();
        assert_eq!(chosen, Some(data));
    }
}
