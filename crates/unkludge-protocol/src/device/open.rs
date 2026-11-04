//! Finding and opening a real keyboard.
//!
//! Only two kinds of HID collection are ever selected: usage page `0xFF02` usage 2 for the
//! keyboard, and usage page `0xFF03` usage 2 for the second collection of `kind = 2` models. `0xFF00`
//! collections never are. On Windows each collection has its own path, so only the selected one
//! opens. On Linux the hidraw node is the whole interface, so opening it reaches any other collection
//! there too. What protects those is the allow-list of commands.

use std::ffi::{CStr, CString};
#[cfg(target_os = "linux")]
use std::path::Path;

use hidapi::HidApi;

use super::descriptor::feature_report_len;
use super::{Device, fetch_block, request_block};
use crate::Error;
use crate::model::{self, Model, VID};
use crate::packet::{self, REPORT_ID, REPORT_LEN, cmd};
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

/// One HID collection of the vendor's, for telling someone what the system sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidCollection {
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
}

impl From<&HidEntry> for HidCollection {
    fn from(entry: &HidEntry) -> HidCollection {
        HidCollection {
            pid: entry.pid,
            usage_page: entry.usage_page,
            usage: entry.usage,
            interface: entry.interface,
        }
    }
}

/// The collections the system lists for this vendor. This only asks the system: no collection gets
/// opened, and nothing reaches a keyboard.
pub fn list_collections() -> Result<Vec<HidCollection>, Error> {
    let api = HidApi::new().map_err(hid_error)?;
    Ok(entries(&api).iter().map(HidCollection::from).collect())
}

fn entries(api: &HidApi) -> Vec<HidEntry> {
    api.device_list()
        .filter(|info| info.vendor_id() == VID)
        .map(|info| HidEntry {
            vid: info.vendor_id(),
            pid: info.product_id(),
            usage_page: info.usage_page(),
            usage: info.usage(),
            interface: info.interface_number(),
            path: info.path().to_owned(),
        })
        .collect()
}

/// Picks the keyboard to open from the system's list, or the one with product ID `pid` if given.
/// `usage_fallback` allows an entry without usages on the right interface. Only for Linux: on
/// Windows those zeros can also mean that reading a collection's capabilities failed, and then it
/// could be any collection. `device_of` says which device a collection is on: see [`device_of`].
fn select(
    entries: &[HidEntry],
    pid: Option<u16>,
    usage_fallback: bool,
    device_of: &dyn Fn(&CStr) -> Option<String>,
) -> Result<Selection, Error> {
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
        .or_else(|| {
            position_of(|e| e.usages() == (0, 0) && e.interface == FALLBACK_INTERFACE)
                .filter(|_| usage_fallback)
        })
        .ok_or_else(|| match entries.iter().find(|e| keyboard(e)) {
            // It's there, so asking to connect it would only send someone looking for a cable.
            Some(entry) => Error::NoUsableCollection(entry.pid),
            None => Error::NotFound,
        })?;

    let model = model::find(entries[main].pid).ok_or(Error::NotFound)?;
    let screen_candidates = if model.has_second_collection() {
        screen_candidates(entries, &entries[main], device_of)
    } else {
        Vec::new()
    };

    Ok(Selection {
        main,
        model,
        screen_candidates,
    })
}

/// The collections that may be the keyboard's second one: of the right usage, and on the same device
/// as far as [`device_of`] can tell. Each one gets asked for its identity, which goes nowhere near a
/// device that isn't this keyboard. The official app has no such limit and asks any collection of
/// that usage, which is what the identity is for: there it's all that tells them apart. So if a second
/// collection ever turns up on a device of its own, this won't find it, and the keyboard gets refused
/// with a message rather than something else getting asked.
fn screen_candidates(
    entries: &[HidEntry],
    main: &HidEntry,
    device_of: &dyn Fn(&CStr) -> Option<String>,
) -> Vec<usize> {
    let Some(device) = device_of(&main.path) else {
        return Vec::new();
    };
    (0..entries.len())
        .filter(|&i| {
            let entry = &entries[i];
            entry.vid == VID
                && entry.usages() == SCREEN_USAGE
                && entry.path != main.path
                && device_of(&entry.path).as_ref() == Some(&device)
        })
        .collect()
}

/// What tells which device a collection is on, from its path, as far as the system lets on. On
/// Linux that's the USB device itself. On Windows nothing hidapi gives goes further than the vendor
/// and product IDs, so two keyboards of one model look alike, and that's left to the identity read.
fn device_of(path: &CStr) -> Option<String> {
    vid_pid_prefix(path).or_else(|| usb_device_of(path))
}

/// The part of a Windows device path before the interface and collection numbers. It holds only
/// the vendor and product IDs. Other systems' paths don't have one.
fn vid_pid_prefix(path: &CStr) -> Option<String> {
    let path = path.to_string_lossy().to_lowercase();
    let end = path.find("&mi_").or_else(|| path.find("&col"))?;
    Some(path[..end].to_string())
}

#[cfg(target_os = "linux")]
fn usb_device_of(path: &CStr) -> Option<String> {
    usb_device_in_sysfs(Path::new("/sys"), path)
}

#[cfg(not(target_os = "linux"))]
fn usb_device_of(_path: &CStr) -> Option<String> {
    None
}

/// The USB device a hidraw node belongs to: the first folder up from the node's own device, in
/// sysfs, that says what its vendor is. Interfaces of one device, and nothing else, share it.
#[cfg(target_os = "linux")]
fn usb_device_in_sysfs(sysfs: &Path, node: &CStr) -> Option<String> {
    let node = Path::new(node.to_str().ok()?).file_name()?;
    let device = sysfs.join("class/hidraw").join(node).join("device");
    let device = std::fs::canonicalize(device).ok()?;
    let usb_device = device
        .ancestors()
        .find(|folder| folder.join("idVendor").is_file())?;
    Some(usb_device.to_string_lossy().into_owned())
}

/// The keyboard's identity. A read only.
fn read_identity<T: Transport>(transport: &T) -> Result<Vec<u8>, Error> {
    let request = packet::read_request(cmd::PAIR_ID_READ, 1, IDENTITY_LEN);
    request_block(transport, &request)?;
    fetch_block(transport, cmd::PAIR_ID_READ, IDENTITY_LEN)
}

/// Whether a collection's feature reports are the full length. Reports of another length would be
/// cut short without any error on Windows, so this is checked before sending it anything.
fn takes_full_reports<T: Transport>(candidate: &T) -> bool {
    candidate
        .report_descriptor()
        .is_ok_and(|descriptor| feature_report_len(&descriptor, REPORT_ID) == Some(REPORT_LEN))
}

/// Refuses a keyboard's main collection if its feature reports aren't the full length. That covers
/// a descriptor with no report 9 or that makes no sense, and one that can't be read at all. A report
/// of unknown length is the same risk as one known to be wrong: on Windows a config write could be
/// cut short and still say it worked. And what's lost is small, as a system that can't give the
/// descriptor of a device it just opened is rare. So nothing goes out, and the message says which
/// of the two it was.
fn check_main<T: Transport>(main: &T, pid: u16) -> Result<(), Error> {
    if let Err(error) = main.report_descriptor() {
        return Err(Error::Hid(format!(
            "can't read the keyboard's report descriptor, so its report length is unknown: {error}"
        )));
    }
    if takes_full_reports(main) {
        Ok(())
    } else {
        Err(Error::NoUsableCollection(pid))
    }
}

/// Makes a device of what was opened, once it's clear that it's safe to send anything.
fn assemble<T: Transport>(
    main: T,
    candidates: impl IntoIterator<Item = Result<T, Error>>,
    model: &'static Model,
) -> Result<Device<T>, Error> {
    check_main(&main, model.pid)?;
    let screen = if model.has_second_collection() {
        let paired = pair_screen(&main, candidates).map_err(|error| match error {
            // The keyboard is right there: it's its second collection that wasn't found.
            Error::NotFound => Error::NoUsableCollection(model.pid),
            other => other,
        })?;
        Some(paired)
    } else {
        None
    };
    Device::new(main, screen, model)
}

/// Finds the keyboard's second collection: the first candidate with full-length feature reports
/// reporting the same identity. If none is found and some could not be opened, says so, since one
/// of those may be the one.
fn pair_screen<T: Transport>(
    main: &T,
    candidates: impl IntoIterator<Item = Result<T, Error>>,
) -> Result<T, Error> {
    let identity = read_identity(main)?;
    let mut not_opened = None;
    for candidate in candidates {
        match candidate {
            Ok(candidate) => {
                if takes_full_reports(&candidate)
                    && read_identity(&candidate).is_ok_and(|other| other == identity)
                {
                    return Ok(candidate);
                }
            }
            Err(error) => {
                not_opened.get_or_insert(error);
            }
        }
    }
    Err(not_opened.map_or(Error::NotFound, |error| {
        Error::Hid(format!(
            "couldn't open a possible screen collection: {error}"
        ))
    }))
}

impl Device<HidTransport> {
    /// Opens the connected keyboard, or the one with product ID `pid` if given.
    #[cfg_attr(feature = "fake", allow(unreachable_code, unused_variables))]
    pub fn open(pid: Option<u16>) -> Result<Self, Error> {
        // Test builds get `fake` through dev-dependencies, in the binaries they run as well. So if
        // a bug sent a dry run down this path, it still couldn't write to a keyboard plugged in.
        #[cfg(feature = "fake")]
        return Err(Error::NotFound);

        let api = HidApi::new().map_err(hid_error)?;
        let entries = entries(&api);

        let selection = select(&entries, pid, cfg!(target_os = "linux"), &device_of)?;
        let main = HidTransport::open(&api, &entries[selection.main].path)?;
        let candidates = selection
            .screen_candidates
            .iter()
            .map(|&i| HidTransport::open(&api, &entries[i].path));
        assemble(main, candidates, selection.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::fake::{FakeEvent, FakeTransport};

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

    fn select_windows(entries: &[HidEntry], pid: Option<u16>) -> Result<Selection, Error> {
        select(entries, pid, false, &vid_pid_prefix)
    }

    /// Where the nodes numbered up to 5 are on one USB device, and the others on another.
    fn linux_device(path: &CStr) -> Option<String> {
        let number: u32 = path
            .to_str()
            .ok()?
            .strip_prefix("/dev/hidraw")?
            .parse()
            .ok()?;
        Some(if number <= 5 { "one" } else { "other" }.to_string())
    }

    fn select_linux(entries: &[HidEntry], pid: Option<u16>) -> Result<Selection, Error> {
        select(entries, pid, true, &linux_device)
    }

    fn picked(entries: &[HidEntry], pid: Option<u16>) -> &HidEntry {
        &entries[select_windows(entries, pid).unwrap().main]
    }

    #[cfg(feature = "fake")]
    #[test]
    fn opening_is_refused_when_built_with_the_fake() {
        assert_eq!(Device::open(None).err(), Some(Error::NotFound));
        assert_eq!(Device::open(Some(S85_ES)).err(), Some(Error::NotFound));
    }

    #[test]
    fn a_listed_collection_keeps_what_the_system_said() {
        let entry = entry(VID, S85_ES, 0xFF02, 2, 1, "path");
        assert_eq!(
            HidCollection::from(&entry),
            HidCollection {
                pid: S85_ES,
                usage_page: 0xFF02,
                usage: 2,
                interface: 1,
            }
        );
    }

    #[test]
    fn picks_the_vendor_collection_from_a_windows_list() {
        let list = windows_list(S85_ES);
        let selection = select_windows(&list, None).unwrap();
        assert_eq!(selection.main, 3);
        assert_eq!(selection.model.pid, S85_ES);
        assert!(selection.screen_candidates.is_empty());
    }

    #[test]
    fn picks_interface_1_from_a_linux_list_without_usages() {
        let list = linux_list(S85_ES);
        assert_eq!(select_linux(&list, None).unwrap().main, 1);
    }

    #[test]
    fn an_entry_without_usages_is_never_picked_elsewhere() {
        // On Windows, failing to read a collection's capabilities also shows as 0 and 0, and that
        // collection could be any of them.
        let list = linux_list(S85_ES);
        assert_eq!(
            select_windows(&list, None).err(),
            Some(Error::NoUsableCollection(S85_ES))
        );
    }

    #[test]
    fn a_supported_keyboard_with_no_usable_collection_isnt_taken_for_no_keyboard() {
        let collections = vec![
            entry(VID, S85_ES, 0x0001, 0x0006, 0, &windows_path(S85_ES, 0, 1)),
            entry(VID, S85_ES, 0xFF00, 0x0001, 1, &windows_path(S85_ES, 1, 2)),
        ];
        assert_eq!(
            select_windows(&collections, None).err(),
            Some(Error::NoUsableCollection(S85_ES))
        );
        assert_eq!(
            select_windows(&collections, Some(S85_ES)).err(),
            Some(Error::NoUsableCollection(S85_ES))
        );
    }

    #[test]
    fn a_keyboard_with_no_usable_collection_doesnt_hide_one_that_has_one() {
        let mut list = vec![entry(
            VID,
            0x01FA,
            0xFF00,
            1,
            1,
            &windows_path(0x01FA, 1, 2),
        )];
        list.extend(windows_list(S85_ES));
        assert_eq!(picked(&list, None).pid, S85_ES);
    }

    #[test]
    fn a_keyboard_with_no_usable_collection_doesnt_count_when_another_was_asked_for() {
        let list = [entry(
            VID,
            0x01FA,
            0xFF00,
            1,
            1,
            &windows_path(0x01FA, 1, 2),
        )];
        assert_eq!(
            select_windows(&list, Some(S85_ES)).err(),
            Some(Error::NotFound)
        );
    }

    #[test]
    fn uses_usages_on_linux_when_they_are_there() {
        let list = vec![
            entry(VID, S85_ES, 0x0001, 0x0006, 0, "/dev/hidraw2"),
            entry(VID, S85_ES, 0xFF00, 0x0001, 1, "/dev/hidraw3"),
            entry(VID, S85_ES, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
        ];
        assert_eq!(select_linux(&list, None).unwrap().main, 2);
    }

    #[test]
    fn a_known_usage_wins_over_an_entry_without_usages() {
        let list = vec![
            entry(VID, S85_ES, 0, 0, 1, "/dev/hidraw3"),
            entry(VID, S85_ES, 0xFF02, 0x0002, 1, "/dev/hidraw4"),
        ];
        assert_eq!(select_linux(&list, None).unwrap().main, 1);
    }

    #[test]
    fn ignores_another_vendors_device() {
        let list = vec![
            entry(0x1234, S85_ES, 0xFF02, 0x0002, 1, "a"),
            entry(0x1234, S85_ES, 0, 0, 1, "b"),
        ];
        assert_eq!(select_linux(&list, None).err(), Some(Error::NotFound));
        assert_eq!(
            select_linux(&list, Some(S85_ES)).err(),
            Some(Error::NotFound)
        );
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
                select_windows(&list, None).err(),
                Some(Error::NotFound),
                "{pid:04X}"
            );
            let list = linux_list(pid);
            assert_eq!(
                select_linux(&list, None).err(),
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
                select_windows(&list, Some(pid)).err(),
                Some(Error::Unsupported(pid))
            );
        }
    }

    #[test]
    fn asking_for_a_supported_pid_that_isnt_connected_is_not_found() {
        let list = windows_list(0x01FA);
        assert_eq!(
            select_windows(&list, Some(S85_ES)).err(),
            Some(Error::NotFound)
        );
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
        assert_eq!(select_windows(&[], None).err(), Some(Error::NotFound));
    }

    #[test]
    fn a_0xff00_collection_of_a_supported_pid_is_never_picked() {
        for model in crate::model::supported() {
            for usage in [0, 1, 2] {
                for interface in [-1, 0, 1, 2] {
                    let path = windows_path(model.pid, 1, 2);
                    let list = [entry(VID, model.pid, 0xFF00, usage, interface, &path)];
                    let refused = Some(Error::NoUsableCollection(model.pid));
                    assert_eq!(select_windows(&list, None).err(), refused);
                    assert_eq!(select_linux(&list, None).err(), refused);
                    assert_eq!(select_linux(&list, Some(model.pid)).err(), refused);
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
                select_linux(&list, None).err(),
                Some(Error::NoUsableCollection(S85_ES)),
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
        let selection = select_windows(&n99_list(), None).unwrap();
        assert_eq!(selection.main, 3);
        assert_eq!(selection.screen_candidates, [5]);
    }

    #[test]
    fn a_model_without_a_screen_collection_gets_none() {
        let mut list = windows_list(S85_ES);
        list.push(entry(VID, S85_ES, 0xFF03, 0x0002, 3, "x"));
        assert!(
            select_windows(&list, None)
                .unwrap()
                .screen_candidates
                .is_empty()
        );
    }

    #[test]
    fn candidates_must_be_vendor_258a_and_exact_usage() {
        let mut list = n99_list();
        list.push(entry(0x1234, N99, 0xFF03, 0x0002, 3, "other vendor"));
        list.push(entry(VID, N99, 0xFF03, 0x0001, 3, "wrong usage"));
        list.push(entry(VID, N99, 0xFF00, 0x0002, 3, "wrong page"));
        assert_eq!(select_windows(&list, None).unwrap().screen_candidates, [5]);
    }

    #[test]
    fn only_a_candidate_on_the_same_device_counts() {
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
        let selection = select_windows(&list, Some(N99)).unwrap();
        assert_eq!(selection.screen_candidates, [6]);
    }

    #[test]
    fn a_collection_on_another_device_is_never_asked_even_if_it_is_all_there_is() {
        let mut list = windows_list(N99);
        list.push(entry(
            VID,
            0x0150,
            0xFF03,
            0x0002,
            0,
            &windows_path(0x0150, 0, 1),
        ));
        assert!(
            select_windows(&list, None)
                .unwrap()
                .screen_candidates
                .is_empty()
        );
    }

    #[test]
    fn on_linux_the_usb_device_decides_whatever_the_product_id_says() {
        let list = vec![
            entry(VID, N99, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "/dev/hidraw4"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "/dev/hidraw7"),
            entry(VID, 0x01FA, 0xFF03, 0x0002, 2, "/dev/hidraw8"),
        ];
        assert_eq!(select_linux(&list, None).unwrap().screen_candidates, [1]);
    }

    #[test]
    fn nothing_is_asked_where_the_system_cant_say_which_device_a_collection_is_on() {
        let list = vec![
            entry(VID, N99, 0xFF02, 0x0002, 1, "unknown-main"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "unknown-a"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "/dev/hidraw4"),
        ];
        assert!(
            select_linux(&list, None)
                .unwrap()
                .screen_candidates
                .is_empty()
        );
        let list = vec![
            entry(VID, N99, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "unknown-a"),
        ];
        assert!(
            select_linux(&list, None)
                .unwrap()
                .screen_candidates
                .is_empty()
        );
    }

    #[test]
    fn on_linux_only_the_vendor_keeps_another_vendors_collection_out() {
        let list = vec![
            entry(VID, N99, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
            entry(0x1234, N99, 0xFF03, 0x0002, 2, "/dev/hidraw2"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "/dev/hidraw4"),
        ];
        assert_eq!(select_linux(&list, None).unwrap().screen_candidates, [2]);
    }

    #[test]
    fn the_keyboards_own_path_is_never_a_candidate() {
        let list = vec![
            entry(VID, N99, 0xFF02, 0x0002, 1, "/dev/hidraw3"),
            entry(VID, N99, 0xFF03, 0x0002, 1, "/dev/hidraw3"),
            entry(VID, N99, 0xFF03, 0x0002, 2, "/dev/hidraw4"),
        ];
        assert_eq!(select_linux(&list, None).unwrap().screen_candidates, [2]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sysfs_says_which_usb_device_a_node_is_on() {
        use std::os::unix::fs::symlink;

        let sysfs = std::env::temp_dir().join(format!("unkludge-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sysfs);
        let usb = sysfs.join("devices/pci0000:00/usb1/1-2");
        let usb_other = sysfs.join("devices/pci0000:00/usb1/1-3");
        // Nodes 3 and 4 on interfaces of one device, node 7 on another, and node 9 on no USB at all.
        for (device, node) in [
            (usb.join("1-2:1.1/0003:258A:01B8.0005"), "hidraw3"),
            (usb.join("1-2:1.2/0003:258A:01B8.0006"), "hidraw4"),
            (usb_other.join("1-3:1.1/0003:258A:01B8.0007"), "hidraw7"),
            (sysfs.join("devices/virtual/0005:258A:01B8.0008"), "hidraw9"),
        ] {
            std::fs::create_dir_all(&device).unwrap();
            let class = sysfs.join("class/hidraw").join(node);
            std::fs::create_dir_all(&class).unwrap();
            symlink(&device, class.join("device")).unwrap();
        }
        std::fs::write(usb.join("idVendor"), "258a\n").unwrap();
        std::fs::write(usb_other.join("idVendor"), "258a\n").unwrap();

        let device_of = |node: &str| {
            let node = CString::new(format!("/dev/{node}")).unwrap();
            usb_device_in_sysfs(&sysfs, &node)
        };
        assert!(device_of("hidraw3").is_some());
        assert_eq!(device_of("hidraw3"), device_of("hidraw4"));
        assert!(device_of("hidraw7").is_some());
        assert_ne!(device_of("hidraw3"), device_of("hidraw7"));
        assert_eq!(device_of("hidraw9"), None);
        assert_eq!(device_of("hidraw1"), None);
        std::fs::remove_dir_all(&sysfs).unwrap();
    }

    #[test]
    fn the_prefix_comes_from_windows_paths_only() {
        let expected = Some(r"\\?\hid#vid_258a&pid_01b8");
        let with_interface = CString::new(windows_path(N99, 1, 3)).unwrap();
        assert_eq!(vid_pid_prefix(&with_interface).as_deref(), expected);
        let upper =
            CString::new(r"\\?\HID#VID_258A&PID_01B8&MI_01#7&2a&0&0000#{4d1e55b2}").unwrap();
        assert_eq!(vid_pid_prefix(&upper).as_deref(), expected);
        // One interface only, so a collection number and no interface number.
        let no_interface =
            CString::new(r"\\?\hid#vid_258a&pid_01b8&col02#7&2a&0&0001#{4d1e55b2}").unwrap();
        assert_eq!(vid_pid_prefix(&no_interface).as_deref(), expected);
        let linux = CString::new("/dev/hidraw3").unwrap();
        assert_eq!(vid_pid_prefix(&linux), None);
    }

    // Pairing.

    /// A descriptor of one vendor collection whose report 9 has this many feature bytes after the ID.
    fn descriptor(feature_bytes: u16) -> Vec<u8> {
        let [lo, hi] = feature_bytes.to_le_bytes();
        vec![
            0x06, 0x03, 0xFF, // Usage page 0xFF03
            0x09, 0x02, // Usage 2
            0xA1, 0x01, // Collection
            0x85, 0x09, // Report ID 9
            0x75, 0x08, // Report size 8
            0x96, lo, hi, // Report count
            0x09, 0x01, // Usage 1
            0xB1, 0x02, // Feature
            0xC0, // End collection
        ]
    }

    /// A collection that answers the identity read with `id`.
    fn answers(id: &[u8]) -> FakeTransport {
        let fake = FakeTransport::new();
        let mut reply = vec![0x09, 0x82, 0x01, 0x00, 0x01, 0x00, 0x06, 0x00];
        reply.extend(id);
        fake.queue_feature_reply(&reply);
        fake
    }

    /// A possible second collection, with full-length feature reports.
    fn candidate(id: &[u8]) -> FakeTransport {
        let fake = answers(id);
        fake.set_report_descriptor(&descriptor(519));
        fake
    }

    const ID: [u8; 6] = [1, 2, 3, 4, 5, 6];
    const OTHER_ID: [u8; 6] = [9, 9, 9, 9, 9, 9];

    #[test]
    fn pairing_reads_6_bytes_with_arg_1_from_both_sides() {
        let main = answers(&ID);
        let screen = candidate(&ID);
        pair_screen(&main, [Ok(screen.clone())]).unwrap();

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
        let main = answers(&ID);
        let other = candidate(&OTHER_ID);
        let right = candidate(&ID);
        let unused = candidate(&ID);
        let found = pair_screen(
            &main,
            [Ok(other.clone()), Ok(right.clone()), Ok(unused.clone())],
        )
        .unwrap();

        assert_eq!(found.sent().len(), right.sent().len());
        assert_eq!(other.sent().len(), 1);
        assert_eq!(right.sent().len(), 1);
        assert!(unused.sent().is_empty());
    }

    #[test]
    fn pairing_without_a_match_is_not_found() {
        let main = answers(&ID);
        let other = candidate(&OTHER_ID);
        assert_eq!(pair_screen(&main, [Ok(other)]).err(), Some(Error::NotFound));
        let main = answers(&ID);
        let none: [Result<FakeTransport, Error>; 0] = [];
        assert_eq!(pair_screen(&main, none).err(), Some(Error::NotFound));
    }

    #[test]
    fn pairing_skips_a_collection_that_cant_answer() {
        let main = answers(&ID);
        // Nothing queued, so its read fails.
        let silent = FakeTransport::new();
        silent.set_report_descriptor(&descriptor(519));
        let right = candidate(&ID);
        pair_screen(&main, [Ok(silent), Ok(right.clone())]).unwrap();
        assert_eq!(right.sent().len(), 1);
    }

    #[test]
    fn pairing_fails_if_the_keyboard_itself_cant_answer() {
        let main = FakeTransport::new();
        let screen = candidate(&ID);
        assert!(matches!(
            pair_screen(&main, [Ok(screen.clone())]),
            Err(Error::Hid(_))
        ));
        assert!(screen.sent().is_empty());
    }

    #[test]
    fn pairing_checks_the_length_before_sending_anything() {
        let main = answers(&ID);
        let screen = candidate(&ID);
        pair_screen(&main, [Ok(screen.clone())]).unwrap();
        assert_eq!(screen.events()[0], FakeEvent::ReadDescriptor);
    }

    #[test]
    fn pairing_sends_nothing_to_a_collection_of_another_length() {
        for feature_bytes in [64, 518, 520, 1031] {
            let main = answers(&ID);
            let wrong = answers(&ID);
            wrong.set_report_descriptor(&descriptor(feature_bytes));
            let right = candidate(&ID);
            let found = pair_screen(&main, [Ok(wrong.clone()), Ok(right.clone())]).unwrap();
            assert!(wrong.sent().is_empty(), "{feature_bytes}");
            assert_eq!(found.sent().len(), 1);
        }
    }

    #[test]
    fn pairing_sends_nothing_to_a_collection_without_a_readable_descriptor() {
        let main = answers(&ID);
        let unreadable = answers(&ID);
        assert_eq!(
            pair_screen(&main, [Ok(unreadable.clone())]).err(),
            Some(Error::NotFound)
        );
        assert!(unreadable.sent().is_empty());

        let main = answers(&ID);
        let malformed = answers(&ID);
        malformed.set_report_descriptor(&[0x96, 0x07]);
        assert_eq!(
            pair_screen(&main, [Ok(malformed.clone())]).err(),
            Some(Error::NotFound)
        );
        assert!(malformed.sent().is_empty());
    }

    // The keyboard's own collection.

    fn no_candidates() -> [Result<FakeTransport, Error>; 0] {
        []
    }

    fn s85() -> &'static Model {
        model::find(S85_ES).unwrap()
    }

    #[test]
    fn a_main_collection_with_full_length_reports_opens_with_nothing_sent() {
        let main = FakeTransport::new();
        main.set_report_descriptor(&descriptor(519));
        let device = assemble(main.clone(), no_candidates(), s85()).unwrap();
        assert_eq!(device.model().pid, S85_ES);
        assert!(main.sent().is_empty());
    }

    #[test]
    fn a_main_collection_of_another_length_is_refused_with_nothing_sent() {
        for feature_bytes in [0, 64, 518, 520, 1031] {
            let main = FakeTransport::new();
            main.set_report_descriptor(&descriptor(feature_bytes));
            let result = assemble(main.clone(), no_candidates(), s85());
            assert_eq!(
                result.err(),
                Some(Error::NoUsableCollection(S85_ES)),
                "{feature_bytes}"
            );
            assert!(main.sent().is_empty(), "{feature_bytes}");
        }
    }

    #[test]
    fn a_main_collection_with_no_report_9_or_a_broken_descriptor_is_refused() {
        // Report 5 only, as a collection of another kind could have.
        let other_report = [
            0x06, 0x02, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x05, 0x75, 0x08, 0x96, 0x07, 0x02,
            0x09, 0x01, 0xB1, 0x02, 0xC0,
        ];
        let cut_off = [0x96, 0x07];
        for bytes in [&other_report[..], &cut_off[..]] {
            let main = FakeTransport::new();
            main.set_report_descriptor(bytes);
            let result = assemble(main.clone(), no_candidates(), s85());
            assert_eq!(result.err(), Some(Error::NoUsableCollection(S85_ES)));
            assert!(main.sent().is_empty());
        }
    }

    #[test]
    fn a_main_collection_whose_descriptor_cant_be_read_is_refused_and_says_so() {
        let main = FakeTransport::new();
        assert!(main.report_descriptor().is_err());
        let Err(Error::Hid(message)) = assemble(main.clone(), no_candidates(), s85()) else {
            panic!("should have been refused");
        };
        assert!(message.contains("report descriptor"), "{message}");
        assert!(main.sent().is_empty());
    }

    #[test]
    fn a_main_collection_is_checked_before_pairing_asks_it_anything() {
        let main = answers(&ID);
        main.set_report_descriptor(&descriptor(64));
        let screen = candidate(&ID);
        let result = assemble(
            main.clone(),
            [Ok(screen.clone())],
            model::find(N99).unwrap(),
        );
        assert_eq!(result.err(), Some(Error::NoUsableCollection(N99)));
        assert!(main.sent().is_empty());
        assert!(screen.sent().is_empty());
    }

    #[test]
    fn a_keyboard_whose_second_collection_cant_be_found_isnt_taken_for_no_keyboard() {
        let main = answers(&ID);
        main.set_report_descriptor(&descriptor(519));
        let result = assemble(main, no_candidates(), model::find(N99).unwrap());
        assert_eq!(result.err(), Some(Error::NoUsableCollection(N99)));
    }

    #[test]
    fn a_model_with_two_collections_is_paired_after_the_check() {
        let main = answers(&ID);
        main.set_report_descriptor(&descriptor(519));
        let screen = candidate(&ID);
        assemble(
            main.clone(),
            [Ok(screen.clone())],
            model::find(N99).unwrap(),
        )
        .unwrap();
        assert_eq!(main.events()[0], FakeEvent::ReadDescriptor);
        assert_eq!(main.sent().len(), 1);
        assert_eq!(screen.sent().len(), 1);
    }

    fn denied() -> Result<FakeTransport, Error> {
        Err(Error::Hid("access denied".to_string()))
    }

    #[test]
    fn pairing_says_so_when_a_candidate_couldnt_be_opened() {
        let main = answers(&ID);
        let other = candidate(&OTHER_ID);
        let Err(Error::Hid(message)) = pair_screen(&main, [denied(), Ok(other)]) else {
            panic!("should have been an error from the transport");
        };
        assert!(message.contains("couldn't open"), "{message}");
        assert!(message.contains("access denied"), "{message}");
    }

    #[test]
    fn pairing_still_works_when_another_candidate_couldnt_be_opened() {
        let main = answers(&ID);
        let right = candidate(&ID);
        pair_screen(&main, [denied(), Ok(right.clone())]).unwrap();
        assert_eq!(right.sent().len(), 1);
    }
}
