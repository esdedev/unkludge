//! One Unkludge program at a time on a keyboard, on Linux: `flock` on the keyboard's own hidraw node.
//! It's the device itself that's locked, so this holds whatever each program's environment says and
//! whichever user is running it, as long as they can open the node at all. The system lets go when the
//! process ends, however it ends.
//!
//! It comes before hidapi opens the node, and so before anything could be sent.

use std::fs::{File, TryLockError};
use std::path::Path;

use crate::Error;

/// Held until dropped.
#[derive(Debug)]
pub(crate) struct NodeLock {
    node: File,
}

impl NodeLock {
    /// [`Error::DeviceBusy`] if someone else has it. Opening the node only for this sends nothing.
    pub(crate) fn take(node: &Path) -> Result<NodeLock, Error> {
        let file =
            File::open(node).map_err(|error| Error::Hid(format!("{}: {error}", node.display())))?;
        match file.try_lock() {
            Ok(()) => Ok(NodeLock { node: file }),
            Err(TryLockError::WouldBlock) => Err(Error::DeviceBusy),
            Err(TryLockError::Error(error)) => {
                Err(Error::LockFailed(format!("{}: {error}", node.display())))
            }
        }
    }
}

impl Drop for NodeLock {
    /// Says so rather than leaving it to the file closing. A program being started from this one has a
    /// copy of the file for a moment, and with that the lock would last until it's done.
    fn drop(&mut self) {
        let _ = self.node.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const NODE_VARIABLE: &str = "UNKLUDGE_TEST_NODE";
    const HELD: &str = "the helper has the node";

    /// A file stands in for the node, as `flock` makes no difference between them.
    fn node(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("unkludge-node-{}-{name}", std::process::id()));
        std::fs::write(&path, "").unwrap();
        path
    }

    #[test]
    fn a_second_taker_is_told_busy_until_the_first_lets_go() {
        let node = node("second");
        let lock = NodeLock::take(&node).unwrap();
        assert_eq!(NodeLock::take(&node).err(), Some(Error::DeviceBusy));
        drop(lock);
        assert!(NodeLock::take(&node).is_ok());
        std::fs::remove_file(&node).unwrap();
    }

    #[test]
    fn letting_go_doesnt_wait_for_a_copy_of_the_file_to_close() {
        let node = node("copy");
        let lock = NodeLock::take(&node).unwrap();
        // As a program being started from this one has for a moment.
        let copy = lock.node.try_clone().unwrap();
        drop(lock);
        assert!(NodeLock::take(&node).is_ok());
        drop(copy);
        std::fs::remove_file(&node).unwrap();
    }

    #[test]
    fn a_node_that_cant_be_opened_says_what_it_is() {
        let missing = Path::new("/nonexistent/hidraw9");
        let Err(Error::Hid(message)) = NodeLock::take(missing) else {
            panic!("should have failed");
        };
        assert!(message.contains("/nonexistent/hidraw9"), "{message}");
    }

    /// Holds the lock until it's killed, in a process of its own, as what's being tested is what the
    /// system does when a process dies.
    #[test]
    #[ignore = "started by the test below, in a process of its own"]
    fn a_process_that_holds_the_node() {
        if let Some(node) = std::env::var_os(NODE_VARIABLE) {
            let _lock = NodeLock::take(Path::new(&node)).unwrap();
            println!("{HELD}");
            std::thread::sleep(Duration::from_secs(120));
        }
    }

    #[test]
    fn another_process_is_kept_out_until_it_dies_however_it_dies() {
        let node = node("killed");
        let mut helper = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "device::node_lock::tests::a_process_that_holds_the_node",
                "--ignored",
                "--nocapture",
            ])
            .env(NODE_VARIABLE, &node)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(helper.stdout.take().unwrap()).lines();
        assert!(
            lines.any(|line| line.is_ok_and(|line| line.contains(HELD))),
            "the helper never got the lock"
        );
        assert_eq!(NodeLock::take(&node).err(), Some(Error::DeviceBusy));
        helper.kill().unwrap();
        helper.wait().unwrap();
        let started = Instant::now();
        while NodeLock::take(&node).is_err() {
            assert!(started.elapsed() < Duration::from_secs(20), "still locked");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::remove_file(&node).unwrap();
    }
}
