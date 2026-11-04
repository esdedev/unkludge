//! The 520-byte feature reports sent to the keyboard, and the acks it answers with.

use crate::Error;

/// Size of every report sent, report ID included.
pub const REPORT_LEN: usize = 520;
/// Size of the header at the start of each report.
pub const HEADER_LEN: usize = 8;
/// Data bytes that fit in one report.
pub(crate) const MAX_DATA: usize = REPORT_LEN - HEADER_LEN;
/// Report ID of every report sent.
pub const REPORT_ID: u8 = 0x09;

/// One report, ready to send.
pub(crate) type Report = [u8; REPORT_LEN];

/// The bytes that name each command, for whatever needs to recognise a report, as a dry run does.
/// They're only numbers: nothing outside this crate can put one in a report or reach a transport.
pub mod command_byte {
    pub const CONFIG_WRITE: u8 = 0x04;
    pub const PALETTES: u8 = 0x0A;
    pub const CLOCK: u8 = 0x0B;
    pub const SCREEN_DATA: u8 = 0x0C;
    pub const SCREEN_CONTROL: u8 = 0x0D;
    pub const PAIR_ID_READ: u8 = 0x82;
    pub const CONFIG_READ: u8 = 0x84;
}

/// The commands this crate can send. These are the only ones that exist here: [`cmd::Cmd`] and
/// [`cmd::ReadCmd`] can't be made from a byte outside this module, so no other command can reach
/// a report. Writes and reads are separate types, so a read can't put a write on the wire.
pub(crate) mod cmd {
    use super::command_byte;

    /// A command that sends data to the keyboard.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Cmd(u8);

    /// A command that asks the keyboard for data, and sends none.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct ReadCmd(u8);

    impl Cmd {
        pub(crate) const fn byte(self) -> u8 {
            self.0
        }
    }

    impl ReadCmd {
        pub(crate) const fn byte(self) -> u8 {
            self.0
        }
    }

    /// Writes the 128-byte config block.
    pub(crate) const CONFIG: Cmd = Cmd(command_byte::CONFIG_WRITE);
    /// Writes the effect palettes.
    pub(crate) const PALETTES: Cmd = Cmd(command_byte::PALETTES);
    /// Writes the clock.
    pub(crate) const CLOCK: Cmd = Cmd(command_byte::CLOCK);
    /// One chunk of screen image data.
    pub(crate) const SCREEN_DATA: Cmd = Cmd(command_byte::SCREEN_DATA);
    /// Starts, finishes or aborts a screen upload.
    pub(crate) const SCREEN_CONTROL: Cmd = Cmd(command_byte::SCREEN_CONTROL);

    /// Reads the config block.
    pub(crate) const CONFIG_READ: ReadCmd = ReadCmd(command_byte::CONFIG_READ);
    /// Reads the 6-byte identity used to pair a second collection with the keyboard.
    pub(crate) const PAIR_ID_READ: ReadCmd = ReadCmd(command_byte::PAIR_ID_READ);

    /// Every command byte above, for tests to check outgoing reports against.
    #[cfg(any(test, feature = "fake"))]
    pub(crate) const ALLOWED: [u8; 7] = [
        CONFIG.byte(),
        CONFIG_READ.byte(),
        PALETTES.byte(),
        CLOCK.byte(),
        SCREEN_DATA.byte(),
        SCREEN_CONTROL.byte(),
        PAIR_ID_READ.byte(),
    ];

    #[cfg(any(test, feature = "fake"))]
    pub(crate) fn is_allowed(byte: u8) -> bool {
        ALLOWED.contains(&byte)
    }
}

use cmd::{Cmd, ReadCmd};

fn empty_report(first: [u8; 8]) -> Report {
    let mut report = [0; REPORT_LEN];
    report[..HEADER_LEN].copy_from_slice(&first);
    report
}

fn le(len: usize) -> [u8; 2] {
    (len as u16).to_le_bytes()
}

/// `09 <cmd> <arg> 00 <chunks> <idx> <len lo> <len hi> <payload>`, zero-filled to 520 bytes.
pub(crate) fn build(command: Cmd, arg: u8, chunks: u8, idx: u8, payload: &[u8]) -> Report {
    assert!(payload.len() <= MAX_DATA, "payload must fit in one report");
    let [lo, hi] = le(payload.len());
    let mut report = empty_report([REPORT_ID, command.byte(), arg, 0, chunks, idx, lo, hi]);
    report[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);
    report
}

/// A read request: the header states the length wanted, and there is no data.
pub(crate) fn read_request(command: ReadCmd, arg: u8, len: usize) -> Report {
    assert!(len <= MAX_DATA, "a read must fit in one report");
    let [lo, hi] = le(len);
    empty_report([REPORT_ID, command.byte(), arg, 0, 1, 0, lo, hi])
}

/// Splits a payload into reports of up to 512 bytes. An empty payload is one empty report.
pub(crate) fn generic_reports(command: Cmd, arg: u8, payload: &[u8]) -> Result<Vec<Report>, Error> {
    let count = payload.len().div_ceil(MAX_DATA).max(1);
    let chunks = u8::try_from(count)
        .map_err(|_| Error::Invalid(format!("{} bytes need too many chunks", payload.len())))?;
    if payload.is_empty() {
        return Ok(vec![build(command, arg, chunks, 0, payload)]);
    }
    Ok(payload
        .chunks(MAX_DATA)
        .zip(0..=chunks)
        .map(|(data, idx)| build(command, arg, chunks, idx, data))
        .collect())
}

/// The chunk checksum: `0xFF` minus the sum of the payload bytes, modulo 256.
pub(crate) fn checksum(payload: &[u8]) -> u8 {
    let sum = payload.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    0xFF - sum
}

/// `09 0C 00 <frame> <idx> <check> <len lo> <len hi> <payload>`, zero-filled to 520 bytes. `check`
/// is 0 unless the model uses the checksum, and `len` is the true payload length.
pub(crate) fn screen_chunk(frame: u8, idx: u8, payload: &[u8], crc: bool) -> Report {
    assert!(payload.len() <= MAX_DATA, "payload must fit in one report");
    let check = if crc { checksum(payload) } else { 0 };
    let [lo, hi] = le(payload.len());
    let mut report = empty_report([
        REPORT_ID,
        cmd::SCREEN_DATA.byte(),
        0,
        frame,
        idx,
        check,
        lo,
        hi,
    ]);
    report[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);
    report
}

/// The two-bit state field of a screen control report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenState {
    Finish = 0,
    Start = 1,
    Abort = 2,
}

/// A screen control report with payload `[(state << 6) | slot] [frames LE16] [delay LE16]`. The
/// slot is always 0.
pub(crate) fn screen_control(state: ScreenState, frames: u16, delay_ms: u16) -> Report {
    let [frames_lo, frames_hi] = frames.to_le_bytes();
    let [delay_lo, delay_hi] = delay_ms.to_le_bytes();
    let payload = [(state as u8) << 6, frames_lo, frames_hi, delay_lo, delay_hi];
    build(cmd::SCREEN_CONTROL, 0, 1, 0, &payload)
}

/// An acknowledgement: the input report `09 0A 06 <status> <idx> <check> ...`, which arrives with
/// or without the leading report ID depending on the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ack {
    /// Non-zero means accepted.
    pub(crate) status: u8,
    pub(crate) idx: u8,
    pub(crate) check: u8,
}

impl Ack {
    /// Reads an input report. Any other kind of report gives `None`.
    pub(crate) fn parse(input: &[u8]) -> Option<Ack> {
        let body = match input {
            [REPORT_ID, rest @ ..] => rest,
            other => other,
        };
        match body {
            [0x0A, 0x06, status, idx, check, ..] => Some(Ack {
                status: *status,
                idx: *idx,
                check: *check,
            }),
            _ => None,
        }
    }

    /// Whether this ack accepts the report that `expect` was made for.
    pub(crate) fn confirms(&self, expect: AckExpect) -> bool {
        self.status != 0
            && expect.idx.is_none_or(|idx| idx == self.idx)
            && expect.check.is_none_or(|check| check == self.check)
    }
}

/// What an ack must contain, beyond a non-zero status, to accept a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AckExpect {
    idx: Option<u8>,
    check: Option<u8>,
}

impl AckExpect {
    /// For a screen chunk: the ack repeats its index and, on checksum models, its checksum. It has
    /// no frame number, so a late ack for the same index of the previous frame looks just the same.
    pub(crate) fn chunk(report: &Report, crc: bool) -> Self {
        AckExpect {
            idx: Some(report[4]),
            check: crc.then_some(report[5]),
        }
    }

    /// For a screen control report: a non-zero status is enough. Any ack would do, so stale input
    /// must be drained before sending one.
    pub(crate) fn control() -> Self {
        AckExpect {
            idx: None,
            check: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allow_list_is_exactly_the_commands_in_use() {
        assert_eq!(cmd::ALLOWED, [0x04, 0x84, 0x0A, 0x0B, 0x0C, 0x0D, 0x82]);
        assert!(!cmd::is_allowed(0x11));
        assert!(!cmd::is_allowed(0x12));
        assert!(cmd::is_allowed(0x0C));
    }

    #[test]
    fn header_layout() {
        let p = build(cmd::CONFIG, 0, 1, 0, &[0xAA; 128]);
        assert_eq!(p.len(), REPORT_LEN);
        assert_eq!(&p[..8], &[0x09, 0x04, 0x00, 0x00, 0x01, 0x00, 0x80, 0x00]);
        assert_eq!(p[8], 0xAA);
        assert_eq!(p[135], 0xAA);
        assert_eq!(p[136], 0x00);
    }

    #[test]
    fn header_carries_arg_chunks_idx_and_length() {
        let p = build(cmd::PALETTES, 3, 2, 1, &[1; 420]);
        assert_eq!(&p[..8], &[0x09, 0x0A, 0x03, 0x00, 0x02, 0x01, 0xA4, 0x01]);
        assert!(p[8 + 420..].iter().all(|&b| b == 0));
    }

    #[test]
    fn a_full_chunk_fills_the_report() {
        let p = build(cmd::PALETTES, 0, 1, 0, &[7; MAX_DATA]);
        assert_eq!(&p[6..8], &[0x00, 0x02]);
        assert_eq!(p[REPORT_LEN - 1], 7);
    }

    #[test]
    #[should_panic(expected = "fit")]
    fn a_payload_too_long_for_one_report_is_a_bug() {
        build(cmd::PALETTES, 0, 1, 0, &[7; MAX_DATA + 1]);
    }

    #[test]
    fn read_requests_state_the_length_and_carry_no_data() {
        let p = read_request(cmd::CONFIG_READ, 0, 128);
        assert_eq!(&p[..8], &[0x09, 0x84, 0x00, 0x00, 0x01, 0x00, 0x80, 0x00]);
        assert!(p[8..].iter().all(|&b| b == 0));

        let p = read_request(cmd::PAIR_ID_READ, 1, 6);
        assert_eq!(&p[..8], &[0x09, 0x82, 0x01, 0x00, 0x01, 0x00, 0x06, 0x00]);
    }

    #[test]
    fn generic_payloads_are_split_at_512_bytes() {
        let payload: Vec<u8> = (0..1100u32).map(|i| i as u8).collect();
        let reports = generic_reports(cmd::PALETTES, 0, &payload).unwrap();
        assert_eq!(reports.len(), 3);
        assert_eq!(
            &reports[0][..8],
            &[0x09, 0x0A, 0x00, 0x00, 0x03, 0x00, 0x00, 0x02]
        );
        assert_eq!(
            &reports[1][..8],
            &[0x09, 0x0A, 0x00, 0x00, 0x03, 0x01, 0x00, 0x02]
        );
        assert_eq!(
            &reports[2][..8],
            &[0x09, 0x0A, 0x00, 0x00, 0x03, 0x02, 0x4C, 0x00]
        );
        assert_eq!(reports[1][8], payload[512]);
        assert_eq!(reports[2][8], payload[1024]);
    }

    #[test]
    fn an_empty_generic_payload_is_one_report() {
        let reports = generic_reports(cmd::CLOCK, 0, &[]).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            &reports[0][..8],
            &[0x09, 0x0B, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn a_payload_needing_too_many_chunks_is_refused() {
        let payload = vec![0; 255 * MAX_DATA + 1];
        assert!(matches!(
            generic_reports(cmd::PALETTES, 0, &payload),
            Err(Error::Invalid(_))
        ));
        assert_eq!(
            generic_reports(cmd::PALETTES, 0, &payload[1..])
                .unwrap()
                .len(),
            255
        );
    }

    #[test]
    fn screen_chunk_header_without_checksum() {
        let p = screen_chunk(1, 0xD6, &[0xFF; 512], false);
        assert_eq!(p.len(), REPORT_LEN);
        assert_eq!(&p[..8], &[0x09, 0x0C, 0x00, 0x01, 0xD6, 0x00, 0x00, 0x02]);
        assert_eq!(p[8], 0xFF);
        assert_eq!(p[REPORT_LEN - 1], 0xFF);
    }

    #[test]
    fn short_last_chunk_states_its_true_length_and_is_zero_filled() {
        let p = screen_chunk(0, 126, &[0x11; 288], false);
        assert_eq!(&p[..8], &[0x09, 0x0C, 0x00, 0x00, 0x7E, 0x00, 0x20, 0x01]);
        assert_eq!(p[8 + 287], 0x11);
        assert!(p[8 + 288..].iter().all(|&b| b == 0));
    }

    #[test]
    fn checksum_is_ff_minus_the_byte_sum() {
        assert_eq!(checksum(&[]), 0xFF);
        assert_eq!(checksum(&[1, 2, 3]), 0xF9);
        // The sum wraps at 256: 0xFF + 0x02 = 0x101, which counts as 1.
        assert_eq!(checksum(&[0xFF, 0x02]), 0xFE);
        assert_eq!(checksum(&[0x11; 288]), 0xFF - 0x20);
    }

    #[test]
    fn screen_chunk_with_checksum_covers_the_payload_only() {
        let p = screen_chunk(2, 5, &[1, 2, 3], true);
        assert_eq!(&p[..8], &[0x09, 0x0C, 0x00, 0x02, 0x05, 0xF9, 0x03, 0x00]);
    }

    #[test]
    fn screen_control_states() {
        let start = screen_control(ScreenState::Start, 2, 20);
        assert_eq!(
            &start[..13],
            &[
                0x09, 0x0D, 0x00, 0x00, 0x01, 0x00, 0x05, 0x00, 0x40, 0x02, 0x00, 0x14, 0x00
            ]
        );
        let finish = screen_control(ScreenState::Finish, 0x0102, 0x0304);
        assert_eq!(&finish[8..13], &[0x00, 0x02, 0x01, 0x04, 0x03]);
        let abort = screen_control(ScreenState::Abort, 1, 0);
        assert_eq!(&abort[8..13], &[0x80, 0x01, 0x00, 0x00, 0x00]);
        assert!(abort[13..].iter().all(|&b| b == 0));
    }

    #[test]
    fn ack_with_and_without_report_id() {
        let a = Ack {
            status: 1,
            idx: 0x73,
            check: 0x5C,
        };
        assert_eq!(
            Ack::parse(&[0x09, 0x0A, 0x06, 0x01, 0x73, 0x5C, 0, 0]),
            Some(a)
        );
        assert_eq!(Ack::parse(&[0x0A, 0x06, 0x01, 0x73, 0x5C, 0, 0]), Some(a));
    }

    #[test]
    fn other_input_reports_are_not_acks() {
        assert_eq!(Ack::parse(&[0x09, 0x0B, 0, 0]), None);
        // Same family, other type: a notice, not an ack.
        assert_eq!(Ack::parse(&[0x09, 0x0A, 0x02, 0x01, 0x73, 0, 0, 0]), None);
        assert_eq!(Ack::parse(&[0x0A, 0x05, 0x01, 0x73, 0, 0, 0]), None);
        assert_eq!(Ack::parse(&[0x07, 0x0A, 0x06, 0x01, 0x73, 0, 0, 0]), None);
    }

    #[test]
    fn an_all_zero_input_report_is_not_an_ack() {
        assert_eq!(Ack::parse(&[0; 8]), None);
        assert_eq!(Ack::parse(&[0x09, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn short_input_reports_are_not_acks() {
        assert_eq!(Ack::parse(&[]), None);
        assert_eq!(Ack::parse(&[0x09]), None);
        assert_eq!(Ack::parse(&[0x09, 0x0A, 0x06, 0x01, 0x73]), None);
        assert_eq!(Ack::parse(&[0x0A, 0x06, 0x01, 0x73]), None);
    }

    fn ack(status: u8, idx: u8, check: u8) -> Ack {
        Ack { status, idx, check }
    }

    #[test]
    fn a_chunk_ack_needs_status_and_index() {
        let p = screen_chunk(0, 7, &[1, 2, 3], false);
        let expect = AckExpect::chunk(&p, false);
        assert!(ack(1, 7, 0).confirms(expect));
        // Any non-zero status counts.
        assert!(ack(2, 7, 0).confirms(expect));
        assert!(!ack(0, 7, 0).confirms(expect));
        assert!(!ack(1, 8, 0).confirms(expect));
        // The check byte only counts on models that use it.
        assert!(ack(1, 7, 0x55).confirms(expect));
    }

    #[test]
    fn a_chunk_ack_with_checksum_must_repeat_it() {
        let p = screen_chunk(0, 7, &[1, 2, 3], true);
        let expect = AckExpect::chunk(&p, true);
        assert!(ack(1, 7, 0xF9).confirms(expect));
        assert!(!ack(1, 7, 0xF8).confirms(expect));
    }

    #[test]
    fn a_control_ack_only_needs_status() {
        let expect = AckExpect::control();
        assert!(ack(1, 0x33, 0x44).confirms(expect));
        assert!(!ack(0, 0, 0).confirms(expect));
    }
}
