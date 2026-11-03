//! Finding and opening a real keyboard.
//!
//! Only two kinds of HID collection are ever opened: usage page `0xFF02` usage 2 for the keyboard,
//! and usage page `0xFF03` usage 2 for the second collection of `kind = 2` models. Everything else
//! stays closed, `0xFF00` collections above all.

use std::ffi::{CStr, CString};

use hidapi::HidApi;

use super::{Device, fetch_block, request_block};
use crate::Error;
use crate::model::{self, Model, VID};
use crate::packet::{self, cmd};
use crate::transport::{HidTransport, Transport, hid_error};

/// Usage page and usage of the collection that carries the protocol.
const MAIN_USAGE: (u16, u16) = (0xFF02, 2);
/// Usage page and usage of the second collection on `kind = 2` models.
const SCREEN_USAGE: (u16, u16) = (0xFF03, 2);
/// The interface that holds the main collection, for when the system can't tell usages.
const FALLBACK_INTERFACE: i32 = 1;
/// Length of the identity that pairs a second collection with its keyboard.
const IDENTITY_LEN: usize = 6;

/// One HID collection as the system lists it. Without usage information, as can happen on Linux,
/// `usage_page` and `usage` are 0.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HidEntry {
    vid: u16,
    pid: u16,
    usage_page: u16,
    usage: u16,
    interface: i32,
    path: CString,
}

impl HidEntry {
    fn usages(&self) -> (u16, u16) {
        (self.usage_page, self.usage)
    }
}

/// What to open, as indexes into the list given to [`select`].
#[derive(Debug, PartialEq, Eq)]
struct Selection {
    main: usize,
    model: &'static Model,
    /// Collections that may be the keyboard's second one, best first. Reading the identity from
    /// each one decides. Always empty on models without a second collection.
    screen_candidates: Vec<usize>,
}

/// Picks the keyboard to open from the system's list, or the one with product ID `pid` if given.
fn select(entries: &[HidEntry], pid: Option<u16>) -> Result<Selection, Error> {
    if let Some(pid) = pid
        && !model::find(pid).is_some_and(Model::is_supported)
    {
        return Err(Error::Unsupported(pid));
    }

    let keyboard = |e: &HidEntry| {
        e.vid == VID
            && pid.is_none_or(|pid| pid == e.pid)
            && model::find(e.pid).is_some_and(Model::is_supported)
    };
    let position_of =
        |wanted: fn(&HidEntry) -> bool| entries.iter().position(|e| keyboard(e) && wanted(e));
    let main = position_of(|e| e.usages() == MAIN_USAGE)
        .or_else(|| position_of(|e| e.usages() == (0, 0) && e.interface == FALLBACK_INTERFACE))
        .ok_or(Error::NotFound)?;

    let model = model::find(entries[main].pid).ok_or(Error::NotFound)?;
    let has_screen_collection = model.screen.is_some_and(|s| s.kind == 2);
    let screen_candidates = if has_screen_collection {
        screen_candidates(entries, &entries[main])
    } else {
        Vec::new()
    };

    Ok(Selection {
        main,
        model,
        screen_candidates,
    })
}

/// The second collection's product ID isn't known, so any collection of the right usage and vendor
/// could be it. Where the paths show which of them are on the keyboard's USB device, only those.
fn screen_candidates(entries: &[HidEntry], main: &HidEntry) -> Vec<usize> {
    let candidates: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].vid == VID && entries[i].usages() == SCREEN_USAGE)
        .collect();

    let Some(prefix) = usb_prefix(&main.path) else {
        return candidates;
    };
    let same_device: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&i| usb_prefix(&entries[i].path).as_ref() == Some(&prefix))
        .collect();
    if same_device.is_empty() {
        candidates
    } else {
        same_device
    }
}

/// The part of a Windows device path that names the USB device, before the interface and collection
/// numbers. Other systems' paths don't have one.
fn usb_prefix(path: &CStr) -> Option<String> {
    let path = path.to_string_lossy().to_lowercase();
    let end = path.find("&mi_").or_else(|| path.find("&col"))?;
    Some(path[..end].to_string())
}

/// The keyboard's identity. A read only.
fn read_identity<T: Transport>(transport: &T) -> Result<Vec<u8>, Error> {
    let request = packet::read_request(cmd::PAIR_ID_READ, 1, IDENTITY_LEN);
    request_block(transport, &request)?;
    fetch_block(transport, cmd::PAIR_ID_READ, IDENTITY_LEN)
}

/// Finds the keyboard's second collection: the first candidate reporting the same identity.
fn pair_screen<T: Transport>(
    main: &T,
    candidates: impl IntoIterator<Item = T>,
) -> Result<T, Error> {
    let identity = read_identity(main)?;
    candidates
        .into_iter()
        .find(|candidate| read_identity(candidate).is_ok_and(|other| other == identity))
        .ok_or(Error::NotFound)
}

impl Device<HidTransport> {
    /// Opens the connected keyboard, or the one with product ID `pid` if given.
    pub fn open(pid: Option<u16>) -> Result<Self, Error> {
        let api = HidApi::new().map_err(hid_error)?;
        let entries: Vec<HidEntry> = api
            .device_list()
            .filter(|info| info.vendor_id() == VID)
            .map(|info| HidEntry {
                vid: info.vendor_id(),
                pid: info.product_id(),
                usage_page: info.usage_page(),
                usage: info.usage(),
                interface: info.interface_number(),
                path: info.path().to_owned(),
            })
            .collect();

        let selection = select(&entries, pid)?;
        let main = HidTransport::open(&api, &entries[selection.main].path)?;
        let screen = if selection.model.screen.is_some_and(|s| s.kind == 2) {
            let candidates = selection
                .screen_candidates
                .iter()
                .filter_map(|&i| HidTransport::open(&api, &entries[i].path).ok());
            Some(pair_screen(&main, candidates)?)
        } else {
            None
        };
        Device::new(main, screen, selection.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::fake::FakeTransport;

    const S85_ES: u16 = 0x0265;
    const N99: u16 = 0x01B8;
    /// In the table, but on the old protocol.
    const V1_MODEL: u16 = 0x002A;
    /// In the table, on the right protocol, but with no screen.
    const NO_SCREEN_MODEL: u16 = 0x0102;
    /// Not in the table.
    const UNKNOWN: u16 = 0xFFFE;

    fn entry(vid: u16, pid: u16, page: u16, usage: u16, interface: i32, path: &str) -> HidEntry {
        HidEntry {
            vid,
            pid,
            usage_page: page,
            usage,
            interface,
            path: CString::new(path).unwrap(),
        }
    }

    fn windows_path(pid: u16, mi: u8, col: u8) -> String {
        format!(
            r"\\?\hid#vid_258a&pid_{pid:04x}&mi_{mi:02}&col{col:02}#7&2a{mi}&0&000{col}#{{4d1e55b2}}"
        )
    }

    /// What Windows lists for one keyboard: one entry per collection.
    fn windows_list(pid: u16) -> Vec<HidEntry> {
        vec![
            entry(VID, pid, 0x0001, 0x0006, 0, &windows_path(pid, 0, 1)),
            entry(VID, pid, 0x000C, 0x0001, 1, &windows_path(pid, 1, 1)),
            entry(VID, pid, 0xFF00, 0x0001, 1, &windows_path(pid, 1, 2)),
            entry(VID, pid, 0xFF02, 0x0002, 1, &windows_path(pid, 1, 3)),
            entry(VID, pid, 0x0001, 0x0002, 2, &windows_path(pid, 2, 1)),
        ]
    }

    /// What Linux lists when it can't tell usages: one entry per interface.
    fn linux_list(pid: u16) -> Vec<HidEntry> {
        vec![
            entry(VID, pid, 0, 0, 0, "/dev/hidraw2"),
            entry(VID, pid, 0, 0, 1, "/dev/hidraw3"),
            entry(VID, pid, 0, 0, 2, "/dev/hidraw4"),
        ]
    }

    fn picked(entries: &[HidEntry], pid: Option<u16>) -> &HidEntry {
        &entries[select(entries, pid).unwrap().main]
    }

    #[test]
    fn picks_the_vendor_collection_from_a_windows_list() {
        let list = windows_list(S85_ES);
        let selection = select(&list, None).unwrap();
        assert_eq!(selection.main, 3);
        assert_eq!(selection.model.pid, S85_ES);
        assert!(selection.screen_candidates.is_empty());
    }

    #[test]
    fn picks_interface_1_from_a_linux_list_without_usages() {
        let list = linux_list(S85_ES);
        assert_eq!(select(&list, None).unwrap().main, 1);
    }

    #[test]
    fn uses_usages_on_linux_when_they_are_there() {
        let list = vec![
            entry(VID, S85_ES, 0x0001, 0x0006, 0, "/dev/hidraw2"),
            entry(VID, S85_ES, 0xFF00, 0x0001, 1, "/dev/hidraw3"),
            entry(VID, S85_ES, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
        ];
        assert_eq!(select(&list, None).unwrap().main, 2);
    }

    #[test]
    fn a_known_usage_wins_over_an_entry_without_usages() {
        let list = vec![
            entry(VID, S85_ES, 0, 0, 1, "/dev/hidraw3"),
            entry(VID, S85_ES, 0xFF02, 0x0002, 1, "/dev/hidraw4"),
        ];
        assert_eq!(select(&list, None).unwrap().main, 1);
    }

    #[test]
    fn ignores_another_vendors_device() {
        let list = vec![
            entry(0x1234, S85_ES, 0xFF02, 0x0002, 1, "a"),
            entry(0x1234, S85_ES, 0, 0, 1, "b"),
        ];
        assert_eq!(select(&list, None).err(), Some(Error::NotFound));
        assert_eq!(select(&list, Some(S85_ES)).err(), Some(Error::NotFound));
    }

    #[test]
    fn skips_another_vendors_device_to_find_the_keyboard() {
        let mut list = vec![entry(0x1234, S85_ES, 0xFF02, 0x0002, 1, "other")];
        list.extend(windows_list(S85_ES));
        assert_eq!(picked(&list, None).usage_page, 0xFF02);
        assert_eq!(picked(&list, None).vid, VID);
    }

    #[test]
    fn unsupported_pids_are_not_picked() {
        for pid in [V1_MODEL, NO_SCREEN_MODEL, UNKNOWN] {
            let list = windows_list(pid);
            assert_eq!(
                select(&list, None).err(),
                Some(Error::NotFound),
                "{pid:04X}"
            );
            let list = linux_list(pid);
            assert_eq!(
                select(&list, None).err(),
                Some(Error::NotFound),
                "{pid:04X}"
            );
        }
    }

    #[test]
    fn asking_for_an_unsupported_pid_says_so() {
        for pid in [V1_MODEL, NO_SCREEN_MODEL, UNKNOWN] {
            let list = windows_list(pid);
            assert_eq!(
                select(&list, Some(pid)).err(),
                Some(Error::Unsupported(pid))
            );
        }
    }

    #[test]
    fn asking_for_a_supported_pid_that_isnt_connected_is_not_found() {
        let list = windows_list(0x01FA);
        assert_eq!(select(&list, Some(S85_ES)).err(), Some(Error::NotFound));
    }

    #[test]
    fn asking_for_a_pid_picks_that_keyboard() {
        let mut list = windows_list(0x01FA);
        list.extend(windows_list(S85_ES));
        assert_eq!(picked(&list, None).pid, 0x01FA);
        assert_eq!(picked(&list, Some(S85_ES)).pid, S85_ES);
    }

    #[test]
    fn nothing_listed_is_not_found() {
        assert_eq!(select(&[], None).err(), Some(Error::NotFound));
    }

    #[test]
    fn a_0xff00_collection_of_a_supported_pid_is_never_picked() {
        for model in crate::model::supported() {
            for usage in [0, 1, 2] {
                for interface in [-1, 0, 1, 2] {
                    let path = windows_path(model.pid, 1, 2);
                    let list = [entry(VID, model.pid, 0xFF00, usage, interface, &path)];
                    assert_eq!(select(&list, None).err(), Some(Error::NotFound));
                    assert_eq!(select(&list, Some(model.pid)).err(), Some(Error::NotFound));
                }
            }
        }
    }

    #[test]
    fn other_collections_are_never_picked() {
        let others = [
            // The second vendor collection alone isn't a keyboard to talk to.
            (0xFF03, 2, 1),
            // Right page, wrong usage.
            (0xFF02, 1, 1),
            (0xFF02, 3, 1),
            // Ordinary keyboard and media collections.
            (0x0001, 6, 1),
            (0x000C, 1, 1),
            // No usages, but the wrong interface.
            (0, 0, 0),
            (0, 0, 2),
            (0, 0, -1),
            // A usage without a page isn't "no usages".
            (0, 2, 1),
        ];
        for (page, usage, interface) in others {
            let list = [entry(VID, S85_ES, page, usage, interface, "x")];
            assert_eq!(
                select(&list, None).err(),
                Some(Error::NotFound),
                "{page:04X} {usage} {interface}"
            );
        }
    }

    // Second collections.

    fn n99_list() -> Vec<HidEntry> {
        let mut list = windows_list(N99);
        list.push(entry(VID, N99, 0xFF03, 0x0002, 3, &windows_path(N99, 3, 1)));
        list
    }

    #[test]
    fn a_model_with_a_screen_collection_gets_candidates() {
        let selection = select(&n99_list(), None).unwrap();
        assert_eq!(selection.main, 3);
        assert_eq!(selection.screen_candidates, [5]);
    }

    #[test]
    fn a_model_without_a_screen_collection_gets_none() {
        let mut list = windows_list(S85_ES);
        list.push(entry(VID, S85_ES, 0xFF03, 0x0002, 3, "x"));
        assert!(select(&list, None).unwrap().screen_candidates.is_empty());
    }

    #[test]
    fn candidates_must_be_vendor_258a_and_exact_usage() {
        let mut list = n99_list();
        list.push(entry(0x1234, N99, 0xFF03, 0x0002, 3, "other vendor"));
        list.push(entry(VID, N99, 0xFF03, 0x0001, 3, "wrong usage"));
        list.push(entry(VID, N99, 0xFF00, 0x0002, 3, "wrong page"));
        assert_eq!(select(&list, None).unwrap().screen_candidates, [5]);
    }

    #[test]
    fn candidates_on_the_same_usb_device_win_where_the_path_can_tell() {
        let mut list = vec![entry(
            VID,
            0x01FA,
            0xFF03,
            0x0002,
            3,
            &windows_path(0x01FA, 3, 1),
        )];
        list.extend(n99_list());
        // Index 0 is on another USB device, index 6 on this one.
        let selection = select(&list, Some(N99)).unwrap();
        assert_eq!(selection.screen_candidates, [6]);
    }

    #[test]
    fn all_vendor_candidates_stay_when_the_path_cant_tell() {
        // Linux paths say nothing about the USB device.
        let list = vec![
            entry(VID, N99, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "/dev/hidraw4"),
            entry(VID, 0x01FA, 0xFF03, 0x0002, 2, "/dev/hidraw7"),
        ];
        assert_eq!(select(&list, None).unwrap().screen_candidates, [1, 2]);
    }

    #[test]
    fn all_vendor_candidates_stay_when_none_shares_the_path_prefix() {
        let mut list = windows_list(N99);
        list.push(entry(
            VID,
            0x0150,
            0xFF03,
            0x0002,
            0,
            &windows_path(0x0150, 0, 1),
        ));
        assert_eq!(select(&list, None).unwrap().screen_candidates, [5]);
    }

    #[test]
    fn usb_prefix_reads_windows_paths_only() {
        let prefix = usb_prefix(&CString::new(windows_path(N99, 1, 3)).unwrap());
        assert_eq!(prefix.as_deref(), Some(r"\\?\hid#vid_258a&pid_01b8"));
        let upper = CString::new(r"\\?\HID#VID_258A&PID_01B8&MI_01#7&2a&0&0000#{4d1e55b2}");
        assert_eq!(
            usb_prefix(&upper.unwrap()).as_deref(),
            Some(r"\\?\hid#vid_258a&pid_01b8")
        );
        assert_eq!(usb_prefix(&CString::new("/dev/hidraw3").unwrap()), None);
    }

    #[test]
    fn opening_links_on_this_platform() {
        // Only takes the function's address: nothing here touches a device.
        let open: fn(Option<u16>) -> Result<Device, Error> = Device::open;
        assert!(!std::hint::black_box(open as usize == 0));
    }

    // Pairing.

    /// A collection that answers the identity read with `id`.
    fn answers(id: &[u8]) -> FakeTransport {
        let fake = FakeTransport::new();
        let mut reply = vec![0x09, 0x82, 0x01, 0x00, 0x01, 0x00, 0x06, 0x00];
        reply.extend(id);
        fake.queue_feature_reply(&reply);
        fake
    }

    #[test]
    fn pairing_reads_6_bytes_with_arg_1_from_both_sides() {
        let main = answers(&[1, 2, 3, 4, 5, 6]);
        let screen = answers(&[1, 2, 3, 4, 5, 6]);
        pair_screen(&main, [screen.clone()]).unwrap();

        for fake in [&main, &screen] {
            let sent = fake.sent();
            assert_eq!(sent.len(), 1);
            assert_eq!(
                sent[0][..8],
                [0x09, 0x82, 0x01, 0x00, 0x01, 0x00, 0x06, 0x00]
            );
            fake.assert_allowed_commands_only();
        }
    }

    #[test]
    fn pairing_takes_the_first_match_and_stops_looking() {
        let main = answers(&[1, 2, 3, 4, 5, 6]);
        let other = answers(&[9, 9, 9, 9, 9, 9]);
        let right = answers(&[1, 2, 3, 4, 5, 6]);
        let unused = answers(&[1, 2, 3, 4, 5, 6]);
        let found = pair_screen(&main, [other.clone(), right.clone(), unused.clone()]).unwrap();

        assert_eq!(found.sent().len(), right.sent().len());
        assert_eq!(other.sent().len(), 1);
        assert_eq!(right.sent().len(), 1);
        assert!(unused.sent().is_empty());
    }

    #[test]
    fn pairing_without_a_match_is_not_found() {
        let main = answers(&[1, 2, 3, 4, 5, 6]);
        let other = answers(&[9, 9, 9, 9, 9, 9]);
        assert_eq!(pair_screen(&main, [other]).err(), Some(Error::NotFound));
        let main = answers(&[1, 2, 3, 4, 5, 6]);
        let none: [FakeTransport; 0] = [];
        assert_eq!(pair_screen(&main, none).err(), Some(Error::NotFound));
    }

    #[test]
    fn pairing_skips_a_collection_that_cant_answer() {
        let main = answers(&[1, 2, 3, 4, 5, 6]);
        // Nothing queued, so its read fails.
        let silent = FakeTransport::new();
        let right = answers(&[1, 2, 3, 4, 5, 6]);
        pair_screen(&main, [silent, right.clone()]).unwrap();
        assert_eq!(right.sent().len(), 1);
    }

    #[test]
    fn pairing_fails_if_the_keyboard_itself_cant_answer() {
        let main = FakeTransport::new();
        let screen = answers(&[1, 2, 3, 4, 5, 6]);
        assert!(matches!(
            pair_screen(&main, [screen.clone()]),
            Err(Error::Hid(_))
        ));
        assert!(screen.sent().is_empty());
    }
}
