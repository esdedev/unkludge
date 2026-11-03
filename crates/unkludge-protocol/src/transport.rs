//! The link to the keyboard, behind a trait so the protocol can run without hardware.

use std::ffi::CStr;
use std::time::Duration;

use hidapi::{HidApi, HidDevice, HidError};

use crate::Error;

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
}

/// A collection of a real keyboard. Only `Device::open` can make one, and the device keeps it to
/// itself.
pub struct HidTransport {
    device: HidDevice,
}

impl HidTransport {
    pub(crate) fn open(api: &HidApi, path: &CStr) -> Result<Self, Error> {
        let device = api.open_path(path).map_err(hid_error)?;
        Ok(HidTransport { device })
    }
}

pub(crate) fn hid_error(error: HidError) -> Error {
    Error::Hid(error.to_string())
}

impl Transport for HidTransport {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        self.device.send_feature_report(report).map_err(hid_error)
    }

    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        self.device.get_feature_report(buf).map_err(hid_error)
    }

    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error> {
        self.device.read_timeout(buf, ms).map_err(hid_error)
    }
}
