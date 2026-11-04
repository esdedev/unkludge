//! One Unkludge program at a time on the keyboard.
//!
//! The system lets any number of programs open a keyboard's collections at once, and nothing on the
//! keyboard tells them apart. So the app and the command-line tool agree here: whoever wants a real
//! keyboard takes this lock first, and keeps it for as long as the keyboard is open. It's a named
//! mutex on Windows and `flock` on a file on Linux, and the system lets go of both when a process
//! ends, however it ends, so a crash can't leave a keyboard locked. A mutex a crashed process held
//! comes back marked as abandoned, which here is just a lock that's free.
//!
//! This isn't the app's single-instance lock, which keeps a second copy of the app from starting.
//! A dry run and a demo open nothing, so they don't come here. The lock is per user session: another
//! user logged in at the same time has a lock of their own.

use std::time::Duration;

use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};

/// How long a command waits for the lock. The app takes it for a moment each time it looks for a
/// keyboard, and a command that comes just then shouldn't be told the keyboard is busy.
pub const COMMAND_WAIT: Duration = Duration::from_secs(1);

pub use imp::{DeviceLock, Place};

/// Takes the lock, then opens the keyboard, so nothing opens without it. The lock stays with the
/// device and goes when it does.
pub fn open_locked<T: Transport>(
    wait: Duration,
    open: impl FnOnce() -> Result<Device<T>, Error>,
) -> Result<Device<T>, Error> {
    open_locked_at(&Place::system()?, wait, open)
}

/// [`open_locked`], with the lock somewhere else.
pub fn open_locked_at<T: Transport>(
    place: &Place,
    wait: Duration,
    open: impl FnOnce() -> Result<Device<T>, Error>,
) -> Result<Device<T>, Error> {
    let lock = DeviceLock::take(place, wait)?;
    let mut device = open()?;
    device.keep_until_closed(Box::new(lock));
    Ok(device)
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use std::sync::mpsc::{Sender, channel};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use unkludge_protocol::Error;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    /// `Local\` is this user's session, as with the app's single-instance mutex.
    const NAME: &str = "Local\\Unkludge-Device";

    /// A mutex's name.
    #[derive(Clone, Debug)]
    pub struct Place(String);

    impl Place {
        pub fn system() -> Result<Place, Error> {
            Ok(Place(NAME.to_string()))
        }

        /// Away from the real one, for tests.
        pub fn scratch(name: &str) -> Place {
            Place(format!("Local\\Unkludge-Test-{}-{name}", std::process::id()))
        }

        /// A name given by another process's [`Place::describe`].
        pub fn from_description(text: &str) -> Place {
            Place(text.to_string())
        }

        pub fn describe(&self) -> String {
            self.0.clone()
        }
    }

    /// Held until dropped. A mutex belongs to the thread that took it, and only that thread can let
    /// go of it, so a thread of its own keeps it. That way the lock can go with a device from one
    /// thread to another, and asking twice from one thread is an answer like any other, where the
    /// mutex would just say yes again.
    #[derive(Debug)]
    pub struct DeviceLock {
        let_go: Option<Sender<()>>,
        owner: Option<JoinHandle<()>>,
    }

    impl DeviceLock {
        /// [`Error::DeviceBusy`] if someone else has it after `wait`.
        pub fn take(place: &Place, wait: Duration) -> Result<DeviceLock, Error> {
            let name: Vec<u16> = OsStr::new(&place.0).encode_wide().chain([0]).collect();
            let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
            let (answer_tx, answer) = channel();
            let (let_go, wait_for_end) = channel::<()>();
            let owner = thread::Builder::new()
                .name("device lock".to_string())
                .spawn(move || match acquire(&name, ms) {
                    Ok(mutex) => {
                        let _ = answer_tx.send(Ok(()));
                        // Ends when the sender goes, which is when the lock is dropped.
                        let _ = wait_for_end.recv();
                        // SAFETY: this is the thread that holds it, and the handle is its own.
                        unsafe {
                            ReleaseMutex(mutex);
                            CloseHandle(mutex);
                        }
                    }
                    Err(error) => {
                        let _ = answer_tx.send(Err(error));
                    }
                })
                .map_err(|error| Error::LockFailed(error.to_string()))?;
            let answer = answer
                .recv()
                .unwrap_or_else(|_| Err(Error::LockFailed("its thread ended".to_string())));
            match answer {
                Ok(()) => Ok(DeviceLock {
                    let_go: Some(let_go),
                    owner: Some(owner),
                }),
                Err(error) => {
                    let _ = owner.join();
                    Err(error)
                }
            }
        }
    }

    /// Takes the mutex on this thread.
    fn acquire(name: &[u16], ms: u32) -> Result<HANDLE, Error> {
        // SAFETY: the name is NUL-terminated, null attributes mean the default ones, and the last
        // error is read at once, on this same thread. The handle is checked before use, and closed
        // here unless it goes back to the caller.
        unsafe {
            let mutex = CreateMutexW(null(), 0, name.as_ptr());
            if mutex.is_null() {
                let code = GetLastError();
                // Only a program running as administrator makes a mutex this one can't open, and it
                // only exists while that program runs.
                if code == ERROR_ACCESS_DENIED {
                    return Err(Error::DeviceBusy);
                }
                return Err(Error::LockFailed(format!("system error {code}")));
            }
            match WaitForSingleObject(mutex, ms) {
                // An abandoned mutex is one a process that ended held, so now it's this thread's.
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(mutex),
                WAIT_TIMEOUT => {
                    CloseHandle(mutex);
                    Err(Error::DeviceBusy)
                }
                _ => {
                    let code = GetLastError();
                    CloseHandle(mutex);
                    Err(Error::LockFailed(format!("system error {code}")))
                }
            }
        }
    }

    impl Drop for DeviceLock {
        /// Only returns once the mutex is free.
        fn drop(&mut self) {
            drop(self.let_go.take());
            if let Some(owner) = self.owner.take() {
                let _ = owner.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use unkludge_protocol::Error;

    use crate::linux_dirs;

    const FILE_NAME: &str = "unkludge-device.lock";
    const LOOK_EVERY: Duration = Duration::from_millis(20);

    /// A lock file.
    #[derive(Clone, Debug)]
    pub struct Place(PathBuf);

    impl Place {
        /// In the runtime folder. See [`linux_dirs::runtime_dir`] for when there's none.
        pub fn system() -> Result<Place, Error> {
            linux_dirs::runtime_dir()
                .map(|dir| Place(dir.join(FILE_NAME)))
                .ok_or_else(|| {
                    Error::LockFailed("no folder for it: HOME isn't set either".to_string())
                })
        }

        /// Away from the real one, for tests.
        pub fn scratch(name: &str) -> Place {
            Place(
                std::env::temp_dir()
                    .join(format!("unkludge-test-{}-{name}.lock", std::process::id())),
            )
        }

        /// A file given by another process's [`Place::describe`].
        pub fn from_description(text: &str) -> Place {
            Place(PathBuf::from(text))
        }

        pub fn describe(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    /// Held until dropped.
    #[derive(Debug)]
    pub struct DeviceLock {
        file: File,
    }

    impl Drop for DeviceLock {
        /// Says so rather than leaving it to the file closing. A program being started from this one
        /// has a copy of the file for a moment, and with that the lock would last until it's done.
        fn drop(&mut self) {
            // SAFETY: the descriptor comes from a file that's open here.
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }

    impl DeviceLock {
        /// [`Error::DeviceBusy`] if someone else has it after `wait`.
        pub fn take(place: &Place, wait: Duration) -> Result<DeviceLock, Error> {
            let file = open_file(&place.0)
                .map_err(|error| Error::LockFailed(format!("{}: {error}", place.0.display())))?;
            let deadline = Instant::now() + wait;
            loop {
                // SAFETY: the descriptor comes from a file that's open here. The lock goes away with
                // the file, which programs started from this one don't inherit.
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    return Ok(DeviceLock { file });
                }
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EWOULDBLOCK) if Instant::now() >= deadline => {
                        return Err(Error::DeviceBusy);
                    }
                    Some(libc::EWOULDBLOCK) => std::thread::sleep(LOOK_EVERY),
                    Some(libc::EINTR) => {}
                    _ => {
                        return Err(Error::LockFailed(format!("{}: {error}", place.0.display())));
                    }
                }
            }
        }
    }

    /// Doesn't follow a link, so that whatever someone left under this name can't make this create or
    /// lock a file elsewhere. A file of another user's can't be opened, and that gets said as it is.
    fn open_file(path: &Path) -> io::Result<File> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::model;
    use unkludge_protocol::transport::fake::FakeTransport;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    const NO_WAIT: Duration = Duration::ZERO;
    const PLACE_VARIABLE: &str = "UNKLUDGE_TEST_LOCK_PLACE";
    const HELD: &str = "the helper has the lock";

    /// What another program sees.
    fn take_from_elsewhere(place: &Place) -> Result<(), Error> {
        DeviceLock::take(place, NO_WAIT).map(drop)
    }

    fn device(fake: &FakeTransport) -> Result<Device<FakeTransport>, Error> {
        Device::new(fake.clone(), None, model::find(0x0265).unwrap())
    }

    #[test]
    fn a_second_taker_is_told_busy_until_the_first_lets_go() {
        let place = Place::scratch("second");
        let lock = DeviceLock::take(&place, NO_WAIT).unwrap();
        assert_eq!(take_from_elsewhere(&place), Err(Error::DeviceBusy));
        drop(lock);
        assert_eq!(take_from_elsewhere(&place), Ok(()));
    }

    #[test]
    fn waiting_gets_a_lock_that_is_let_go_of_meanwhile() {
        let place = Place::scratch("wait");
        let lock = DeviceLock::take(&place, NO_WAIT).unwrap();
        let waiter = {
            let place = place.clone();
            std::thread::spawn(move || DeviceLock::take(&place, Duration::from_secs(20)).map(drop))
        };
        std::thread::sleep(Duration::from_millis(100));
        drop(lock);
        assert_eq!(waiter.join().unwrap(), Ok(()));
    }

    #[test]
    fn a_wait_that_runs_out_is_busy_and_takes_about_that_long() {
        let place = Place::scratch("wait-out");
        let _lock = DeviceLock::take(&place, NO_WAIT).unwrap();
        let waiter = {
            let place = place.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let result = DeviceLock::take(&place, Duration::from_millis(100)).map(drop);
                (result, started.elapsed())
            })
        };
        let (result, took) = waiter.join().unwrap();
        assert_eq!(result, Err(Error::DeviceBusy));
        assert!(took >= Duration::from_millis(90), "{took:?}");
    }

    #[test]
    fn the_keyboard_only_opens_with_the_lock_and_stays_locked_while_open() {
        let place = Place::scratch("device");
        let fake = FakeTransport::new();
        let device = open_locked_at(&place, NO_WAIT, || device(&fake)).unwrap();
        assert_eq!(take_from_elsewhere(&place), Err(Error::DeviceBusy));
        drop(device);
        assert_eq!(take_from_elsewhere(&place), Ok(()));
    }

    #[test]
    fn a_keyboard_someone_else_has_locked_isnt_even_opened() {
        let place = Place::scratch("refused");
        let _held = DeviceLock::take(&place, NO_WAIT).unwrap();
        let mut opened = false;
        let result: Result<Device<FakeTransport>, Error> = open_locked_at(&place, NO_WAIT, || {
            opened = true;
            Err(Error::NotFound)
        });
        assert_eq!(result.err(), Some(Error::DeviceBusy));
        assert!(!opened);
    }

    #[test]
    fn opening_that_fails_lets_go_of_the_lock() {
        let place = Place::scratch("failed-open");
        let result: Result<Device<FakeTransport>, Error> =
            open_locked_at(&place, NO_WAIT, || Err(Error::NotFound));
        assert_eq!(result.err(), Some(Error::NotFound));
        assert_eq!(take_from_elsewhere(&place), Ok(()));
    }

    /// Holds the lock until it's killed. A process of its own, which the test below starts, as the
    /// point is what the system does when a process dies.
    #[test]
    #[ignore = "started by the test below, in a process of its own"]
    fn a_process_that_holds_the_lock() {
        if let Ok(text) = std::env::var(PLACE_VARIABLE) {
            let _lock = DeviceLock::take(&Place::from_description(&text), NO_WAIT).unwrap();
            println!("{HELD}");
            std::thread::sleep(Duration::from_secs(120));
        }
    }

    #[test]
    fn a_lock_can_be_let_go_of_on_another_thread_as_a_device_can_move() {
        fn assert_send<T: Send>() {}
        assert_send::<DeviceLock>();
        let place = Place::scratch("moved");
        let lock = DeviceLock::take(&place, NO_WAIT).unwrap();
        std::thread::spawn(move || drop(lock)).join().unwrap();
        assert_eq!(take_from_elsewhere(&place), Ok(()));
    }

    #[test]
    fn a_process_that_dies_holding_the_lock_doesnt_keep_it() {
        let place = Place::scratch("killed");
        let mut helper = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device_lock::tests::a_process_that_holds_the_lock",
                "--ignored",
                "--nocapture",
            ])
            .env(PLACE_VARIABLE, place.describe())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(helper.stdout.take().unwrap()).lines();
        assert!(
            lines.any(|line| line.is_ok_and(|line| line.contains(HELD))),
            "the helper never got the lock"
        );

        assert_eq!(take_from_elsewhere(&place), Err(Error::DeviceBusy));
        // No clean-up runs in the process: this is what a crash looks like from outside.
        helper.kill().unwrap();
        helper.wait().unwrap();

        // The system may take a moment to tidy up after the process.
        let started = Instant::now();
        while take_from_elsewhere(&place).is_err() {
            assert!(started.elapsed() < Duration::from_secs(20), "still locked");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_link_in_the_way_is_a_failure_and_isnt_followed() {
        let elsewhere =
            std::env::temp_dir().join(format!("unkludge-test-{}-link-target", std::process::id()));
        let link =
            std::env::temp_dir().join(format!("unkludge-test-{}-link.lock", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let place = Place::from_description(&link.to_string_lossy());
        let result = DeviceLock::take(&place, NO_WAIT);
        std::fs::remove_file(&link).unwrap();
        assert!(matches!(result, Err(Error::LockFailed(_))), "{result:?}");
        assert!(!elsewhere.exists());
    }
}
