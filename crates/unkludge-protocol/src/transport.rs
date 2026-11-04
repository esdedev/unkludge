//! The link to the keyboard, behind a trait so the protocol can run without hardware.

use std::ffi::CStr;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::time::{Duration, Instant};

use hidapi::{HidApi, HidDevice, HidError, MAX_REPORT_DESCRIPTOR_SIZE};

use crate::Error;
#[cfg(target_os = "linux")]
use crate::device::node_lock::NodeLock;

#[cfg(any(test, feature = "fake"))]
pub mod fake;

/// One HID collection of a keyboard.
pub trait Transport {
    /// Sends a feature report. The first byte is the report ID.
    fn send_feature(&self, report: &[u8]) -> Result<(), Error>;

    /// Fetches a feature report into `buf`, whose first byte holds the report ID wanted. Returns
    /// the number of bytes written.
    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error>;

    /// Reads one input report, waiting up to `ms` milliseconds. Returns the number of bytes
    /// written, or 0 if nothing arrived.
    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error>;

    /// Pauses between packets. All protocol pacing goes through this, so a transport with no
    /// hardware behind it can skip the wait.
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    /// The time, for timing the wait for an ack. A transport with no hardware behind it can keep
    /// its own clock, so that waits take no real time.
    fn now(&self) -> Instant {
        Instant::now()
    }

    /// The collection's HID report descriptor. Only needed for checking a possible second
    /// collection, so transports that are never paired can leave this as it is.
    fn report_descriptor(&self) -> Result<Vec<u8>, Error> {
        Err(Error::Hid("no report descriptor".to_string()))
    }
}

/// A collection of a real keyboard. Only `Device::open` can make one, and the device keeps it to
/// itself.
pub struct HidTransport {
    device: HidDevice,
    /// Before anything else, and let go of after the device closes, as fields go in this order.
    #[cfg(target_os = "linux")]
    _node_lock: NodeLock,
}

impl HidTransport {
    pub(crate) fn open(api: &HidApi, path: &CStr) -> Result<Self, Error> {
        #[cfg(target_os = "linux")]
        let _node_lock = NodeLock::take(Path::new(&*path.to_string_lossy()))?;
        let device = api.open_path(path).map_err(hid_error)?;
        Ok(HidTransport {
            device,
            #[cfg(target_os = "linux")]
            _node_lock,
        })
    }
}

pub(crate) fn hid_error(error: HidError) -> Error {
    Error::Hid(error.to_string())
}

impl Transport for HidTransport {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        self.device.send_feature_report(report).map_err(hid_error)
    }

    /// On Windows this can block, as the system call behind it has no timeout. If a keyboard stops
    /// answering here the calling thread waits with it.
    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        self.device.get_feature_report(buf).map_err(hid_error)
    }

    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error> {
        self.device.read_timeout(buf, ms).map_err(hid_error)
    }

    fn report_descriptor(&self) -> Result<Vec<u8>, Error> {
        let mut descriptor = vec![0; MAX_REPORT_DESCRIPTOR_SIZE];
        let len = self
            .device
            .get_report_descriptor(&mut descriptor)
            .map_err(hid_error)?;
        descriptor.truncate(len);
        Ok(descriptor)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn a_keyboard_someone_else_has_locked_isnt_even_opened() {
        let node = std::env::temp_dir().join(format!("unkludge-transport-{}", std::process::id()));
        std::fs::write(&node, "").unwrap();
        let lock = NodeLock::take(&node).unwrap();
        // This asks the system nothing about keyboards. It would fail either way, as a file isn't a
        // keyboard, but only the lock says it like this.
        let api = HidApi::new_without_enumerate().unwrap();
        let path = CString::new(node.to_str().unwrap()).unwrap();
        assert_eq!(
            HidTransport::open(&api, &path).err(),
            Some(Error::DeviceBusy)
        );
        drop(lock);
        std::fs::remove_file(&node).unwrap();
    }
}
