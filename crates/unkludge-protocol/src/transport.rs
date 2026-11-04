//! The link to the keyboard, behind a trait so the protocol can run without hardware.

use std::ffi::CStr;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(not(windows))]
use hidapi::MAX_REPORT_DESCRIPTOR_SIZE;
use hidapi::{HidApi, HidDevice, HidError};

use crate::Error;
use crate::device::descriptor;
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

    /// How long a feature report with this ID is, counting the ID, for checking a collection before
    /// sending it anything. `None` if there's no such report, as far as the system says.
    fn feature_report_len(&self, report_id: u8) -> Result<Option<usize>, Error> {
        Ok(descriptor::feature_report_len(
            &self.report_descriptor()?,
            report_id,
        ))
    }

    /// The collection's HID report descriptor, which [`Transport::feature_report_len`] reads unless it
    /// has a better way.
    fn report_descriptor(&self) -> Result<Vec<u8>, Error> {
        Err(Error::Hid("no report descriptor".to_string()))
    }
}

/// A collection of a real keyboard. Only `Device::open` can make one, and the device keeps it to
/// itself.
pub struct HidTransport {
    device: HidDevice,
    /// For asking the system about the collection, which goes by this.
    #[cfg(windows)]
    path: std::ffi::CString,
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
            #[cfg(windows)]
            path: path.to_owned(),
            #[cfg(target_os = "linux")]
            _node_lock,
        })
    }
}

/// The feature report length the system has for this collection, counting the report ID. This only
/// asks about the collection: it's opened with no access to read or write, so nothing can reach it.
#[cfg(windows)]
fn feature_report_byte_length(path: &CStr) -> Result<usize, Error> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let last_error =
        |what: &str| Error::Hid(format!("{what}: {}", std::io::Error::last_os_error()));
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
    // SAFETY: the path is NUL-terminated and outlives the call, null means no security attributes and
    // no template, and each thing got here is checked before use and given back once. The structure
    // is plain numbers, valid when zeroed, and is written by the call.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("can't look at the collection"));
        }
        let mut preparsed = 0;
        let got = HidD_GetPreparsedData(handle, &mut preparsed);
        let error = last_error("can't get the collection's details");
        CloseHandle(handle);
        if !got {
            return Err(error);
        }
        let mut caps: HIDP_CAPS = std::mem::zeroed();
        let status = HidP_GetCaps(preparsed, &mut caps);
        HidD_FreePreparsedData(preparsed);
        if status != HIDP_STATUS_SUCCESS {
            return Err(Error::Hid(format!(
                "can't read the collection's details: status {status:#x}"
            )));
        }
        Ok(usize::from(caps.FeatureReportByteLength))
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

    /// On Windows this is the collection's own number, which is what the official app goes by and what
    /// hidapi cuts reports to. A descriptor there is only hidapi's reconstruction of one, which can
    /// differ from it by the padding at the end of a report. The number is for the longest feature
    /// report in the collection, whichever its ID, as that's all the system gives.
    #[cfg(windows)]
    fn feature_report_len(&self, _report_id: u8) -> Result<Option<usize>, Error> {
        feature_report_byte_length(&self.path).map(Some)
    }

    #[cfg(not(windows))]
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn a_collection_that_isnt_there_says_so() {
        let path = CString::new(r"\\?\hid#vid_0000&pid_0000#nothing#{4d1e55b2}").unwrap();
        let Err(Error::Hid(message)) = feature_report_byte_length(&path) else {
            panic!("should have failed");
        };
        assert!(message.contains("can't look at"), "{message}");
    }
}
