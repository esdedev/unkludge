//! A keyboard that exists only in memory, for tests. It records what it receives, never sleeps,
//! and can be told to misbehave.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::Transport;
use crate::Error;
use crate::packet::{REPORT_ID, REPORT_LEN, cmd};

/// Something that happened on a [`FakeTransport`], in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeEvent {
    Sent(Vec<u8>),
    SendFailed(Vec<u8>),
    GetFeature,
    Read { ms: i32 },
    Sleep(Duration),
}

/// How an automatic ack goes wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckFault {
    /// No ack at all.
    Drop,
    /// An ack with status 0.
    Reject,
    /// An ack for another chunk.
    WrongIdx,
    /// An ack with another check byte.
    WrongCheck,
}

#[derive(Default)]
struct State {
    events: Vec<FakeEvent>,
    feature_replies: VecDeque<Vec<u8>>,
    input: VecDeque<Vec<u8>>,
    auto_ack: bool,
    acks_made: usize,
    ack_faults: BTreeMap<usize, AckFault>,
    sends_tried: usize,
    failing_sends: BTreeSet<usize>,
    disconnected: bool,
}

/// An in-memory [`Transport`]. Clones share one state, so a test can keep a clone to look at what
/// happened after handing another to a `Device`.
#[derive(Clone, Default)]
pub struct FakeTransport {
    state: Arc<Mutex<State>>,
}

impl FakeTransport {
    /// A keyboard that says nothing unless told what to say.
    pub fn new() -> Self {
        Self::default()
    }

    /// A keyboard that acknowledges each screen report the way real ones do. Other commands get
    /// no ack, as on the real thing.
    pub fn with_auto_ack() -> Self {
        let fake = Self::new();
        fake.lock().auto_ack = true;
        fake
    }

    /// Queues the reply to the next `get_feature`. An empty queue makes it fail.
    pub fn queue_feature_reply(&self, reply: &[u8]) {
        self.lock().feature_replies.push_back(reply.to_vec());
    }

    /// Queues an input report for `read_timeout`.
    pub fn queue_input(&self, report: &[u8]) {
        self.lock().input.push_back(report.to_vec());
    }

    /// Makes automatic ack number `nth`, counting from 0, go wrong.
    pub fn ack_fault(&self, nth: usize, fault: AckFault) {
        self.lock().ack_faults.insert(nth, fault);
    }

    /// Makes send attempt number `nth`, counting from 0, fail.
    pub fn fail_send(&self, nth: usize) {
        self.lock().failing_sends.insert(nth);
    }

    /// Makes every call from now on fail, as when the keyboard is unplugged.
    pub fn disconnect(&self) {
        self.lock().disconnected = true;
    }

    /// Everything that happened so far, in order.
    pub fn events(&self) -> Vec<FakeEvent> {
        self.lock().events.clone()
    }

    /// The reports that were sent successfully, in order.
    pub fn sent(&self) -> Vec<Vec<u8>> {
        self.lock()
            .events
            .iter()
            .filter_map(|event| match event {
                FakeEvent::Sent(report) => Some(report.clone()),
                _ => None,
            })
            .collect()
    }

    /// The pauses that would have happened on a real device, in order.
    pub fn sleeps(&self) -> Vec<Duration> {
        self.lock()
            .events
            .iter()
            .filter_map(|event| match event {
                FakeEvent::Sleep(duration) => Some(*duration),
                _ => None,
            })
            .collect()
    }

    /// How many queued input reports are still unread.
    pub fn pending_input(&self) -> usize {
        self.lock().input.len()
    }

    /// The reports, attempted or sent, that aren't 520 bytes with report ID 9 and a command from
    /// the allow-list.
    pub fn disallowed_reports(&self) -> Vec<Vec<u8>> {
        self.lock()
            .events
            .iter()
            .filter_map(|event| match event {
                FakeEvent::Sent(report) | FakeEvent::SendFailed(report) => Some(report),
                _ => None,
            })
            .filter(|report| !is_allowed(report))
            .cloned()
            .collect()
    }

    /// Fails the test if anything outside the allow-list was attempted on this transport.
    #[track_caller]
    pub fn assert_allowed_commands_only(&self) {
        let bad = self.disallowed_reports();
        let shown: Vec<&[u8]> = bad.iter().map(|r| &r[..r.len().min(8)]).collect();
        assert!(
            bad.is_empty(),
            "reports outside the allow-list: {shown:02X?}"
        );
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }
}

fn is_allowed(report: &[u8]) -> bool {
    report.len() == REPORT_LEN && report[0] == REPORT_ID && cmd::is_allowed(report[1])
}

impl State {
    fn check_connected(&self) -> Result<(), Error> {
        if self.disconnected {
            Err(Error::Hid("the fake device is disconnected".to_string()))
        } else {
            Ok(())
        }
    }

    /// Answers a screen report with the ack a real keyboard would give, unless told otherwise.
    fn auto_ack(&mut self, report: &[u8]) {
        let command = report.get(1).copied();
        let (mut idx, mut check) = if command == Some(cmd::SCREEN_DATA.byte()) {
            (report[4], report[5])
        } else if command == Some(cmd::SCREEN_CONTROL.byte()) {
            (0, 0)
        } else {
            return;
        };
        let mut status = 1;

        let nth = self.acks_made;
        self.acks_made += 1;
        match self.ack_faults.get(&nth) {
            Some(AckFault::Drop) => return,
            Some(AckFault::Reject) => status = 0,
            Some(AckFault::WrongIdx) => idx = idx.wrapping_add(1),
            Some(AckFault::WrongCheck) => check = check.wrapping_add(1),
            None => {}
        }
        self.input
            .push_back(vec![REPORT_ID, 0x0A, 0x06, status, idx, check, 0, 0]);
    }
}

fn copy_into(buf: &mut [u8], data: &[u8]) -> usize {
    let len = buf.len().min(data.len());
    buf[..len].copy_from_slice(&data[..len]);
    len
}

impl Transport for FakeTransport {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        let mut state = self.lock();
        state.check_connected()?;

        let nth = state.sends_tried;
        state.sends_tried += 1;
        if state.failing_sends.contains(&nth) {
            state.events.push(FakeEvent::SendFailed(report.to_vec()));
            return Err(Error::Hid(format!("the fake failed send {nth}")));
        }

        state.events.push(FakeEvent::Sent(report.to_vec()));
        if state.auto_ack {
            state.auto_ack(report);
        }
        Ok(())
    }

    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        let mut state = self.lock();
        state.check_connected()?;
        state.events.push(FakeEvent::GetFeature);
        let reply = state
            .feature_replies
            .pop_front()
            .ok_or_else(|| Error::Hid("the fake has no reply queued".to_string()))?;
        Ok(copy_into(buf, &reply))
    }

    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error> {
        let mut state = self.lock();
        state.check_connected()?;
        state.events.push(FakeEvent::Read { ms });
        Ok(state
            .input
            .pop_front()
            .map_or(0, |report| copy_into(buf, &report)))
    }

    fn sleep(&self, duration: Duration) {
        self.lock().events.push(FakeEvent::Sleep(duration));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{build, screen_chunk};

    #[test]
    fn it_records_sends_and_never_really_sleeps() {
        let fake = FakeTransport::new();
        fake.sleep(Duration::from_secs(3600));
        fake.send_feature(&[1, 2, 3]).unwrap();
        assert_eq!(
            fake.events(),
            [
                FakeEvent::Sleep(Duration::from_secs(3600)),
                FakeEvent::Sent(vec![1, 2, 3])
            ]
        );
        assert_eq!(fake.sent(), [vec![1, 2, 3]]);
        assert_eq!(fake.sleeps(), [Duration::from_secs(3600)]);
    }

    #[test]
    fn clones_share_their_state() {
        let fake = FakeTransport::new();
        fake.clone().send_feature(&[1]).unwrap();
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn get_feature_replies_from_the_queue() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&[9, 8, 7]);
        let mut buf = [0; 8];
        assert_eq!(fake.get_feature(&mut buf), Ok(3));
        assert_eq!(&buf[..3], &[9, 8, 7]);
        assert!(matches!(fake.get_feature(&mut buf), Err(Error::Hid(_))));
    }

    #[test]
    fn read_timeout_returns_zero_when_nothing_is_queued() {
        let fake = FakeTransport::new();
        fake.queue_input(&[5, 6]);
        let mut buf = [0; 8];
        assert_eq!(fake.read_timeout(&mut buf, 0), Ok(2));
        assert_eq!(fake.read_timeout(&mut buf, 0), Ok(0));
    }

    #[test]
    fn the_nth_send_can_fail() {
        let fake = FakeTransport::new();
        fake.fail_send(1);
        assert!(fake.send_feature(&[1]).is_ok());
        assert!(fake.send_feature(&[2]).is_err());
        assert!(fake.send_feature(&[3]).is_ok());
        assert_eq!(fake.sent(), [vec![1], vec![3]]);
    }

    #[test]
    fn auto_ack_answers_screen_reports_only() {
        let fake = FakeTransport::with_auto_ack();
        let mut buf = [0; 8];

        fake.send_feature(&build(cmd::CLOCK, 0, 1, 0, &[0; 14]))
            .unwrap();
        assert_eq!(fake.pending_input(), 0);

        fake.send_feature(&screen_chunk(1, 0x73, &[1, 2, 3], true))
            .unwrap();
        assert_eq!(fake.read_timeout(&mut buf, 0), Ok(8));
        assert_eq!(buf, [0x09, 0x0A, 0x06, 0x01, 0x73, 0xF9, 0, 0]);

        fake.send_feature(&build(cmd::SCREEN_CONTROL, 0, 1, 0, &[0; 5]))
            .unwrap();
        assert_eq!(fake.read_timeout(&mut buf, 0), Ok(8));
        assert_eq!(buf, [0x09, 0x0A, 0x06, 0x01, 0, 0, 0, 0]);
    }

    #[test]
    fn the_nth_ack_can_go_wrong() {
        let chunk = screen_chunk(0, 7, &[1], true);
        let check = chunk[5];
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Drop);
        fake.ack_fault(1, AckFault::Reject);
        fake.ack_fault(2, AckFault::WrongIdx);
        fake.ack_fault(3, AckFault::WrongCheck);
        let mut buf = [0; 8];

        fake.send_feature(&chunk).unwrap();
        assert_eq!(fake.pending_input(), 0);

        fake.send_feature(&chunk).unwrap();
        fake.read_timeout(&mut buf, 0).unwrap();
        assert_eq!(buf[3..6], [0, 7, check]);

        fake.send_feature(&chunk).unwrap();
        fake.read_timeout(&mut buf, 0).unwrap();
        assert_eq!(buf[3..6], [1, 8, check]);

        fake.send_feature(&chunk).unwrap();
        fake.read_timeout(&mut buf, 0).unwrap();
        assert_eq!(buf[3..6], [1, 7, check.wrapping_add(1)]);

        fake.send_feature(&chunk).unwrap();
        fake.read_timeout(&mut buf, 0).unwrap();
        assert_eq!(buf[3..6], [1, 7, check]);
    }

    #[test]
    fn disconnect_makes_every_call_fail() {
        let fake = FakeTransport::new();
        fake.disconnect();
        let mut buf = [0; 8];
        assert!(fake.send_feature(&[1]).is_err());
        assert!(fake.get_feature(&mut buf).is_err());
        assert!(fake.read_timeout(&mut buf, 0).is_err());
    }

    #[test]
    fn the_allow_list_helper_passes_every_allowed_command() {
        let fake = FakeTransport::new();
        for command in cmd::ALLOWED {
            fake.send_feature(&build(command, 0, 1, 0, &[])).unwrap();
        }
        assert!(fake.disallowed_reports().is_empty());
        fake.assert_allowed_commands_only();
    }

    fn report(id: u8, command: u8) -> Vec<u8> {
        let mut report = vec![0; REPORT_LEN];
        report[0] = id;
        report[1] = command;
        report
    }

    #[test]
    fn the_allow_list_helper_finds_forbidden_reports() {
        // Factory reset, stored URL, an unused command, the updater's report IDs, a wrong length.
        let bad = [
            report(0x09, 0x11),
            report(0x09, 0x12),
            report(0x09, 0x03),
            report(0x05, 0x04),
            report(0x06, 0x04),
            vec![0x09, 0x04],
            vec![],
        ];
        let fake = FakeTransport::new();
        for report in &bad {
            fake.send_feature(report).unwrap();
        }
        assert_eq!(fake.disallowed_reports(), bad);
    }

    #[test]
    fn the_allow_list_helper_counts_failed_attempts() {
        let fake = FakeTransport::new();
        fake.fail_send(0);
        let _ = fake.send_feature(&report(0x09, 0x11));
        assert_eq!(fake.disallowed_reports().len(), 1);
    }

    #[test]
    #[should_panic(expected = "outside the allow-list")]
    fn the_allow_list_assertion_fails_a_test() {
        let fake = FakeTransport::new();
        fake.send_feature(&report(0x09, 0x11)).unwrap();
        fake.assert_allowed_commands_only();
    }
}
