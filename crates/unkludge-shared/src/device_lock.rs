//! One Unkludge program at a time on the keyboard.
//!
//! The system lets any number of programs open a keyboard's collections at once, and nothing on the
//! keyboard tells them apart. So the app and the command-line tool agree: whoever wants a real keyboard
//! holds a lock from before it opens until it's closed. The system lets go when a process ends, however
//! it ends, so a crash can't leave a keyboard locked.
//!
//! On Windows it's a named mutex here. A mutex a crashed process held comes back marked as abandoned,
//! which here is just a lock that's free. It's per user session: another user logged in at the same
//! time has a lock of their own. It covers all keyboards: there's one mutex.
//!
//! On Linux it's `flock` on the keyboard's hidraw node, which only the protocol crate's opening gets to
//! see, so it's there and not here: see its `device::node_lock`. That's per keyboard, and holds for any
//! user and any environment. There's no lock file: one would have to be found by every program in the
//! same place, whatever their environment, and it would add nothing to locking the device itself.
//!
//! This isn't the app's single-instance lock, which keeps a second copy of the app from starting.
//! A dry run and a demo open nothing, so they don't come here.

use std::time::Duration;

use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};

/// How long a command waits for the lock. The app takes it for a moment each time it looks for a
/// keyboard, and a command that comes just then shouldn't be told the keyboard is busy.
pub const COMMAND_WAIT: Duration = Duration::from_secs(1);

#[cfg(windows)]
pub use imp::{DeviceLock, Place};

/// Opens the keyboard, with the lock held from before that until the device closes.
#[cfg(windows)]
pub fn open_locked<T: Transport>(
    wait: Duration,
    open: impl FnMut() -> Result<Device<T>, Error>,
) -> Result<Device<T>, Error> {
    open_locked_at(&Place::system()?, wait, open)
}

/// [`open_locked`], with the lock somewhere else.
#[cfg(windows)]
pub fn open_locked_at<T: Transport>(
    place: &Place,
    wait: Duration,
    mut open: impl FnMut() -> Result<Device<T>, Error>,
) -> Result<Device<T>, Error> {
    let lock = DeviceLock::take(place, wait)?;
    let mut device = open()?;
    device.keep_until_closed(Box::new(lock));
    Ok(device)
}

/// Opens the keyboard, which takes the lock as part of that. If someone else has it this tries again
/// for up to `wait`.
#[cfg(not(windows))]
pub fn open_locked<T: Transport>(
    wait: Duration,
    mut open: impl FnMut() -> Result<Device<T>, Error>,
) -> Result<Device<T>, Error> {
    const LOOK_EVERY: Duration = Duration::from_millis(20);

    let deadline = std::time::Instant::now() + wait;
    loop {
        match open() {
            Err(Error::DeviceBusy) if std::time::Instant::now() < deadline => {
                std::thread::sleep(LOOK_EVERY);
            }
            other => return other,
        }
    }
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
            Place(format!(
                "Local\\Unkludge-Test-{}-{name}",
                std::process::id()
            ))
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::Instant;
    use unkludge_protocol::model;
    use unkludge_protocol::transport::fake::FakeTransport;

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

    /// A handle of its own on the mutex, so that it goes on existing when its holder dies. That's what
    /// makes it abandoned: with no handles left there would be no mutex at all, and a new one is free.
    struct KeepsItExisting(windows_sys::Win32::Foundation::HANDLE);

    impl KeepsItExisting {
        fn new(place: &Place) -> KeepsItExisting {
            use std::os::windows::ffi::OsStrExt;
            let name: Vec<u16> = std::ffi::OsStr::new(&place.describe())
                .encode_wide()
                .chain([0])
                .collect();
            // SAFETY: the name is NUL-terminated, null attributes mean the default ones, and this
            // doesn't ask to hold it.
            let handle = unsafe {
                windows_sys::Win32::System::Threading::CreateMutexW(
                    std::ptr::null(),
                    0,
                    name.as_ptr(),
                )
            };
            assert!(!handle.is_null());
            KeepsItExisting(handle)
        }
    }

    impl Drop for KeepsItExisting {
        fn drop(&mut self) {
            // SAFETY: the handle is this one's own.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }

    #[test]
    fn a_process_that_dies_holding_the_lock_doesnt_keep_it() {
        let place = Place::scratch("killed");
        let _keeps_it_existing = KeepsItExisting::new(&place);
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
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use unkludge_protocol::model;
    use unkludge_protocol::transport::fake::FakeTransport;

    fn device() -> Result<Device<FakeTransport>, Error> {
        Device::new(FakeTransport::new(), None, model::find(0x0265).unwrap())
    }

    #[test]
    fn a_keyboard_someone_had_for_a_moment_opens_on_another_try() {
        let mut tries = 0;
        let opened = open_locked(Duration::from_secs(20), || {
            tries += 1;
            if tries < 3 {
                Err(Error::DeviceBusy)
            } else {
                device()
            }
        });
        assert!(opened.is_ok());
        assert_eq!(tries, 3);
    }

    #[test]
    fn a_keyboard_someone_keeps_is_busy_when_the_wait_is_over() {
        let mut tries = 0;
        let opened: Result<Device<FakeTransport>, Error> = open_locked(Duration::ZERO, || {
            tries += 1;
            Err(Error::DeviceBusy)
        });
        assert_eq!(opened.err(), Some(Error::DeviceBusy));
        assert_eq!(tries, 1);
    }

    #[test]
    fn other_failures_arent_tried_again() {
        let mut tries = 0;
        let opened: Result<Device<FakeTransport>, Error> =
            open_locked(Duration::from_secs(20), || {
                tries += 1;
                Err(Error::NotFound)
            });
        assert_eq!(opened.err(), Some(Error::NotFound));
        assert_eq!(tries, 1);
    }
}
