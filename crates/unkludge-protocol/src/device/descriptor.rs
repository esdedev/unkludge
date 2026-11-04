//! Reads a HID report descriptor far enough to tell how long a feature report is.

/// Item types, from bits 2-3 of an item's first byte.
const MAIN: u8 = 0;
const GLOBAL: u8 = 1;
/// Item tags, from its top 4 bits.
const FEATURE: u8 = 0xB;
const REPORT_SIZE: u8 = 0x7;
const REPORT_ID: u8 = 0x8;
const REPORT_COUNT: u8 = 0x9;
const PUSH: u8 = 0xA;
const POP: u8 = 0xB;
/// First byte of a long item, followed by its length and tag. None are defined, so they're skipped.
const LONG_ITEM: u8 = 0xFE;

/// The values that stay in force from one item to the next.
#[derive(Clone, Copy, Default)]
struct Globals {
    report_size: u64,
    report_count: u64,
    report_id: u64,
}

/// The length in bytes of the feature report with this ID, counting the ID. `None` if the
/// descriptor has no such report or is malformed.
pub(crate) fn feature_report_len(descriptor: &[u8], report_id: u8) -> Option<usize> {
    let mut globals = Globals::default();
    let mut pushed = Vec::new();
    let mut bits = 0u64;
    let mut found = false;

    let mut rest = descriptor;
    while let [first, after_first @ ..] = rest {
        if *first == LONG_ITEM {
            let [len, _tag, data @ ..] = after_first else {
                return None;
            };
            rest = data.get(usize::from(*len)..)?;
            continue;
        }

        // The low 2 bits give the data length, with 3 meaning 4 bytes.
        let len = match first & 0b11 {
            3 => 4,
            len => usize::from(len),
        };
        let data = after_first.get(..len)?;
        rest = &after_first[len..];
        let value = data
            .iter()
            .rev()
            .fold(0, |value, &byte| (value << 8) | u64::from(byte));

        match ((first >> 2) & 0b11, first >> 4) {
            (GLOBAL, REPORT_SIZE) => globals.report_size = value,
            (GLOBAL, REPORT_ID) => globals.report_id = value,
            (GLOBAL, REPORT_COUNT) => globals.report_count = value,
            (GLOBAL, PUSH) => pushed.push(globals),
            (GLOBAL, POP) => globals = pushed.pop()?,
            (MAIN, FEATURE) if globals.report_id == u64::from(report_id) => {
                let item_bits = globals.report_size.checked_mul(globals.report_count)?;
                bits = bits.checked_add(item_bits)?;
                found = true;
            }
            _ => {}
        }
    }

    if !found {
        return None;
    }
    usize::try_from(bits.div_ceil(8) + 1).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: u8 = 9;

    /// Usage page, usage and collection, then these items, then the end of the collection.
    fn collection(items: &[u8]) -> Vec<u8> {
        let mut descriptor = vec![0x06, 0x02, 0xFF, 0x09, 0x02, 0xA1, 0x01];
        descriptor.extend(items);
        descriptor.push(0xC0);
        descriptor
    }

    #[test]
    fn a_520_byte_feature_report() {
        // Report ID 9, report size 8 bits, report count 519, feature.
        let d = collection(&[
            0x85, 0x09, 0x75, 0x08, 0x96, 0x07, 0x02, 0x09, 0x01, 0xB1, 0x02,
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(520));
    }

    #[test]
    fn the_report_id_byte_only_counts_once() {
        let d = collection(&[0x85, 0x09, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x02]);
        assert_eq!(feature_report_len(&d, ID), Some(2));
    }

    #[test]
    fn other_report_ids_dont_count() {
        let d = collection(&[
            0x85, 0x05, 0x75, 0x08, 0x95, 0x05, 0xB1, 0x02, // Report 5: 5 bytes
            0x85, 0x09, 0x95, 0x07, 0xB1, 0x02, // Report 9: 7 bytes
            0x85, 0x06, 0x96, 0x07, 0x04, 0xB1, 0x02, // Report 6: 1031 bytes
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(8));
        assert_eq!(feature_report_len(&d, 5), Some(6));
        assert_eq!(feature_report_len(&d, 6), Some(1032));
    }

    #[test]
    fn input_and_output_items_dont_count() {
        let d = collection(&[
            0x85, 0x09, 0x75, 0x08, 0x95, 0x07, // Report 9, 7 bytes per item
            0x81, 0x02, // Input
            0x91, 0x02, // Output
            0xB1, 0x02, // Feature
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(8));
    }

    #[test]
    fn several_feature_items_add_up_and_round_up_to_bytes() {
        let d = collection(&[
            0x85, 0x09, // Report 9
            0x75, 0x01, 0x95, 0x07, 0xB1, 0x02, // 7 bits
            0x75, 0x08, 0x95, 0x02, 0xB1, 0x02, // 2 bytes
            0x75, 0x01, 0x95, 0x02, 0xB1, 0x03, // 2 bits, with other flags
        ]);
        // 25 bits make 4 bytes, plus the ID.
        assert_eq!(feature_report_len(&d, ID), Some(5));
    }

    #[test]
    fn global_values_carry_over_to_the_next_collection() {
        let mut d = collection(&[0x85, 0x09, 0x75, 0x08, 0x95, 0x02, 0xB1, 0x02]);
        d.extend(collection(&[0xB1, 0x02]));
        assert_eq!(feature_report_len(&d, ID), Some(5));
    }

    #[test]
    fn four_byte_values_are_read_little_endian() {
        let d = collection(&[
            0x85, 0x09, 0x75, 0x08, 0x97, 0x07, 0x02, 0x00, 0x00, 0xB1, 0x02,
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(520));
    }

    #[test]
    fn items_with_a_value_of_no_bytes_mean_zero() {
        // Report size with no data, so this feature item is empty. The next one counts.
        let d = collection(&[
            0x85, 0x09, 0x95, 0x05, 0x74, 0xB1, 0x02, 0x75, 0x08, 0xB1, 0x02,
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(6));
    }

    #[test]
    fn push_and_pop_restore_the_global_values() {
        let d = collection(&[
            0x85, 0x09, 0x75, 0x08, 0x95, 0x02, // Report 9, 2 bytes
            0xA4, // Push
            0x85, 0x05, 0x95, 0x63, 0xB1, 0x02, // Report 5, 99 bytes
            0xB4, // Pop
            0xB1, 0x02, // Report 9 again
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(3));
        assert_eq!(feature_report_len(&d, 5), Some(100));
    }

    #[test]
    fn long_items_are_skipped() {
        // A long item of 2 bytes whose content looks like a feature item.
        let d = collection(&[
            0x85, 0x09, 0x75, 0x08, 0x95, 0x02, 0xFE, 0x02, 0xF0, 0xB1, 0x02, 0xB1, 0x02,
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(3));
    }

    #[test]
    fn local_items_with_the_same_tag_arent_global_ones() {
        // Local tags 7, 8 and 9 use the same numbers as report size, ID and count.
        let d = collection(&[
            0x85, 0x09, 0x75, 0x08, 0x95, 0x02, // Report 9, 2 bytes
            0x79, 0x63, 0x89, 0x05, 0x99, 0x63, // Local items
            0xB1, 0x02,
        ]);
        assert_eq!(feature_report_len(&d, ID), Some(3));
    }

    #[test]
    fn no_feature_report_with_that_id_is_none() {
        assert_eq!(feature_report_len(&[], ID), None);
        let input_only = collection(&[0x85, 0x09, 0x75, 0x08, 0x95, 0x07, 0x81, 0x02]);
        assert_eq!(feature_report_len(&input_only, ID), None);
        let other_id = collection(&[0x85, 0x05, 0x75, 0x08, 0x95, 0x07, 0xB1, 0x02]);
        assert_eq!(feature_report_len(&other_id, ID), None);
        // No report IDs at all.
        let unnumbered = collection(&[0x75, 0x08, 0x95, 0x07, 0xB1, 0x02]);
        assert_eq!(feature_report_len(&unnumbered, ID), None);
    }

    #[test]
    fn a_malformed_descriptor_is_none() {
        // Cut off inside an item's value.
        let good = collection(&[0x85, 0x09, 0x75, 0x08, 0x96, 0x07, 0x02, 0xB1, 0x02]);
        assert_eq!(feature_report_len(&good[..good.len() - 1 - 1], ID), None);
        // Cut off inside a long item.
        let d = collection(&[0x85, 0x09, 0x75, 0x08, 0x95, 0x02, 0xB1, 0x02]);
        let mut long = d.clone();
        long.extend([0xFE, 0x05, 0xF0, 0x00]);
        assert_eq!(feature_report_len(&long, ID), None);
        // A long item's header cut off.
        let mut long = d.clone();
        long.extend([0xFE]);
        assert_eq!(feature_report_len(&long, ID), None);
        // A pop with nothing pushed.
        let mut pop = d;
        pop.push(0xB4);
        assert_eq!(feature_report_len(&pop, ID), None);
    }

    #[test]
    fn sizes_too_big_to_count_are_none() {
        let d = collection(&[
            0x85, 0x09, 0x77, 0xFF, 0xFF, 0xFF, 0xFF, 0x97, 0xFF, 0xFF, 0xFF, 0xFF, 0xB1, 0x02,
            0xB1, 0x02,
        ]);
        assert_eq!(feature_report_len(&d, ID), None);
    }
}
