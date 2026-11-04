//! A connected keyboard: how reports get sent, paced, retried and acknowledged.
//!
//! There are three send paths. Generic commands (config, palettes, clock) get no ack: each chunk
//! waits 25 ms first, and only the send itself is retried. Screen control reports are paced the
//! same way and then wait for an ack. Screen data chunks wait for an ack with no pause between
//! them, so the keyboard's answer sets the pace.

use std::any::Any;
use std::time::Duration;

use crate::Error;
use crate::model::Model;
use crate::packet::cmd::{Cmd, ReadCmd};
use crate::packet::{self, Ack, AckExpect, HEADER_LEN, REPORT_ID, REPORT_LEN, Report};
use crate::transport::{HidTransport, Transport};

pub(crate) mod descriptor;
#[cfg(target_os = "linux")]
pub(crate) mod node_lock;
mod open;

pub use open::{HidCollection, list_collections};

/// Wait before each generic chunk, each read request and each screen control report.
const CHUNK_DELAY: Duration = Duration::from_millis(25);
/// How many times one report is handed to the transport before giving up on it.
const SEND_TRIES: u32 = 3;
/// Gap between those tries, except for screen data.
const SEND_RETRY_GAP: Duration = Duration::from_millis(50);
/// Gap between those tries for screen data.
const ACKED_SEND_RETRY_GAP: Duration = Duration::from_millis(100);
/// Wait between a read request and fetching the reply.
const READ_WAIT: Duration = Duration::from_millis(25);
/// Default for `Device::ack_timeout`.
const ACK_TIMEOUT: Duration = Duration::from_millis(8000);
/// Default for `Device::retries`.
const RESENDS: u32 = 5;
/// With two collections, how long each wait on one of them lasts before checking the other.
const ACK_POLL_MS: i32 = 10;
/// Upper limit on the input reports one drain discards, so a chatty device can't hold it forever.
const DRAIN_LIMIT: usize = 256;
/// Big enough for the 8-byte input reports, and for longer ones if a platform pads them.
const INPUT_LEN: usize = 64;

/// Which of a keyboard's HID collections a report goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collection {
    /// Carries everything, except screen data on models with a second collection.
    Main,
    /// The second collection of `kind = 2` models.
    Screen,
}

/// What [`Device::on_packet`] gets to see. For display only: it can't change what is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketEvent<'a> {
    /// A report was handed to the transport.
    Sent {
        collection: Collection,
        report: &'a [u8],
    },
    /// An ack arrived `latency` after waiting for it began.
    Ack {
        idx: u8,
        accepted: bool,
        latency: Duration,
    },
}

/// The callback in [`Device::on_packet`].
pub type OnPacket = Box<dyn for<'a> FnMut(PacketEvent<'a>) + Send>;

/// How waiting for one ack ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AckOutcome {
    /// The ack accepts the report.
    Confirmed,
    /// An ack arrived that doesn't accept the report.
    Rejected,
    /// No ack arrived in time.
    TimedOut,
}

/// A connected keyboard.
///
/// The transports stay private: the only ways to reach the keyboard are this crate's named
/// operations, so nothing outside the allow-list can be sent.
pub struct Device<T: Transport = HidTransport> {
    main: T,
    screen: Option<T>,
    model: &'static Model,
    /// How long to wait for each ack.
    pub(crate) ack_timeout: Duration,
    /// How many times a screen chunk is sent again after an ack that doesn't accept it.
    pub(crate) retries: u32,
    /// Called for each report sent and each ack received.
    pub on_packet: Option<OnPacket>,
    /// Last, as fields go in this order: it outlives the transports.
    kept: Option<Box<dyn Any + Send>>,
}

impl<T: Transport> Device<T> {
    /// Wraps already-open collections. `screen` is the second collection, which `kind = 2` models
    /// need and others must not have.
    pub fn new(main: T, screen: Option<T>, model: &'static Model) -> Result<Self, Error> {
        if !model.is_supported() {
            return Err(Error::Unsupported(model.pid));
        }
        let wants_screen = model.has_second_collection();
        if wants_screen != screen.is_some() {
            return Err(Error::Invalid(format!(
                "model {:04X} {} a second collection",
                model.pid,
                if wants_screen {
                    "needs"
                } else {
                    "doesn't have"
                }
            )));
        }
        Ok(Device {
            main,
            screen,
            model,
            ack_timeout: ACK_TIMEOUT,
            retries: RESENDS,
            on_packet: None,
            kept: None,
        })
    }

    /// Holds on to something until the device is closed, as the lock that says who has the keyboard.
    pub fn keep_until_closed(&mut self, kept: Box<dyn Any + Send>) {
        self.kept = Some(kept);
    }

    /// The model of this keyboard.
    pub fn model(&self) -> &'static Model {
        self.model
    }

    /// Pauses, skipping the wait on transports with no hardware behind them.
    pub(crate) fn sleep(&self, duration: Duration) {
        self.main.sleep(duration);
    }

    /// Sends an unacked command on the main collection, split into chunks as needed.
    pub(crate) fn send_generic(
        &mut self,
        command: Cmd,
        arg: u8,
        payload: &[u8],
    ) -> Result<(), Error> {
        for report in packet::generic_reports(command, arg, payload)? {
            self.main.sleep(CHUNK_DELAY);
            try_send(&self.main, &report, SEND_RETRY_GAP)?;
            self.notify_sent(Collection::Main, &report);
        }
        Ok(())
    }

    /// Reads a block from the main collection, returning its `len` data bytes.
    pub(crate) fn read_block(
        &mut self,
        command: ReadCmd,
        arg: u8,
        len: usize,
    ) -> Result<Vec<u8>, Error> {
        let request = packet::read_request(command, arg, len);
        request_block(&self.main, &request)?;
        self.notify_sent(Collection::Main, &request);
        fetch_block(&self.main, command, len)
    }

    /// Discards input waiting on any collection, without waiting for more.
    pub(crate) fn drain(&mut self) -> Result<(), Error> {
        let mut input = [0; INPUT_LEN];
        for transport in std::iter::once(&self.main).chain(&self.screen) {
            for _ in 0..DRAIN_LIMIT {
                if transport.read_timeout(&mut input, 0)? == 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Sends one screen data chunk and waits for its ack. An ack that doesn't accept it sends the
    /// chunk again, up to `retries` times, then gives [`Error::Rejected`]. No ack in time gives
    /// [`Error::NoAck`] at once.
    pub(crate) fn send_screen_chunk(&mut self, report: &Report) -> Result<(), Error> {
        let to = self.screen_collection();
        let crc = self.model.screen.is_some_and(|s| s.crc);
        let expect = AckExpect::chunk(report, crc);
        let (frame, idx) = (report[3], report[4]);
        for _ in 0..=self.retries {
            match self.send_and_wait(to, report, expect, ACKED_SEND_RETRY_GAP)? {
                AckOutcome::Confirmed => return Ok(()),
                AckOutcome::Rejected => {}
                AckOutcome::TimedOut => return Err(Error::NoAck { frame, idx }),
            }
        }
        Err(Error::Rejected { frame, idx })
    }

    /// Sends a screen control report and waits for its ack: on the main collection, then on the
    /// second one if the model has it. Stops at the first collection that doesn't confirm, and
    /// leaves what to do about that to the caller.
    pub(crate) fn send_screen_control(&mut self, report: &Report) -> Result<AckOutcome, Error> {
        let outcome = self.send_control_to(Collection::Main, report)?;
        if outcome != AckOutcome::Confirmed || self.screen.is_none() {
            return Ok(outcome);
        }
        self.send_control_to(Collection::Screen, report)
    }

    /// Paced like a generic chunk. Input is only discarded after the pause, so that nothing that
    /// arrives during it can pass for the ack, as any ack confirms a control report.
    fn send_control_to(&mut self, to: Collection, report: &Report) -> Result<AckOutcome, Error> {
        self.transport(to).sleep(CHUNK_DELAY);
        self.send_and_wait(to, report, AckExpect::control(), SEND_RETRY_GAP)
    }

    fn screen_collection(&self) -> Collection {
        if self.screen.is_some() {
            Collection::Screen
        } else {
            Collection::Main
        }
    }

    fn transport(&self, collection: Collection) -> &T {
        match (collection, &self.screen) {
            (Collection::Screen, Some(screen)) => screen,
            _ => &self.main,
        }
    }

    /// One send and one wait. Input already waiting is discarded first: a stale ack would count as
    /// the answer to this report, and every later one would then be answered one ack behind. When
    /// the send keeps failing the wait still decides, since the report may have got through. If no
    /// ack comes either, the send's error is the one to report.
    fn send_and_wait(
        &mut self,
        to: Collection,
        report: &Report,
        expect: AckExpect,
        retry_gap: Duration,
    ) -> Result<AckOutcome, Error> {
        self.drain()?;
        let sent = try_send(self.transport(to), report, retry_gap);
        if sent.is_ok() {
            self.notify_sent(to, report);
        }
        match (self.wait_for_ack(to, expect)?, sent) {
            (AckOutcome::TimedOut, Err(error)) => Err(error),
            (outcome, _) => Ok(outcome),
        }
    }

    /// Waits for the next ack, ignoring other input. Other input doesn't extend the wait.
    fn wait_for_ack(&mut self, to: Collection, expect: AckExpect) -> Result<AckOutcome, Error> {
        let start = self.transport(to).now();
        let deadline = start + self.ack_timeout;
        let mut input = [0; INPUT_LEN];
        loop {
            let remaining = deadline.saturating_duration_since(self.transport(to).now());
            if remaining.is_zero() {
                return Ok(AckOutcome::TimedOut);
            }
            // Nothing read isn't necessarily the end of the wait: the deadline decides.
            let len = self.read_input(to, &mut input, remaining)?;
            let Some(ack) = Ack::parse(&input[..len]) else {
                continue;
            };
            let accepted = ack.confirms(expect);
            self.notify(PacketEvent::Ack {
                idx: ack.idx,
                accepted,
                latency: self.transport(to).now().saturating_duration_since(start),
            });
            return Ok(if accepted {
                AckOutcome::Confirmed
            } else {
                AckOutcome::Rejected
            });
        }
    }

    /// Reads one input report. With two collections both feed the same ack queue, so it waits
    /// briefly on the one the report went to, then checks the other.
    fn read_input(
        &self,
        to: Collection,
        input: &mut [u8],
        remaining: Duration,
    ) -> Result<usize, Error> {
        let ms = i32::try_from(remaining.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
        let Some(screen) = &self.screen else {
            return self.main.read_timeout(input, ms);
        };
        let (first, second) = match to {
            Collection::Screen => (screen, &self.main),
            Collection::Main => (&self.main, screen),
        };
        match first.read_timeout(input, ms.min(ACK_POLL_MS))? {
            0 => second.read_timeout(input, 0),
            len => Ok(len),
        }
    }

    fn notify_sent(&mut self, collection: Collection, report: &Report) {
        self.notify(PacketEvent::Sent { collection, report });
    }

    fn notify(&mut self, event: PacketEvent<'_>) {
        if let Some(on_packet) = &mut self.on_packet {
            on_packet(event);
        }
    }
}

/// Hands a report to the transport, up to [`SEND_TRIES`] times with `gap` between tries. Any error
/// gets the other tries, deliberately: see the pacing table in `docs/PROTOCOL.md`.
fn try_send<T: Transport>(transport: &T, report: &Report, gap: Duration) -> Result<(), Error> {
    let mut result = transport.send_feature(report);
    for _ in 1..SEND_TRIES {
        if result.is_ok() {
            break;
        }
        transport.sleep(gap);
        result = transport.send_feature(report);
    }
    result
}

/// Sends a read request, with the same pacing and tries as a generic chunk.
fn request_block<T: Transport>(transport: &T, request: &Report) -> Result<(), Error> {
    transport.sleep(CHUNK_DELAY);
    try_send(transport, request, SEND_RETRY_GAP)
}

/// Waits, fetches the reply to a read request and returns its `len` data bytes.
fn fetch_block<T: Transport>(
    transport: &T,
    command: ReadCmd,
    len: usize,
) -> Result<Vec<u8>, Error> {
    transport.sleep(READ_WAIT);
    let mut reply = [0; REPORT_LEN];
    reply[0] = REPORT_ID;
    let got = transport.get_feature(&mut reply)?;
    parse_reply(command, &reply[..got.min(REPORT_LEN)], len)
}

/// The reply repeats the request's header, with or without the leading report ID, followed by the
/// data. Anything else isn't a reply to this read, even with the same command in a write's form.
fn parse_reply(command: ReadCmd, reply: &[u8], len: usize) -> Result<Vec<u8>, Error> {
    let body = match reply {
        [REPORT_ID, rest @ ..] => rest,
        other => other,
    };
    let echo = body.first().copied();
    let read_cmd = command.byte();
    if echo != Some(read_cmd) {
        return Err(Error::Invalid(format!(
            "reply to command {read_cmd:#04x} echoes {echo:02X?}"
        )));
    }
    body.get(HEADER_LEN - 1..HEADER_LEN - 1 + len)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "reply to command {read_cmd:#04x} has {} bytes, wanted {len} of data",
                reply.len()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{ScreenState, cmd, screen_chunk, screen_control};
    use crate::transport::fake::{AckFault, FakeEvent, FakeTransport};
    use std::sync::{Arc, Mutex};

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn s85() -> &'static Model {
        crate::model::find(0x0265).unwrap()
    }

    fn m87() -> &'static Model {
        crate::model::find(0x01A2).unwrap()
    }

    fn n99() -> &'static Model {
        crate::model::find(0x01B8).unwrap()
    }

    fn device(fake: &FakeTransport) -> Device<FakeTransport> {
        Device::new(fake.clone(), None, s85()).unwrap()
    }

    fn pair() -> (FakeTransport, FakeTransport, Device<FakeTransport>) {
        let main = FakeTransport::with_auto_ack();
        let screen = FakeTransport::with_auto_ack();
        let device = Device::new(main.clone(), Some(screen.clone()), n99()).unwrap();
        (main, screen, device)
    }

    #[test]
    fn defaults_come_from_the_protocol() {
        let device = device(&FakeTransport::new());
        assert_eq!(device.ack_timeout, MS(8000));
        assert_eq!(device.retries, 5);
        assert_eq!(device.model().pid, 0x0265);
    }

    #[test]
    fn new_refuses_models_this_version_cant_configure() {
        let v1 = crate::model::MODELS
            .iter()
            .find(|m| m.protocol == crate::model::Protocol::V1)
            .unwrap();
        let result = Device::new(FakeTransport::new(), None, v1);
        assert_eq!(result.err(), Some(Error::Unsupported(v1.pid)));
    }

    #[test]
    fn new_wants_a_second_collection_exactly_when_the_model_has_one() {
        let result = Device::new(FakeTransport::new(), None, n99());
        assert!(matches!(result.err(), Some(Error::Invalid(_))));
        let result = Device::new(FakeTransport::new(), Some(FakeTransport::new()), s85());
        assert!(matches!(result.err(), Some(Error::Invalid(_))));
    }

    // Unacked generic sends.

    #[test]
    fn generic_send_waits_25_ms_before_each_chunk_and_reads_nothing() {
        let fake = FakeTransport::new();
        let mut device = device(&fake);
        let payload = vec![0x5A; 600];
        device.send_generic(cmd::PALETTES, 0, &payload).unwrap();

        let events = fake.events();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], FakeEvent::Sleep(MS(25)));
        assert!(matches!(&events[1], FakeEvent::Sent(r) if r[1] == 0x0A && r[5] == 0));
        assert_eq!(events[2], FakeEvent::Sleep(MS(25)));
        assert!(matches!(&events[3], FakeEvent::Sent(r) if r[1] == 0x0A && r[5] == 1));
    }

    #[test]
    fn generic_send_retries_a_failed_send_50_ms_later() {
        let fake = FakeTransport::new();
        fake.fail_send(0);
        fake.fail_send(1);
        device(&fake).send_generic(cmd::CLOCK, 0, &[1; 14]).unwrap();

        assert_eq!(fake.sent().len(), 1);
        assert_eq!(fake.sleeps(), [MS(25), MS(50), MS(50)]);
    }

    #[test]
    fn generic_send_gives_up_after_3_tries_with_the_transport_error() {
        let fake = FakeTransport::new();
        (0..3).for_each(|n| fake.fail_send(n));
        let result = device(&fake).send_generic(cmd::PALETTES, 0, &[1; 600]);

        assert!(matches!(result, Err(Error::Hid(_))));
        assert!(fake.sent().is_empty());
        assert_eq!(fake.sleeps(), [MS(25), MS(50), MS(50)]);
        let attempts = fake
            .events()
            .iter()
            .filter(|e| matches!(e, FakeEvent::SendFailed(_)))
            .count();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn generic_sends_only_use_the_main_collection() {
        let (main, screen, mut device) = pair();
        device.send_generic(cmd::CLOCK, 0, &[1; 14]).unwrap();
        assert_eq!(main.sent().len(), 1);
        assert!(screen.sent().is_empty());
    }

    // Reads.

    fn reply(echo: u8, data: &[u8], with_id: bool) -> Vec<u8> {
        let mut reply = Vec::new();
        if with_id {
            reply.push(0x09);
        }
        reply.extend([echo, 0, 0, 1, 0, data.len() as u8, 0]);
        reply.extend(data);
        reply
    }

    #[test]
    fn read_block_sends_the_request_waits_then_fetches() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&reply(0x84, &[1, 2, 3], true));
        let data = device(&fake).read_block(cmd::CONFIG_READ, 0, 3).unwrap();
        assert_eq!(data, [1, 2, 3]);

        let events = fake.events();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], FakeEvent::Sleep(MS(25)));
        assert!(matches!(
            &events[1],
            FakeEvent::Sent(r) if r[..8] == [0x09, 0x84, 0, 0, 1, 0, 3, 0]
        ));
        assert_eq!(events[2], FakeEvent::Sleep(MS(25)));
        assert_eq!(events[3], FakeEvent::GetFeature);
    }

    #[test]
    fn read_block_handles_replies_without_the_report_id() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&reply(0x84, &[1, 2, 3], false));
        let data = device(&fake).read_block(cmd::CONFIG_READ, 0, 3).unwrap();
        assert_eq!(data, [1, 2, 3]);
    }

    #[test]
    fn read_block_rejects_an_echo_with_the_read_bit_clear() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&reply(0x04, &[1, 2, 3], true));
        let result = device(&fake).read_block(cmd::CONFIG_READ, 0, 3);
        assert!(matches!(result, Err(Error::Invalid(_))));
    }

    #[test]
    fn read_block_returns_only_the_length_asked_for() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&reply(0x84, &[1, 2, 3, 4, 5], true));
        let data = device(&fake).read_block(cmd::CONFIG_READ, 0, 3).unwrap();
        assert_eq!(data, [1, 2, 3]);
    }

    #[test]
    fn read_block_rejects_a_wrong_echo() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&reply(0x83, &[1, 2, 3], true));
        let result = device(&fake).read_block(cmd::CONFIG_READ, 0, 3);
        assert!(matches!(result, Err(Error::Invalid(_))));
    }

    #[test]
    fn read_block_rejects_a_short_reply() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&reply(0x84, &[1, 2], true));
        let result = device(&fake).read_block(cmd::CONFIG_READ, 0, 3);
        assert!(matches!(result, Err(Error::Invalid(_))));

        fake.queue_feature_reply(&[]);
        let result = device(&fake).read_block(cmd::CONFIG_READ, 0, 3);
        assert!(matches!(result, Err(Error::Invalid(_))));
    }

    #[test]
    fn read_block_passes_on_transport_errors() {
        let fake = FakeTransport::new();
        // The fake has no reply queued, so fetching fails.
        let result = device(&fake).read_block(cmd::CONFIG_READ, 0, 3);
        assert!(matches!(result, Err(Error::Hid(_))));
    }

    // Acked sends, the path screen data takes.

    fn chunk(idx: u8) -> crate::packet::Report {
        screen_chunk(0, idx, &[1, 2, 3], false)
    }

    #[test]
    fn a_chunk_returns_on_its_ack_with_no_pause() {
        let fake = FakeTransport::with_auto_ack();
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 1);
        assert!(fake.sleeps().is_empty());
    }

    #[test]
    fn the_ack_wait_lasts_up_to_8_seconds() {
        let fake = FakeTransport::with_auto_ack();
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.events().last(), Some(&FakeEvent::Read { ms: 8000 }));
    }

    #[test]
    fn other_input_reports_dont_count_as_acks() {
        let fake = FakeTransport::new();
        fake.queue_input_after_send(&[0x09, 0x0A, 0x02, 0x01, 0x07, 0, 0, 0]);
        fake.queue_input_after_send(&[0x09, 0x0B, 0, 0, 0, 0, 0, 0]);
        fake.queue_input_after_send(&[0x09, 0x0A, 0x06, 0x01, 0x07, 0, 0, 0]);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn acks_without_the_report_id_count() {
        let fake = FakeTransport::new();
        fake.queue_input_after_send(&[0x0A, 0x06, 0x01, 0x07, 0, 0, 0]);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn only_the_bytes_read_count_as_the_report() {
        // The first report leaves a status and an index in the buffer, where the rest of an ack
        // would go. The second has only the first 3 bytes of one.
        let fake = FakeTransport::new();
        fake.queue_input_after_send(&[0x09, 0x0B, 0x00, 0x01, 0x07, 0, 0, 0]);
        fake.queue_input_after_send(&[0x09, 0x0A, 0x06]);
        let result = device(&fake).send_screen_chunk(&chunk(7));
        assert_eq!(result, Err(Error::NoAck { frame: 0, idx: 7 }));
    }

    #[test]
    fn other_input_doesnt_extend_the_wait() {
        // 20 other reports at a second each, then an ack that comes too late to count.
        let fake = FakeTransport::new();
        fake.set_input_latency(MS(1000));
        for _ in 0..20 {
            fake.queue_input_after_send(&[0x09, 0x0B, 0, 0, 0, 0, 0, 0]);
        }
        fake.queue_input_after_send(&[0x09, 0x0A, 0x06, 0x01, 0x07, 0, 0, 0]);

        let result = device(&fake).send_screen_chunk(&chunk(7));
        assert_eq!(result, Err(Error::NoAck { frame: 0, idx: 7 }));
        assert_eq!(fake.pending_input(), 21 - 8);
        let waits: Vec<i32> = fake
            .events()
            .iter()
            .filter_map(|e| match e {
                FakeEvent::Read { ms } if *ms > 0 => Some(*ms),
                _ => None,
            })
            .collect();
        assert_eq!(waits, [8000, 7000, 6000, 5000, 4000, 3000, 2000, 1000]);
    }

    #[test]
    fn a_read_returning_0_early_is_not_a_timeout() {
        let fake = FakeTransport::new();
        fake.queue_input_after_send(&[]);
        fake.queue_input_after_send(&[0x09, 0x0A, 0x06, 0x01, 0x07, 0, 0, 0]);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn one_stale_ack_doesnt_make_later_chunks_go_out_twice() {
        let fake = FakeTransport::with_auto_ack();
        fake.queue_input(&[0x09, 0x0A, 0x06, 0x01, 0x63, 0, 0, 0]);
        let mut device = device(&fake);
        for idx in 0..50 {
            device.send_screen_chunk(&chunk(idx)).unwrap();
        }
        let sent: Vec<u8> = fake.sent().iter().map(|report| report[4]).collect();
        assert_eq!(sent, (0..50).collect::<Vec<u8>>());
    }

    #[test]
    fn input_is_drained_right_before_each_send() {
        let fake = FakeTransport::with_auto_ack();
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        let events = fake.events();
        assert_eq!(events[0], FakeEvent::Read { ms: 0 });
        assert!(matches!(events[1], FakeEvent::Sent(_)));
    }

    #[test]
    fn an_ack_for_another_chunk_resends() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::WrongIdx);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 2);
        assert_eq!(fake.sent()[0], fake.sent()[1]);
    }

    #[test]
    fn an_ack_with_status_zero_resends() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Reject);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 2);
    }

    #[test]
    fn a_wrong_checksum_resends_on_checksum_models_only() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::WrongCheck);
        let mut device = Device::new(fake.clone(), None, m87()).unwrap();
        device
            .send_screen_chunk(&screen_chunk(0, 7, &[1, 2, 3], true))
            .unwrap();
        assert_eq!(fake.sent().len(), 2);

        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::WrongCheck);
        self::device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn resends_are_bounded_then_rejected() {
        let fake = FakeTransport::with_auto_ack();
        (0..6).for_each(|n| fake.ack_fault(n, AckFault::Reject));
        let result = device(&fake).send_screen_chunk(&chunk(7));
        assert_eq!(result, Err(Error::Rejected { frame: 0, idx: 7 }));
        // The first send and 5 resends.
        assert_eq!(fake.sent().len(), 6);
    }

    #[test]
    fn the_resend_limit_can_be_changed() {
        let fake = FakeTransport::with_auto_ack();
        (0..6).for_each(|n| fake.ack_fault(n, AckFault::Reject));
        let mut device = device(&fake);
        device.retries = 1;
        assert!(device.send_screen_chunk(&chunk(7)).is_err());
        assert_eq!(fake.sent().len(), 2);
    }

    #[test]
    fn a_timeout_fails_without_resending() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Drop);
        let result = device(&fake).send_screen_chunk(&chunk(7));
        assert_eq!(result, Err(Error::NoAck { frame: 0, idx: 7 }));
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn a_failed_send_retries_100_ms_later() {
        let fake = FakeTransport::with_auto_ack();
        fake.fail_send(0);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(fake.sent().len(), 1);
        assert_eq!(fake.sleeps(), [MS(100)]);
    }

    #[test]
    fn after_3_failed_sends_the_ack_wait_decides() {
        // No ack: the transport error is the useful one to report.
        let fake = FakeTransport::new();
        (0..3).for_each(|n| fake.fail_send(n));
        let result = device(&fake).send_screen_chunk(&chunk(7));
        assert!(matches!(result, Err(Error::Hid(_))));
        assert_eq!(fake.sleeps(), [MS(100), MS(100)]);

        // An ack after all: the report got through.
        let fake = FakeTransport::new();
        (0..3).for_each(|n| fake.fail_send(n));
        fake.queue_input_after_send(&[0x09, 0x0A, 0x06, 0x01, 0x07, 0, 0, 0]);
        device(&fake).send_screen_chunk(&chunk(7)).unwrap();
    }

    #[test]
    fn a_read_error_is_returned() {
        let fake = FakeTransport::with_auto_ack();
        fake.disconnect();
        let result = device(&fake).send_screen_chunk(&chunk(7));
        assert!(matches!(result, Err(Error::Hid(_))));
    }

    // Screen control reports.

    fn control() -> crate::packet::Report {
        screen_control(ScreenState::Start, 1, 0)
    }

    #[test]
    fn control_reports_accept_any_index() {
        let fake = FakeTransport::new();
        fake.queue_input_after_send(&[0x09, 0x0A, 0x06, 0x01, 0x33, 0x44, 0, 0]);
        let outcome = device(&fake).send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::Confirmed);
    }

    #[test]
    fn a_stale_ack_doesnt_confirm_a_control_report() {
        let fake = FakeTransport::new();
        fake.queue_input(&[0x09, 0x0A, 0x06, 0x01, 0x33, 0x44, 0, 0]);
        let outcome = device(&fake).send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::TimedOut);
    }

    #[test]
    fn control_reports_tell_a_refusal_from_a_timeout() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Reject);
        let outcome = device(&fake).send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::Rejected);
        assert_eq!(fake.sent().len(), 1);

        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Drop);
        let outcome = device(&fake).send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::TimedOut);
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn a_control_report_waits_25_ms_then_drains_and_sends() {
        let fake = FakeTransport::with_auto_ack();
        device(&fake).send_screen_control(&control()).unwrap();
        let events = fake.events();
        assert_eq!(events[0], FakeEvent::Sleep(MS(25)));
        assert_eq!(events[1], FakeEvent::Read { ms: 0 });
        assert!(matches!(events[2], FakeEvent::Sent(_)));
        assert_eq!(fake.sleeps(), [MS(25)]);
    }

    #[test]
    fn a_failed_control_send_retries_50_ms_later() {
        let fake = FakeTransport::with_auto_ack();
        fake.fail_send(0);
        let outcome = device(&fake).send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::Confirmed);
        assert_eq!(fake.sent().len(), 1);
        assert_eq!(fake.sleeps(), [MS(25), MS(50)]);
    }

    // Models with a second collection.

    #[test]
    fn each_collection_gets_its_own_wait_for_a_control_report() {
        let (main, screen, mut device) = pair();
        device.send_screen_control(&control()).unwrap();
        assert_eq!(main.sleeps(), [MS(25)]);
        assert_eq!(screen.sleeps(), [MS(25)]);
    }

    #[test]
    fn control_goes_to_main_then_to_the_screen_collection() {
        let (main, screen, mut device) = pair();
        let outcome = device.send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::Confirmed);
        assert_eq!(main.sent().len(), 1);
        assert_eq!(screen.sent().len(), 1);
        assert_eq!(main.sent()[0], screen.sent()[0]);
    }

    #[test]
    fn control_stops_at_the_main_collection_when_it_refuses() {
        let (main, screen, mut device) = pair();
        main.ack_fault(0, AckFault::Reject);
        let outcome = device.send_screen_control(&control()).unwrap();
        assert_eq!(outcome, AckOutcome::Rejected);
        assert!(screen.sent().is_empty());
    }

    #[test]
    fn chunks_go_only_to_the_screen_collection() {
        let (main, screen, mut device) = pair();
        device.send_screen_chunk(&chunk(7)).unwrap();
        assert!(main.sent().is_empty());
        assert_eq!(screen.sent().len(), 1);
    }

    #[test]
    fn both_collections_feed_one_ack_queue() {
        let (main, screen, mut device) = pair();
        // The chunk goes to the screen collection, and the ack shows up on the main one.
        screen.route_acks_to(&main);
        device.send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(screen.sent().len(), 1);
    }

    #[test]
    fn a_timeout_with_two_collections_ends_at_the_deadline() {
        let (_main, screen, mut device) = pair();
        screen.ack_fault(0, AckFault::Drop);
        let result = device.send_screen_chunk(&chunk(7));
        assert_eq!(result, Err(Error::NoAck { frame: 0, idx: 7 }));
    }

    #[test]
    fn with_two_collections_the_wait_polls_in_short_slices() {
        let (main, screen, mut device) = pair();
        device.send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(screen.events().last(), Some(&FakeEvent::Read { ms: 10 }));
        // Only the drain before sending: the ack was on the screen collection, as expected.
        assert_eq!(main.events(), [FakeEvent::Read { ms: 0 }]);
    }

    // Draining and observing.

    #[test]
    fn drain_discards_stale_input_on_every_collection() {
        let (main, screen, mut device) = pair();
        main.queue_input(&[1]);
        main.queue_input(&[2]);
        screen.queue_input(&[3]);
        device.drain().unwrap();
        assert_eq!(main.pending_input(), 0);
        assert_eq!(screen.pending_input(), 0);
    }

    #[test]
    fn drain_doesnt_wait() {
        let fake = FakeTransport::new();
        device(&fake).drain().unwrap();
        assert_eq!(fake.events(), [FakeEvent::Read { ms: 0 }]);
    }

    #[test]
    fn drain_stops_even_if_input_never_ends() {
        let fake = FakeTransport::new();
        for _ in 0..1000 {
            fake.queue_input(&[1]);
        }
        device(&fake).drain().unwrap();
        assert!(fake.pending_input() > 0);
    }

    #[test]
    fn on_packet_sees_sends_and_acks() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Reject);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let mut device = device(&fake);
        device.on_packet = Some(Box::new(move |event| {
            log.lock().unwrap().push(match event {
                PacketEvent::Sent { collection, report } => (0, collection as u8, report[4]),
                PacketEvent::Ack { idx, accepted, .. } => (1, accepted as u8, idx),
            });
        }));
        device.send_screen_chunk(&chunk(7)).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [(0, 0, 7), (1, 0, 7), (0, 0, 7), (1, 1, 7)]
        );
    }

    #[test]
    fn a_device_can_move_to_a_worker_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<Device<FakeTransport>>();
        assert_send::<Device>();
    }

    #[test]
    fn every_flow_here_uses_allowed_commands() {
        let fake = FakeTransport::with_auto_ack();
        fake.queue_feature_reply(&reply(0x84, &[0; 128], true));
        let mut device = device(&fake);
        device.send_generic(cmd::CONFIG, 0, &[0; 128]).unwrap();
        device.read_block(cmd::CONFIG_READ, 0, 128).unwrap();
        device.send_screen_chunk(&chunk(1)).unwrap();
        device.send_screen_control(&control()).unwrap();
        assert_eq!(fake.sent().len(), 4);
        fake.assert_allowed_commands_only();
    }
}
