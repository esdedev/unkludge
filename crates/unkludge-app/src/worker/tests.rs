//! The worker on the in-memory keyboard. Time only passes when a test says so, and nothing here
//! opens a real device.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::AtomicUsize;

use image::{ImageFormat, Rgba, RgbaImage};
use unkludge_protocol::config::{CONFIG_LEN, SleepTime};
use unkludge_protocol::lighting::{ColorChoice, ColorSetting, LightMode, ModeChoice, Rgb};
use unkludge_protocol::model;
use unkludge_protocol::screen::LoadOptions;
use unkludge_protocol::transport::fake::{AckFault, FakeTransport};
use unkludge_shared::device_lock::{DeviceLock, Place, open_locked_at};

use super::timing::DEFAULT_CHUNK_TIME;
use super::*;

const CONFIG_WRITE: u8 = 0x04;
const CONFIG_READ: u8 = 0x84;
const PALETTES_WRITE: u8 = 0x0A;
const CLOCK: u8 = 0x0B;
const SCREEN_DATA: u8 = 0x0C;
const SCREEN_CONTROL: u8 = 0x0D;
/// Where a config write's block starts in its report.
const HEADER: usize = 8;
const EFFECT_5_BRIGHTNESS: usize = 0x38 + 10;
const EFFECT_5_SPEED_COLOR: usize = 0x39 + 10;
/// A still image on the S85: the start, 215 chunks and the finish.
const S85_STEPS: usize = 217;

fn s85() -> &'static Model {
    model::find(0x0265).unwrap()
}

/// A block showing effect 5 at brightness 8, speed 2 and slot 3.
fn block() -> [u8; CONFIG_LEN] {
    let mut bytes = [0; CONFIG_LEN];
    bytes[0x0A] = 5;
    bytes[0x18] = 0x14;
    bytes[EFFECT_5_BRIGHTNESS] = 8;
    bytes[EFFECT_5_SPEED_COLOR] = 0x23;
    bytes[0x7E..].copy_from_slice(&[0x5A, 0xA5]);
    bytes
}

fn reply(block: &[u8; CONFIG_LEN]) -> Vec<u8> {
    let mut reply = vec![0x09, CONFIG_READ, 0, 0, 1, 0, 0x80, 0];
    reply.extend(block);
    reply
}

fn queue_reads(fake: &FakeTransport, count: usize) {
    for _ in 0..count {
        fake.queue_feature_reply(&reply(&block()));
    }
}

fn current() -> CurrentLighting {
    CurrentLighting {
        mode: LightMode::Effect(5),
        brightness: Some(8),
        speed: Some(2),
        color: Some(ColorSetting::Slot(3)),
        sleep: unkludge_protocol::lighting::SleepSetting::Known(SleepTime::Min10),
    }
}

fn commands(fake: &FakeTransport) -> Vec<u8> {
    fake.sent().iter().map(|report| report[1]).collect()
}

fn count(fake: &FakeTransport, command: u8) -> usize {
    commands(fake).iter().filter(|&&c| c == command).count()
}

fn written_block(report: &[u8]) -> &[u8] {
    &report[HEADER..HEADER + CONFIG_LEN]
}

fn config_writes(fake: &FakeTransport) -> Vec<Vec<u8>> {
    fake.sent()
        .into_iter()
        .filter(|report| report[1] == CONFIG_WRITE)
        .collect()
}

/// What the factory hands out on each try, in order. Once these run out, no keyboard is found.
type Opens = VecDeque<Result<FakeTransport, Error>>;
type Open = Box<dyn FnMut() -> Result<Device<FakeTransport>, Error> + Send>;

struct Rig {
    cmds: Sender<Cmd>,
    events: Receiver<Event>,
    worker: Worker<FakeTransport, Open>,
    opens_tried: Arc<AtomicUsize>,
    start: Instant,
    /// What the worker's own clock says. Steps set it.
    clock: Arc<Mutex<Instant>>,
}

impl Rig {
    fn with_opens(opens: Opens, auto_clock: bool, notify: Notify) -> Rig {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let opens_tried = Arc::new(AtomicUsize::new(0));
        let tried = Arc::clone(&opens_tried);
        let mut opens = opens;
        let open: Open = Box::new(move || {
            tried.fetch_add(1, Ordering::SeqCst);
            let fake = opens.pop_front().unwrap_or(Err(Error::NotFound))?;
            Device::new(fake, None, s85())
        });
        let options = Options {
            auto_clock,
            palettes: BTreeMap::new(),
            rk_app_running: Box::new(|| false),
        };
        let emitter = Emitter {
            events: event_tx,
            notify,
        };
        let mut worker = Worker::new(open, cmd_rx, options, emitter, Arc::default());
        let start = Instant::now();
        let clock = Arc::new(Mutex::new(start));
        let reading = Arc::clone(&clock);
        worker.clock = Arc::new(move || *reading.lock().unwrap());
        Rig {
            cmds: cmd_tx,
            events: event_rx,
            worker,
            opens_tried,
            start,
            clock,
        }
    }

    /// A keyboard that's there on the first look, with the worker's clock running on the fake's
    /// own time, so that the pauses and waits it skips count as they would.
    fn measuring(fake: &FakeTransport) -> Rig {
        let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), false, Arc::new(|| {}));
        let fake_time = fake.clone();
        rig.worker.clock = Arc::new(move || fake_time.now());
        fake.queue_feature_reply(&reply(&block()));
        rig.step(0);
        rig.drain();
        rig
    }

    /// A keyboard opened as the real one is, with the lock first.
    fn locked(fake: &FakeTransport, place: Place) -> Rig {
        let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
        let fake = fake.clone();
        rig.worker.open = Box::new(move || {
            open_locked_at(&place, Duration::ZERO, || {
                Device::new(fake.clone(), None, s85())
            })
        });
        rig
    }

    /// A keyboard that's there on the first look, with the clock left alone.
    fn connected(fake: &FakeTransport) -> Rig {
        Rig::connected_with(fake, false, Arc::new(|| {}))
    }

    fn connected_with(fake: &FakeTransport, auto_clock: bool, notify: Notify) -> Rig {
        let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), auto_clock, notify);
        fake.queue_feature_reply(&reply(&block()));
        rig.step(0);
        rig.drain();
        rig
    }

    fn step(&mut self, ms: u64) -> Duration {
        let now = self.start + Duration::from_millis(ms);
        *self.clock.lock().unwrap() = now;
        self.worker.step(now)
    }

    /// As the interface would have it, sharing the rig's state but with no thread behind it.
    fn handle(&self) -> WorkerHandle {
        WorkerHandle {
            cmds: self.cmds.clone(),
            shared: Arc::clone(&self.worker.shared),
            emitter: self.worker.emitter.clone(),
            thread: None,
            beat: Arc::default(),
            shutdown_wait: Duration::ZERO,
        }
    }

    fn send(&self, cmd: Cmd) {
        self.cmds.send(cmd).unwrap();
    }

    fn drain(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    fn opens_tried(&self) -> usize {
        self.opens_tried.load(Ordering::SeqCst)
    }
}

fn brightness(value: u8) -> LightingChange {
    LightingChange {
        brightness: Some(value),
        ..LightingChange::default()
    }
}

fn speed(value: u8) -> LightingChange {
    LightingChange {
        speed: Some(value),
        ..LightingChange::default()
    }
}

fn png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn still() -> Arc<Animation> {
    let spec = s85().screen.unwrap();
    Arc::new(Animation::from_bytes(&png(), &spec, LoadOptions::default()).unwrap())
}

fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("unkludge-worker-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn gif(frames: usize) -> Vec<u8> {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame};

    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        for value in 0..frames {
            let image = RgbaImage::from_pixel(8, 8, Rgba([value as u8, 0, 0, 255]));
            let delay = Delay::from_numer_denom_ms(100, 1);
            encoder
                .encode_frame(Frame::from_parts(image, 0, 0, delay))
                .unwrap();
        }
    }
    bytes
}

/// Leaves out the progress of an upload, which has hundreds of steps.
fn without_progress(events: Vec<Event>) -> Vec<Event> {
    events
        .into_iter()
        .filter(|event| !matches!(event, Event::Progress { .. }))
        .collect()
}

fn started(total: usize) -> impl Fn(&Event) -> bool {
    move |event| matches!(event, Event::UploadStarted { total: t, .. } if *t == total)
}

fn bad_read(fake: &FakeTransport) {
    // Config reads try five times.
    for _ in 0..5 {
        fake.queue_feature_reply(&reply(&[0; CONFIG_LEN]));
    }
}

fn is_bad_block(event: &Event) -> bool {
    matches!(event, Event::Error(Problem::Device(Error::Invalid(_))))
}

/// A notify that pulls the plug on the keyboard after this many events.
fn unplug_after(fake: &FakeTransport, events: usize) -> Notify {
    let unplugged = fake.clone();
    let seen = AtomicUsize::new(0);
    Arc::new(move || {
        if seen.fetch_add(1, Ordering::SeqCst) == events {
            unplugged.disconnect();
        }
    })
}

/// Asks for a report and gives back what it says.
fn report_of(rig: &mut Rig, ms: u64) -> ReportData {
    rig.send(Cmd::Report);
    rig.step(ms);
    let events = rig.drain();
    let Some(Event::ReportReady(data)) = events
        .into_iter()
        .rfind(|event| matches!(event, Event::ReportReady(_)))
    else {
        panic!("no report");
    };
    data
}

// Connecting

#[test]
fn a_keyboard_appearing_gives_connected_then_its_lighting() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 1);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), false, Arc::new(|| {}));
    rig.step(0);
    assert_eq!(
        rig.drain(),
        [
            Event::RkAppRunning(false),
            Event::Connected { model: s85() },
            Event::Lighting(current()),
        ]
    );
    assert_eq!(commands(&fake), [CONFIG_READ]);
}

#[test]
fn the_clock_is_set_on_connecting_only_with_auto_clock_on() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 1);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), true, Arc::new(|| {}));
    rig.step(0);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));
    assert_eq!(commands(&fake), [CONFIG_READ, CLOCK]);

    let fake = FakeTransport::new();
    queue_reads(&fake, 1);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), false, Arc::new(|| {}));
    rig.step(0);
    assert!(!rig.drain().contains(&Event::ClockSynced));
    assert_eq!(commands(&fake), [CONFIG_READ]);
}

#[test]
fn the_clock_is_the_local_time() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 1);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), true, Arc::new(|| {}));
    rig.step(0);
    let sent = fake.sent();
    let clock = sent.iter().find(|report| report[1] == CLOCK).unwrap();
    let year = u16::from_le_bytes([clock[HEADER + 3], clock[HEADER + 4]]);
    assert_eq!(year, device_now().unwrap().year);
}

#[test]
fn no_keyboard_is_told_once_and_looked_for_at_the_polling_interval() {
    let mut rig = Rig::with_opens(VecDeque::new(), true, Arc::new(|| {}));
    assert_eq!(rig.step(0), RECONNECT_INTERVAL);
    assert_eq!(
        rig.drain(),
        [Event::RkAppRunning(false), Event::Disconnected]
    );
    assert_eq!(rig.opens_tried(), 1);

    // Not due yet.
    assert_eq!(rig.step(1500), Duration::from_millis(500));
    assert_eq!(rig.opens_tried(), 1);

    assert_eq!(rig.step(2000), RECONNECT_INTERVAL);
    assert_eq!(rig.opens_tried(), 2);
    assert_eq!(rig.drain(), []);
}

#[test]
fn a_keyboard_that_wont_open_is_told_once_per_error() {
    let denied = || Err(Error::Hid("permission denied".to_string()));
    let opens = VecDeque::from([denied(), denied(), Err(Error::NotFound)]);
    let mut rig = Rig::with_opens(opens, false, Arc::new(|| {}));
    rig.step(0);
    let error = Error::Hid("permission denied".to_string());
    assert_eq!(
        rig.drain(),
        [Event::RkAppRunning(false), Event::CannotOpen(error.clone())]
    );
    rig.step(2000);
    assert_eq!(rig.drain(), []);
    // Unplugged: the message has to go away.
    rig.step(4000);
    assert_eq!(rig.drain(), [Event::Disconnected]);
}

/// What another program sees.
fn take_from_elsewhere(place: &Place) -> Result<(), Error> {
    DeviceLock::take(place, Duration::ZERO).map(drop)
}

#[test]
fn the_worker_holds_the_lock_for_as_long_as_it_has_the_keyboard() {
    let place = Place::scratch("worker-holds");
    let fake = FakeTransport::new();
    queue_reads(&fake, 1);
    let mut rig = Rig::locked(&fake, place.clone());
    rig.step(0);
    assert!(rig.drain().contains(&Event::Connected { model: s85() }));
    assert_eq!(take_from_elsewhere(&place), Err(Error::DeviceBusy));

    // Polls don't let go of it.
    queue_reads(&fake, 1);
    rig.step(REFRESH_MS);
    assert_eq!(take_from_elsewhere(&place), Err(Error::DeviceBusy));

    fake.disconnect();
    rig.step(2 * REFRESH_MS);
    assert!(rig.drain().contains(&Event::Disconnected));
    assert_eq!(take_from_elsewhere(&place), Ok(()));
}

/// Makes the next report's three tries fail, counting the send attempts so far.
fn refuse_next_send(fake: &FakeTransport) {
    let attempts = send_attempts(fake);
    for attempt in attempts..attempts + 3 {
        fake.fail_send(attempt);
    }
}

fn send_attempts(fake: &FakeTransport) -> usize {
    use unkludge_protocol::transport::fake::FakeEvent;
    fake.events()
        .iter()
        .filter(|event| matches!(event, FakeEvent::Sent(_) | FakeEvent::SendFailed(_)))
        .count()
}

/// How many times this command was tried, with each report given three tries at most.
fn tried(fake: &FakeTransport, command: u8) -> usize {
    use unkludge_protocol::transport::fake::FakeEvent;
    fake.events()
        .iter()
        .filter(|event| match event {
            FakeEvent::Sent(report) | FakeEvent::SendFailed(report) => report[1] == command,
            _ => false,
        })
        .count()
}

fn is_usb_error(event: &Event) -> bool {
    matches!(event, Event::Error(Problem::Device(Error::Hid(_))))
}

#[test]
fn a_clock_the_keyboard_refuses_isnt_an_unplug_and_isnt_sent_again() {
    let fake = FakeTransport::new();
    // The read on connecting works, and so does every read after it.
    queue_reads(&fake, 10);
    fake.fail_send(1);
    fake.fail_send(2);
    fake.fail_send(3);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), true, Arc::new(|| {}));
    rig.step(0);
    let events = rig.drain();
    assert_eq!(events[1], Event::Connected { model: s85() });
    assert_eq!(events[2], Event::Lighting(current()));
    assert!(is_usb_error(&events[3]), "{events:?}");
    assert_eq!(events.len(), 4, "{events:?}");
    assert_eq!(tried(&fake, CLOCK), 3, "one report, with its three tries");

    // Time goes by, and there's no loop: no new connection and no new clock.
    for look in 1..=3 {
        rig.step(look * REFRESH_MS);
        assert_eq!(rig.drain(), [], "look {look}");
    }
    assert_eq!(rig.opens_tried(), 1);
    assert_eq!(tried(&fake, CLOCK), 3);
    assert!(rig.worker.device.is_some());
}

#[test]
fn asking_for_the_clock_after_a_refusal_is_one_attempt_and_shows_the_error() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 10);
    let mut rig = Rig::connected(&fake);
    refuse_next_send(&fake);
    let before = send_attempts(&fake);
    rig.send(Cmd::SyncClock);
    rig.step(1);
    let events = rig.drain();
    assert!(is_usb_error(&events[0]), "{events:?}");
    assert_eq!(events.len(), 1, "{events:?}");
    // The clock's three tries and the one read.
    assert_eq!(send_attempts(&fake), before + 4);

    // The next one gets through.
    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
}

#[test]
fn waking_up_tries_a_refused_clock_once_more() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 10);
    fake.fail_send(1);
    fake.fail_send(2);
    fake.fail_send(3);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), true, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
    assert_eq!(rig.opens_tried(), 1);
}

#[test]
fn a_light_write_the_keyboard_refuses_isnt_an_unplug_and_isnt_sent_again() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 10);
    let mut rig = Rig::connected(&fake);
    let before = send_attempts(&fake);
    // The write comes after the change's own read.
    for attempt in before + 1..before + 4 {
        fake.fail_send(attempt);
    }
    rig.send(Cmd::SetLighting(brightness(4)));
    rig.step(1);
    let events = rig.drain();
    assert!(is_usb_error(&events[0]), "{events:?}");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        tried(&fake, CONFIG_WRITE),
        3,
        "one report, with its three tries"
    );
    assert!(rig.worker.device.is_some());

    for look in 1..=2 {
        rig.step(look * REFRESH_MS);
        assert_eq!(rig.drain(), [], "look {look}");
    }
    assert_eq!(tried(&fake, CONFIG_WRITE), 3);
    assert_eq!(rig.opens_tried(), 1);
}

#[test]
fn a_write_that_fails_with_the_read_after_it_failing_too_is_an_unplug() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    fake.disconnect();
    rig.send(Cmd::SyncClock);
    rig.step(1);
    let events = rig.drain();
    assert!(is_usb_error(&events[0]), "{events:?}");
    assert_eq!(events[1], Event::Disconnected);
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(rig.worker.device.is_none());
}

#[test]
fn a_refused_write_with_a_bad_block_after_it_stays_connected() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    refuse_next_send(&fake);
    bad_read(&fake);
    rig.send(Cmd::SyncClock);
    rig.step(1);
    let events = rig.drain();
    assert!(is_usb_error(&events[0]), "{events:?}");
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(rig.worker.device.is_some());
}

#[test]
fn a_keyboard_a_command_has_isnt_an_unplug_and_gets_looked_at_again() {
    let place = Place::scratch("worker-busy");
    let holder = DeviceLock::take(&place, Duration::ZERO).unwrap();
    let fake = FakeTransport::new();
    let mut rig = Rig::locked(&fake, place);
    rig.step(0);
    assert_eq!(
        rig.drain(),
        [
            Event::RkAppRunning(false),
            Event::CannotOpen(Error::DeviceBusy)
        ]
    );
    assert!(fake.sent().is_empty(), "nothing goes out without the lock");

    // Told once, however long it lasts.
    rig.step(2000);
    assert_eq!(rig.drain(), []);
    assert!(fake.sent().is_empty());

    drop(holder);
    queue_reads(&fake, 1);
    rig.step(4000);
    assert_eq!(
        rig.drain(),
        [
            Event::Connected { model: s85() },
            Event::Lighting(current())
        ]
    );
}

#[test]
fn a_keyboard_with_no_usable_collection_is_told_once_and_not_as_no_keyboard() {
    let none = || Err(Error::NoUsableCollection(0x0265));
    let mut rig = Rig::with_opens(VecDeque::from([none(), none()]), true, Arc::new(|| {}));
    rig.step(0);
    assert_eq!(
        rig.drain(),
        [
            Event::RkAppRunning(false),
            Event::CannotOpen(Error::NoUsableCollection(0x0265))
        ]
    );
    rig.step(2000);
    assert_eq!(rig.drain(), []);
    assert_eq!(rig.opens_tried(), 2);
}

#[test]
fn a_first_read_that_fails_on_the_usb_level_doesnt_connect() {
    // The fake has no reply queued, which is an I/O error.
    let fake = FakeTransport::new();
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake)]), true, Arc::new(|| {}));
    rig.step(0);
    let events = rig.drain();
    assert!(
        matches!(events[1], Event::CannotOpen(Error::Hid(_))),
        "{events:?}"
    );
    assert!(!events.iter().any(|e| matches!(e, Event::Connected { .. })));
}

#[test]
fn a_block_without_its_end_marker_connects_but_gives_no_lighting_and_no_writes() {
    let fake = FakeTransport::with_auto_ack();
    bad_read(&fake);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), true, Arc::new(|| {}));
    rig.step(0);
    let events = rig.drain();
    assert_eq!(events[1], Event::Connected { model: s85() });
    assert!(is_bad_block(&events[2]), "{events:?}");
    assert_eq!(events.len(), 3);

    // Nothing can be written, and each command says why.
    for cmd in [
        Cmd::SyncClock,
        Cmd::SetLighting(brightness(4)),
        Cmd::UploadScreen(still()),
    ] {
        rig.send(cmd);
        rig.step(1);
        assert_eq!(rig.drain(), [Event::Error(Problem::UnusableConfig)]);
    }
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.step(1);
    rig.step(400);
    assert_eq!(rig.drain(), [Event::Error(Problem::UnusableConfig)]);

    // Only the reads went out.
    assert!(commands(&fake).iter().all(|&c| c == CONFIG_READ));
    assert_eq!(count(&fake, CONFIG_READ), 5);
}

#[test]
fn a_block_that_comes_good_lets_writes_through_with_the_clock_that_was_due() {
    let fake = FakeTransport::new();
    bad_read(&fake);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), true, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    queue_reads(&fake, 2);
    rig.step(REFRESH_MS);
    assert_eq!(
        rig.drain(),
        [Event::Lighting(current()), Event::ClockSynced]
    );
    assert_eq!(count(&fake, CLOCK), 1);

    rig.send(Cmd::SetLighting(brightness(4)));
    rig.step(REFRESH_MS + 1);
    assert_eq!(config_writes(&fake).len(), 1);
}

#[test]
fn a_block_that_comes_good_doesnt_set_the_clock_with_auto_clock_off() {
    let fake = FakeTransport::new();
    bad_read(&fake);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    queue_reads(&fake, 1);
    rig.step(REFRESH_MS);
    assert_eq!(rig.drain(), [Event::Lighting(current())]);
    assert_eq!(count(&fake, CLOCK), 0);
}

#[test]
fn a_block_gone_bad_on_a_poll_is_told_once_and_stops_writes() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    bad_read(&fake);
    bad_read(&fake);
    rig.step(REFRESH_MS);
    let events = rig.drain();
    assert_eq!(events.len(), 1);
    assert!(is_bad_block(&events[0]), "{events:?}");
    rig.step(2 * REFRESH_MS);
    assert_eq!(rig.drain(), []);

    rig.send(Cmd::SyncClock);
    rig.step(2 * REFRESH_MS + 1);
    assert_eq!(rig.drain(), [Event::Error(Problem::UnusableConfig)]);
    assert_eq!(count(&fake, CLOCK), 0);
}

#[test]
fn a_block_bad_since_connecting_isnt_told_again_at_the_next_poll() {
    let fake = FakeTransport::new();
    bad_read(&fake);
    bad_read(&fake);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    rig.step(REFRESH_MS);
    assert_eq!(rig.drain(), []);
}

#[test]
fn a_block_that_goes_bad_again_after_coming_good_is_told_again() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    bad_read(&fake);
    rig.step(REFRESH_MS);
    rig.drain();
    queue_reads(&fake, 1);
    rig.step(2 * REFRESH_MS);
    assert_eq!(rig.drain(), []);
    bad_read(&fake);
    rig.step(3 * REFRESH_MS);
    let events = rig.drain();
    assert_eq!(events.len(), 1);
    assert!(is_bad_block(&events[0]), "{events:?}");
}

#[test]
fn refresh_finding_a_bad_block_stops_writes_too() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    bad_read(&fake);
    rig.send(Cmd::Refresh);
    rig.step(1);
    let events = rig.drain();
    assert!(is_bad_block(&events[0]), "{events:?}");
    assert_eq!(events.len(), 1, "it stays connected");
    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain(), [Event::Error(Problem::UnusableConfig)]);
}

#[test]
fn the_rk_app_is_told_when_it_changes() {
    let running = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&running);
    let (_cmds, cmd_rx) = channel();
    let (event_tx, events) = channel();
    let options = Options {
        auto_clock: false,
        palettes: BTreeMap::new(),
        rk_app_running: Box::new(move || seen.load(Ordering::SeqCst)),
    };
    let emitter = Emitter {
        events: event_tx,
        notify: Arc::new(|| {}),
    };
    let open = || -> Result<Device<FakeTransport>, Error> { Err(Error::NotFound) };
    let mut worker = Worker::new(open, cmd_rx, options, emitter, Arc::default());
    let start = Instant::now();
    worker.step(start);
    assert_eq!(events.try_recv(), Ok(Event::RkAppRunning(false)));
    assert_eq!(events.try_recv(), Ok(Event::Disconnected));

    worker.step(start + RECONNECT_INTERVAL);
    assert!(events.try_recv().is_err());

    running.store(true, Ordering::SeqCst);
    worker.step(start + 2 * RECONNECT_INTERVAL);
    assert_eq!(events.try_recv(), Ok(Event::RkAppRunning(true)));
}

// Losing the keyboard

#[test]
fn an_io_error_gives_disconnected_and_it_comes_back() {
    let first = FakeTransport::new();
    queue_reads(&first, 1);
    let second = FakeTransport::new();
    queue_reads(&second, 1);
    let opens = VecDeque::from([Ok(first.clone()), Ok(second.clone())]);
    let mut rig = Rig::with_opens(opens, true, Arc::new(|| {}));
    rig.step(0);
    rig.drain();

    first.disconnect();
    assert_eq!(rig.step(REFRESH_MS), RECONNECT_INTERVAL);
    assert_eq!(rig.drain(), [Event::Disconnected]);

    // Back after the polling interval, with its clock set again.
    rig.step(REFRESH_MS + 2000);
    assert_eq!(
        rig.drain(),
        [
            Event::Connected { model: s85() },
            Event::Lighting(current()),
            Event::ClockSynced
        ]
    );
    assert_eq!(commands(&second), [CONFIG_READ, CLOCK]);
}

const REFRESH_MS: u64 = 5000;

#[test]
fn losing_the_keyboard_forgets_what_was_waiting_for_it() {
    let first = FakeTransport::new();
    queue_reads(&first, 1);
    let second = FakeTransport::new();
    queue_reads(&second, 1);
    let opens = VecDeque::from([Ok(first.clone()), Ok(second.clone())]);
    let mut rig = Rig::with_opens(opens, false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();

    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.step(1);
    first.disconnect();
    rig.send(Cmd::SyncClock);
    rig.step(2);
    rig.drain();

    // Past the time the slider would have been written, on the keyboard that came next.
    rig.step(2002);
    rig.step(2500);
    assert_eq!(
        rig.drain(),
        [
            Event::Connected { model: s85() },
            Event::Lighting(current())
        ]
    );
    assert!(config_writes(&second).is_empty());
}

#[test]
fn with_the_keyboard_gone_its_model_is_gone_too() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    fake.disconnect();
    rig.step(REFRESH_MS);
    rig.drain();

    assert_eq!(report_of(&mut rig, REFRESH_MS + 1).model, None);
    rig.send(Cmd::LoadImage {
        path: PathBuf::from("nothing.gif"),
        fit: Fit::Cover,
        delay: None,
    });
    rig.step(REFRESH_MS + 2);
    assert_eq!(rig.drain(), [Event::Error(Problem::NoKeyboard)]);
}

#[test]
fn a_keyboard_that_comes_back_after_a_bad_read_has_its_lighting_told() {
    // Connected with this lighting, lost, back with a bad block, then the same lighting again: the
    // interface has been told nothing since it came back, so that isn't "no change".
    let first = FakeTransport::new();
    queue_reads(&first, 1);
    let second = FakeTransport::new();
    bad_read(&second);
    queue_reads(&second, 1);
    let opens = VecDeque::from([Ok(first.clone()), Ok(second.clone())]);
    let mut rig = Rig::with_opens(opens, false, Arc::new(|| {}));
    rig.step(0);
    first.disconnect();
    rig.step(REFRESH_MS);
    rig.step(REFRESH_MS + 2000);
    rig.drain();
    rig.step(REFRESH_MS + 7000);
    assert_eq!(rig.drain(), [Event::Lighting(current())]);
}

#[test]
fn a_command_that_finds_the_keyboard_gone_says_so_and_disconnects() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    fake.disconnect();
    rig.send(Cmd::SyncClock);
    rig.step(1);
    let events = rig.drain();
    assert!(
        matches!(events[0], Event::Error(Problem::Device(Error::Hid(_)))),
        "{events:?}"
    );
    assert_eq!(events[1], Event::Disconnected);
}

#[test]
fn commands_with_no_keyboard_are_refused() {
    let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    for cmd in [
        Cmd::SetLighting(brightness(4)),
        Cmd::AdjustLighting(brightness(4)),
        Cmd::SyncClock,
        Cmd::UploadScreen(still()),
        Cmd::LoadImage {
            path: PathBuf::from("nothing.gif"),
            fit: Fit::Cover,
            delay: None,
        },
    ] {
        rig.send(cmd);
        rig.step(1);
        assert_eq!(rig.drain(), [Event::Error(Problem::NoKeyboard)]);
    }
    assert_eq!(rig.opens_tried(), 1, "they don't look for one either");
}

#[test]
fn refresh_with_no_keyboard_looks_for_one_now() {
    let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
    rig.step(0);
    assert_eq!(rig.opens_tried(), 1);
    rig.send(Cmd::Refresh);
    rig.step(1);
    assert_eq!(rig.opens_tried(), 2);
}

// Polling

#[test]
fn polling_tells_of_lighting_only_when_it_changed() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);

    queue_reads(&fake, 1);
    assert_eq!(rig.step(REFRESH_MS), REFRESH_INTERVAL);
    assert_eq!(rig.drain(), []);

    let mut changed = block();
    changed[EFFECT_5_BRIGHTNESS] = 16;
    fake.queue_feature_reply(&reply(&changed));
    rig.step(2 * REFRESH_MS);
    let expected = CurrentLighting {
        brightness: Some(16),
        ..current()
    };
    assert_eq!(rig.drain(), [Event::Lighting(expected)]);
    // Polling only ever reads.
    assert_eq!(count(&fake, CONFIG_READ), 3);
    assert_eq!(commands(&fake).len(), 3);
}

#[test]
fn refresh_reads_now_and_always_tells() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::Refresh);
    // It also puts off the next poll.
    assert_eq!(rig.step(4000), REFRESH_INTERVAL);
    assert_eq!(rig.drain(), [Event::Lighting(current())]);
}

#[test]
fn refresh_writes_a_waiting_slider_first_so_that_what_it_reads_is_current() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 2);
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.send(Cmd::Refresh);
    rig.step(1);
    assert_eq!(
        commands(&fake),
        [CONFIG_READ, CONFIG_READ, CONFIG_WRITE, CONFIG_READ]
    );
}

// Lighting

#[test]
fn set_lighting_writes_at_once() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::SetLighting(brightness(4)));
    rig.step(1);
    let expected = CurrentLighting {
        brightness: Some(4),
        ..current()
    };
    assert_eq!(rig.drain(), [Event::Lighting(expected)]);
    let writes = config_writes(&fake);
    assert_eq!(writes.len(), 1);
    assert_eq!(written_block(&writes[0])[EFFECT_5_BRIGHTNESS], 4);
}

#[test]
fn a_write_puts_off_the_next_poll() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::SetLighting(brightness(4)));
    assert_eq!(rig.step(4000), REFRESH_INTERVAL);
}

#[test]
fn a_poll_that_reads_what_was_just_written_doesnt_tell_it_again() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::SetLighting(brightness(4)));
    rig.step(1);
    rig.drain();
    let mut written = block();
    written[EFFECT_5_BRIGHTNESS] = 4;
    fake.queue_feature_reply(&reply(&written));
    rig.step(REFRESH_MS + 1);
    assert_eq!(rig.drain(), []);
}

#[test]
fn merging_keeps_each_field_and_the_later_value_wins() {
    let earlier = LightingChange {
        mode: Some(ModeChoice::Off),
        brightness: Some(1),
        speed: Some(1),
        color: Some(ColorChoice::Rainbow),
        sleep: Some(SleepTime::Min5),
    };
    let later = LightingChange {
        mode: Some(ModeChoice::Effect(2)),
        brightness: Some(2),
        speed: Some(2),
        color: Some(ColorChoice::Slot(2)),
        sleep: Some(SleepTime::Min10),
    };
    assert_eq!(merged(earlier, later), later);
    assert_eq!(merged(earlier, LightingChange::default()), earlier);
    assert_eq!(merged(LightingChange::default(), later), later);
}

#[test]
fn a_slider_burst_writes_once_with_the_latest_value_300_ms_after_the_last() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);

    for (value, at) in [(4, 100), (12, 200), (16, 300)] {
        rig.send(Cmd::AdjustLighting(brightness(value)));
        // Asks to come back when the last value will have settled.
        assert_eq!(rig.step(at), SETTLE_TIME);
    }
    assert_eq!(rig.step(599), Duration::from_millis(1));
    assert!(config_writes(&fake).is_empty());
    assert_eq!(rig.drain(), []);

    rig.step(600);
    let writes = config_writes(&fake);
    assert_eq!(writes.len(), 1);
    assert_eq!(written_block(&writes[0])[EFFECT_5_BRIGHTNESS], 16);
    let expected = CurrentLighting {
        brightness: Some(16),
        ..current()
    };
    assert_eq!(rig.drain(), [Event::Lighting(expected)]);

    // Nothing is left over.
    rig.step(1000);
    assert_eq!(config_writes(&fake).len(), 1);
}

#[test]
fn a_burst_on_two_sliders_keeps_both() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.send(Cmd::AdjustLighting(speed(5)));
    rig.step(100);
    rig.step(400);
    let writes = config_writes(&fake);
    assert_eq!(writes.len(), 1);
    assert_eq!(written_block(&writes[0])[EFFECT_5_BRIGHTNESS], 4);
    assert_eq!(written_block(&writes[0])[EFFECT_5_SPEED_COLOR] >> 4, 5);
}

#[test]
fn set_lighting_writes_a_waiting_slider_first_for_the_effect_it_was_for() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    let mut after_slider = block();
    after_slider[EFFECT_5_BRIGHTNESS] = 4;
    fake.queue_feature_reply(&reply(&block()));
    fake.queue_feature_reply(&reply(&after_slider));
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.send(Cmd::SetLighting(LightingChange {
        mode: Some(ModeChoice::Effect(1)),
        ..LightingChange::default()
    }));
    rig.step(1);
    let writes = config_writes(&fake);
    assert_eq!(writes.len(), 2);
    assert_eq!(written_block(&writes[0])[EFFECT_5_BRIGHTNESS], 4);
    assert_eq!(written_block(&writes[0])[0x0A], 5);
    assert_eq!(written_block(&writes[1])[EFFECT_5_BRIGHTNESS], 4);
    assert_eq!(written_block(&writes[1])[0x0A], 1);
}

#[test]
fn polling_waits_for_a_slider_to_settle() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 2);
    rig.send(Cmd::AdjustLighting(brightness(4)));
    assert_eq!(rig.step(4900), SETTLE_TIME);
    // The poll is due, but a read now could put the slider back.
    let reads = count(&fake, CONFIG_READ);
    assert_eq!(rig.step(5100), Duration::from_millis(100));
    assert_eq!(count(&fake, CONFIG_READ), reads);
    assert_eq!(rig.drain(), []);

    // After a write the next poll is a full interval away.
    assert_eq!(rig.step(5200), REFRESH_INTERVAL);
    assert_eq!(config_writes(&fake).len(), 1);
    assert_eq!(rig.step(5300), Duration::from_millis(4900));
}

#[test]
fn a_refused_change_is_an_error_and_keeps_the_keyboard() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::SetLighting(brightness(5)));
    rig.step(1);
    let events = rig.drain();
    assert!(
        matches!(events[0], Event::Error(Problem::Device(Error::Invalid(_)))),
        "{events:?}"
    );
    assert_eq!(events.len(), 1);
    assert!(config_writes(&fake).is_empty());
}

#[test]
fn a_custom_colour_tells_of_the_new_palettes_for_saving() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    let pink = Rgb::new(0xFF, 0x10, 0x80);
    rig.send(Cmd::SetLighting(LightingChange {
        color: Some(ColorChoice::Custom(pink)),
        ..LightingChange::default()
    }));
    rig.step(1);
    let events = rig.drain();
    let Event::PalettesChanged { pid, palettes } = &events[1] else {
        panic!("{events:?}");
    };
    assert_eq!(*pid, 0x0265);
    assert_eq!(palettes.get(5, 0), Some(pink));
    assert_eq!(count(&fake, PALETTES_WRITE), 1);
}

#[test]
fn the_saved_palettes_of_the_model_are_the_ones_sent() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 2);
    let saved_color = Rgb::new(1, 2, 3);
    let mut saved = Palettes::default();
    saved.set(9, 4, saved_color).unwrap();

    let (cmds, cmd_rx) = channel();
    let (event_tx, events) = channel();
    let options = Options {
        auto_clock: false,
        palettes: BTreeMap::from([(0x0265, saved)]),
        rk_app_running: Box::new(|| false),
    };
    let emitter = Emitter {
        events: event_tx,
        notify: Arc::new(|| {}),
    };
    let handed_out = fake.clone();
    let open = move || Device::new(handed_out.clone(), None, s85());
    let mut worker = Worker::new(open, cmd_rx, options, emitter, Arc::default());
    worker.step(Instant::now());
    cmds.send(Cmd::SetLighting(LightingChange {
        color: Some(ColorChoice::Custom(Rgb::new(9, 9, 9))),
        ..LightingChange::default()
    }))
    .unwrap();
    worker.step(Instant::now());

    let changed = events
        .try_iter()
        .find_map(|event| match event {
            Event::PalettesChanged { palettes, .. } => Some(palettes),
            _ => None,
        })
        .expect("new palettes");
    assert_eq!(changed.get(9, 4), Some(saved_color));
    assert_eq!(changed.get(5, 0), Some(Rgb::new(9, 9, 9)));
}

// The clock

#[test]
fn sync_clock_works_whatever_the_setting() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::SyncClock);
    rig.step(1);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
    assert_eq!(count(&fake, CLOCK), 1);
}

#[test]
fn resuming_sets_the_clock_only_with_auto_clock_on_and_after_looking_at_the_keyboard() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(count(&fake, CLOCK), 0);
    assert_eq!(rig.drain(), []);

    rig.send(Cmd::SetAutoClock(true));
    queue_reads(&fake, 1);
    rig.send(Cmd::Resumed);
    rig.step(2);
    assert_eq!(count(&fake, CLOCK), 1);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
    assert_eq!(
        commands(&fake),
        [CONFIG_READ, CONFIG_READ, CONFIG_READ, CLOCK]
    );
}

#[test]
fn waking_with_the_keyboard_gone_is_no_usb_error() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected_with(&fake, true, Arc::new(|| {}));
    rig.drain();
    let clocks = count(&fake, CLOCK);
    fake.disconnect();
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain(), [Event::Disconnected]);
    assert_eq!(count(&fake, CLOCK), clocks);
}

#[test]
fn waking_with_no_keyboard_leaves_the_clock_to_connecting() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 1);
    let opens = VecDeque::from([Err(Error::NotFound), Ok(fake.clone())]);
    let mut rig = Rig::with_opens(opens, true, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(
        rig.drain(),
        [
            Event::Connected { model: s85() },
            Event::Lighting(current()),
            Event::ClockSynced
        ]
    );
    assert_eq!(count(&fake, CLOCK), 1);
}

#[test]
fn waking_while_a_slider_settles_looks_at_the_keyboard_after_it() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected_with(&fake, true, Arc::new(|| {}));
    rig.drain();
    queue_reads(&fake, 2);
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain(), []);
    // The write puts off the look at the keyboard, and the clock with it.
    rig.step(301);
    assert_eq!(count(&fake, CLOCK), 1);
    rig.step(5301);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));
    assert_eq!(count(&fake, CLOCK), 2);
}

// Uploads

#[test]
fn an_upload_tells_how_it_goes_and_leaves_the_keyboard_usable() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    let events = rig.drain();
    assert!(started(S85_STEPS)(&events[0]), "{:?}", events[0]);
    assert!(matches!(
        events[1],
        Event::Progress {
            done: 1,
            total: S85_STEPS,
            ..
        }
    ));
    assert!(matches!(
        events[S85_STEPS],
        Event::Progress {
            done: S85_STEPS,
            total: S85_STEPS,
            ..
        }
    ));
    assert_eq!(events[S85_STEPS + 1], Event::UploadDone);
    assert_eq!(events.len(), S85_STEPS + 2);
    assert_eq!(count(&fake, SCREEN_DATA), 215);
    fake.assert_allowed_commands_only();

    // Over, so a cancel has nothing to act on.
    rig.handle().send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), []);

    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
}

#[test]
fn the_estimate_is_the_steps_times_the_measured_chunk_time() {
    let fake = FakeTransport::with_auto_ack();
    fake.set_input_latency(Duration::from_millis(10));
    let mut rig = Rig::measuring(&fake);

    // Nothing measured yet, so the first upload goes by the usual figure.
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    let events = rig.drain();
    assert_eq!(
        events[0],
        Event::UploadStarted {
            total: S85_STEPS,
            estimate: 217 * DEFAULT_CHUNK_TIME,
        }
    );
    // The step after the first has the start behind it and nothing measured yet.
    let Event::Progress { remaining, .. } = events[1] else {
        panic!("{:?}", events[1]);
    };
    assert_eq!(remaining, 216 * DEFAULT_CHUNK_TIME);
    // By the last chunk every gap has been 10 ms, with one step left.
    let Event::Progress { remaining, .. } = events[S85_STEPS - 1] else {
        panic!("{:?}", events[S85_STEPS - 1]);
    };
    assert_eq!(remaining, Duration::from_millis(10));
    let Event::Progress { remaining, .. } = events[S85_STEPS] else {
        panic!("{:?}", events[S85_STEPS]);
    };
    assert_eq!(remaining, Duration::ZERO);

    rig.send(Cmd::UploadScreen(still()));
    rig.step(2);
    assert_eq!(
        rig.drain()[0],
        Event::UploadStarted {
            total: S85_STEPS,
            estimate: Duration::from_millis(2170),
        }
    );
}

#[test]
fn a_chunk_sent_twice_takes_as_long_as_both_sends_took() {
    let fake = FakeTransport::with_auto_ack();
    fake.set_input_latency(Duration::from_millis(10));
    // The start, then the first two chunks. The second has to go again.
    fake.ack_fault(2, AckFault::WrongIdx);
    let mut rig = Rig::measuring(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    rig.drain();
    // 214 gaps between 215 chunks: 213 of 10 ms and one of 20 ms.
    assert_eq!(rig.worker.chunk_time.mean(), Duration::from_micros(10_046));
}

#[test]
fn the_time_after_an_upload_counts_from_when_it_ended() {
    let fake = FakeTransport::with_auto_ack();
    let hour_later = Arc::new(Mutex::new(None::<Arc<Mutex<Instant>>>));
    let advance = Arc::clone(&hour_later);
    let seen = AtomicUsize::new(0);
    let notify: Notify = Arc::new(move || {
        // Partway through the upload, an hour goes by.
        if seen.fetch_add(1, Ordering::SeqCst) == 50
            && let Some(clock) = advance.lock().unwrap().as_ref()
        {
            let mut now = clock.lock().unwrap();
            *now += Duration::from_secs(3600);
        }
    });
    let mut rig = Rig::connected_with(&fake, false, notify);
    *hour_later.lock().unwrap() = Some(Arc::clone(&rig.clock));

    rig.send(Cmd::UploadScreen(still()));
    // The step started an hour ago as far as the worker's clock can now tell.
    rig.step(1);
    rig.drain();
    // A poll is due five seconds after the upload's end, and no sooner.
    let reads = count(&fake, CONFIG_READ);
    assert_eq!(rig.step(3_600_000), Duration::from_millis(5001));
    assert_eq!(count(&fake, CONFIG_READ), reads);
}

#[test]
fn cancelling_says_so_at_once_then_stops_with_an_abort() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::CancelUpload);
    rig.step(1);
    assert_eq!(
        without_progress(rig.drain())[1..],
        [Event::Cancelling, Event::UploadCancelled]
    );
    let controls: Vec<Vec<u8>> = fake
        .sent()
        .into_iter()
        .filter(|report| report[1] == SCREEN_CONTROL)
        .collect();
    assert_eq!(controls.len(), 2);
    assert_eq!(controls[0][HEADER], 1 << 6);
    assert_eq!(controls[1][HEADER], 2 << 6);
    assert!(count(&fake, SCREEN_DATA) < 215);

    // The next upload isn't cancelled by the old flag.
    rig.send(Cmd::UploadScreen(still()));
    rig.step(2);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
}

#[test]
fn the_handle_cancels_without_waiting_for_the_worker() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    let handle = rig.handle();

    // Nothing is going on, so this does nothing. Later uploads don't see it.
    handle.send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), []);

    // The worker hasn't got to this yet, and is as good as stuck in an ack wait.
    handle.send(Cmd::UploadScreen(still()));
    handle.send(Cmd::CancelUpload);
    handle.send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), [Event::Cancelling]);

    rig.step(1);
    // A cancel that's already set stops it before anything starts.
    assert_eq!(rig.drain(), [Event::UploadCancelled]);
    assert_eq!(count(&fake, SCREEN_CONTROL), 0);

    handle.send(Cmd::UploadScreen(still()));
    rig.step(2);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
}

#[test]
fn a_cancel_with_no_upload_does_nothing() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::CancelUpload);
    rig.step(1);
    assert_eq!(rig.drain(), []);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(2);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
}

#[test]
fn a_cancel_of_an_upload_that_wont_run_still_gets_its_end() {
    // No keyboard.
    let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    let handle = rig.handle();
    handle.send(Cmd::UploadScreen(still()));
    handle.send(Cmd::CancelUpload);
    rig.step(1);
    assert_eq!(rig.drain(), [Event::Cancelling, Event::UploadCancelled]);

    // A bad block.
    let fake = FakeTransport::new();
    bad_read(&fake);
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake)]), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    let handle = rig.handle();
    handle.send(Cmd::UploadScreen(still()));
    handle.send(Cmd::CancelUpload);
    rig.step(1);
    assert_eq!(rig.drain(), [Event::Cancelling, Event::UploadCancelled]);
}

#[test]
fn a_refused_upload_leaves_nothing_to_cancel() {
    let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    let handle = rig.handle();
    handle.send(Cmd::UploadScreen(still()));
    rig.step(1);
    assert_eq!(rig.drain(), [Event::Error(Problem::NoKeyboard)]);
    handle.send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), []);

    // An image that doesn't fit the keyboard.
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    let other = model::find(0x01A2).unwrap().screen.unwrap();
    let wrong = Animation::from_bytes(&png(), &other, LoadOptions::default()).unwrap();
    let handle = rig.handle();
    handle.send(Cmd::UploadScreen(Arc::new(wrong)));
    rig.step(1);
    rig.drain();
    handle.send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), []);
}

#[test]
fn a_cancel_just_as_an_upload_gets_refused_is_what_gets_told() {
    // Between the worker's check for a cancel and its refusal, which no test can get in between.
    let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    let shared = Arc::clone(&rig.worker.shared);
    shared.begin_upload();
    shared.request_cancel(&rig.worker.emitter);
    rig.worker.refuse_upload(Problem::NoKeyboard);
    assert_eq!(rig.drain(), [Event::Cancelling, Event::UploadCancelled]);
}

#[test]
fn a_cancel_that_loses_the_race_with_the_end_still_has_one_end() {
    // Asked for after the last step: the upload completes, and that's the end event.
    let fake = FakeTransport::with_auto_ack();
    let handle_slot = Arc::new(Mutex::new(None::<WorkerHandle>));
    let cancel_from = Arc::clone(&handle_slot);
    let seen = AtomicUsize::new(0);
    let notify: Notify = Arc::new(move || {
        if seen.fetch_add(1, Ordering::SeqCst) == 3 + S85_STEPS
            && let Some(handle) = cancel_from.lock().unwrap().as_ref()
        {
            handle.send(Cmd::CancelUpload);
        }
    });
    let mut rig = Rig::connected_with(&fake, false, notify);
    *handle_slot.lock().unwrap() = Some(rig.handle());
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    assert_eq!(
        without_progress(rig.drain())[1..],
        [Event::Cancelling, Event::UploadDone]
    );
    handle_slot.lock().unwrap().take();
}

#[test]
fn a_finish_the_keyboard_doesnt_confirm_is_a_warning() {
    let fake = FakeTransport::with_auto_ack();
    // The start, then 215 chunks, then the finish.
    fake.ack_fault(216, AckFault::Drop);
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    assert_eq!(
        without_progress(rig.drain())[1..],
        [Event::UploadUnconfirmed, Event::UploadDone]
    );
}

#[test]
fn an_upload_the_keyboard_stops_answering_fails_but_stays_connected() {
    let fake = FakeTransport::with_auto_ack();
    fake.ack_fault(3, AckFault::Drop);
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    assert_eq!(
        without_progress(rig.drain())[1..],
        [Event::UploadFailed(Error::NoAck { frame: 0, idx: 2 })]
    );
    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
}

#[test]
fn a_keyboard_that_disappears_during_an_upload_gives_an_error_and_disconnected() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected_with(&fake, false, unplug_after(&fake, 20));
    rig.send(Cmd::UploadScreen(still()));
    assert_eq!(rig.step(1), RECONNECT_INTERVAL);
    let events = without_progress(rig.drain());
    assert!(started(S85_STEPS)(&events[0]));
    assert!(
        matches!(events[1], Event::UploadFailed(Error::Hid(_))),
        "{events:?}"
    );
    assert_eq!(events[2], Event::Disconnected);
    assert_eq!(events.len(), 3);
}

#[test]
fn the_clock_waits_for_an_upload_and_runs_once_after_it() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::SetAutoClock(true));
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SyncClock);
    rig.send(Cmd::Resumed);
    rig.step(1);

    assert_eq!(
        without_progress(rig.drain())[1..],
        [Event::UploadDone, Event::ClockSynced]
    );
    let sent = commands(&fake);
    let last_control = sent.iter().rposition(|&c| c == SCREEN_CONTROL).unwrap();
    let clock = sent.iter().position(|&c| c == CLOCK).unwrap();
    assert!(clock > last_control, "{sent:?}");
    assert_eq!(count(&fake, CLOCK), 1);

    // It isn't still waiting for the next one.
    rig.send(Cmd::UploadScreen(still()));
    rig.step(2);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
    assert_eq!(count(&fake, CLOCK), 1);
}

#[test]
fn waking_during_an_upload_sets_the_clock_after_it_only_with_auto_clock_on() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
    assert_eq!(count(&fake, CLOCK), 0);

    rig.send(Cmd::SetAutoClock(true));
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::Resumed);
    rig.step(2);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));
    assert_eq!(count(&fake, CLOCK), 1);
}

#[test]
fn an_explicit_clock_during_an_upload_counts_whatever_the_setting() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SyncClock);
    // Waking with the setting off doesn't cancel what was asked for.
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));
    assert_eq!(count(&fake, CLOCK), 1);
}

#[test]
fn turning_the_automatic_clock_off_during_an_upload_is_respected() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected_with(&fake, true, Arc::new(|| {}));
    rig.drain();
    let clocks = count(&fake, CLOCK);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::Resumed);
    rig.send(Cmd::SetAutoClock(false));
    rig.step(1);
    assert_eq!(count(&fake, CLOCK), clocks);

    // It stays off.
    queue_reads(&fake, 1);
    rig.send(Cmd::Resumed);
    rig.step(2);
    assert_eq!(count(&fake, CLOCK), clocks);
}

#[test]
fn turning_the_automatic_clock_on_during_an_upload_counts_for_the_next_wake() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SetAutoClock(true));
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));
}

#[test]
fn a_clock_with_the_automatic_setting_off_and_no_wake_isnt_set_after_an_upload() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
    assert_eq!(count(&fake, CLOCK), 0);
}

#[test]
fn the_clock_also_runs_after_a_cancelled_or_failed_upload() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SyncClock);
    rig.send(Cmd::CancelUpload);
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));

    fake.ack_fault(fake_acks_so_far(&fake) + 1, AckFault::Drop);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain().last(), Some(&Event::ClockSynced));
}

fn fake_acks_so_far(fake: &FakeTransport) -> usize {
    count(fake, SCREEN_CONTROL) + count(fake, SCREEN_DATA)
}

#[test]
fn a_clock_waiting_on_an_upload_is_forgotten_with_the_keyboard() {
    let first = FakeTransport::with_auto_ack();
    let second = FakeTransport::with_auto_ack();
    queue_reads(&first, 1);
    queue_reads(&second, 2);
    let opens = VecDeque::from([Ok(first.clone()), Ok(second.clone())]);
    let mut rig = Rig::with_opens(opens, false, unplug_after(&first, 20));
    rig.step(0);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SyncClock);
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::Disconnected));

    // The next keyboard connects, gets looked at, and uploads, with no clock from the old request.
    rig.step(2001);
    rig.step(7001);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(7002);
    assert_eq!(rig.drain().last(), Some(&Event::UploadDone));
    assert_eq!(count(&second, CLOCK), 0);
}

#[test]
fn lighting_and_a_second_upload_are_refused_during_an_upload() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SetLighting(brightness(4)));
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::Refresh);
    rig.step(1);
    assert_eq!(
        without_progress(rig.drain())[1..],
        [
            Event::Error(Problem::Busy),
            Event::Error(Problem::Busy),
            Event::Error(Problem::Busy),
            Event::UploadDone,
        ]
    );
    assert!(config_writes(&fake).is_empty());
    // Only the read from connecting.
    assert_eq!(count(&fake, CONFIG_READ), 1);
    assert_eq!(count(&fake, SCREEN_DATA), 215);
    // And nothing was held back for later.
    rig.step(100);
    assert!(config_writes(&fake).is_empty());
}

#[test]
fn a_second_upload_in_the_last_pauses_of_one_is_refused_too() {
    let fake = FakeTransport::with_auto_ack();
    let image = still();
    let cmds_slot = Arc::new(Mutex::new(None::<Sender<Cmd>>));
    let cmds = Arc::clone(&cmds_slot);
    let again = Arc::clone(&image);
    let seen = AtomicUsize::new(0);
    let notify: Notify = Arc::new(move || {
        // The upload's last progress, after which only its pauses are left.
        if seen.fetch_add(1, Ordering::SeqCst) == 3 + S85_STEPS
            && let Some(cmds) = cmds.lock().unwrap().as_ref()
        {
            cmds.send(Cmd::UploadScreen(Arc::clone(&again))).unwrap();
        }
    });
    let mut rig = Rig::connected_with(&fake, false, notify);
    *cmds_slot.lock().unwrap() = Some(rig.cmds.clone());
    rig.send(Cmd::UploadScreen(image));
    rig.step(1);
    assert_eq!(
        without_progress(rig.drain())[1..],
        [Event::Error(Problem::Busy), Event::UploadDone]
    );
    assert_eq!(count(&fake, SCREEN_DATA), 215);
}

#[test]
fn a_waiting_slider_goes_out_before_an_upload() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::AdjustLighting(brightness(4)));
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    let sent = commands(&fake);
    let write = sent.iter().position(|&c| c == CONFIG_WRITE).unwrap();
    let start = sent.iter().position(|&c| c == SCREEN_CONTROL).unwrap();
    assert!(write < start, "{sent:?}");
}

#[test]
fn an_animation_of_the_wrong_size_is_an_error_with_nothing_sent() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    let other = model::find(0x01A2).unwrap().screen.unwrap();
    assert_ne!(other.width, s85().screen.unwrap().width);
    let wrong = Animation::from_bytes(&png(), &other, LoadOptions::default()).unwrap();
    rig.send(Cmd::UploadScreen(Arc::new(wrong)));
    rig.step(1);
    let events = rig.drain();
    assert!(
        matches!(events[0], Event::Error(Problem::Device(Error::Invalid(_)))),
        "{events:?}"
    );
    assert_eq!(events.len(), 1);
    assert_eq!(count(&fake, SCREEN_CONTROL), 0);
    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
}

#[test]
fn every_flow_sends_only_allowed_commands() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 4);
    rig.send(Cmd::SetLighting(LightingChange {
        mode: Some(ModeChoice::Effect(5)),
        brightness: Some(4),
        speed: Some(1),
        color: Some(ColorChoice::Custom(Rgb::new(1, 2, 3))),
        sleep: Some(SleepTime::Min5),
    }));
    rig.send(Cmd::AdjustLighting(brightness(12)));
    rig.send(Cmd::SyncClock);
    rig.send(Cmd::Refresh);
    rig.send(Cmd::Report);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    fake.assert_allowed_commands_only();
}

// Images

fn wait_for_image(rig: &Rig) -> Event {
    rig.events.recv_timeout(Duration::from_secs(30)).unwrap()
}

#[test]
fn an_image_loads_off_the_worker_and_comes_with_its_plan_and_estimate() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    let path = temp_file("loads.gif", &gif(3));
    rig.send(Cmd::LoadImage {
        path: path.clone(),
        fit: Fit::Contain,
        delay: Some(250),
    });
    rig.step(1);
    let Event::ImageLoaded(image) = wait_for_image(&rig) else {
        panic!("no image");
    };
    assert_eq!(image.path, path);
    assert_eq!(image.animation.frames().len(), 3);
    assert_eq!(image.animation.delay_ms(), 250);
    assert_eq!(image.plan.frame_count(), 3);
    assert_eq!(image.estimate, (3 * 215 + 2) * DEFAULT_CHUNK_TIME);
    assert_eq!(image.animation.width(), s85().screen.unwrap().width);
    assert!(!image.animation.truncated());
    assert!(commands(&fake).len() == 1, "loading sends nothing");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn too_many_frames_are_cut_with_the_image_saying_so() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    let max = usize::from(s85().screen.unwrap().max_frames);
    let path = temp_file("cut.gif", &gif(max + 2));
    rig.send(Cmd::LoadImage {
        path: path.clone(),
        fit: Fit::Cover,
        delay: None,
    });
    rig.step(1);
    let Event::ImageLoaded(image) = wait_for_image(&rig) else {
        panic!("no image");
    };
    assert!(image.animation.truncated());
    assert_eq!(image.animation.frames().len(), max);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_file_that_wont_load_is_told_with_its_own_event() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    let path = temp_file("bad.gif", b"not an image");
    rig.send(Cmd::LoadImage {
        path: path.clone(),
        fit: Fit::Cover,
        delay: None,
    });
    rig.step(1);
    assert!(matches!(
        wait_for_image(&rig),
        Event::ImageFailed(Error::Image(_))
    ));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn an_image_can_load_during_an_upload_of_another() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    let path = temp_file("during.gif", &gif(2));
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::LoadImage {
        path: path.clone(),
        fit: Fit::Cover,
        delay: None,
    });
    rig.step(1);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = None;
    while Instant::now() < deadline {
        match rig.events.recv_timeout(Duration::from_secs(30)).unwrap() {
            Event::ImageLoaded(image) => {
                seen = Some(image.path);
                break;
            }
            Event::Progress { .. } | Event::UploadStarted { .. } | Event::UploadDone => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(seen.as_ref(), Some(&path));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_load_refused_with_no_keyboard_replaces_one_still_running() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    let (started_tx, started) = channel();
    let (release, release_rx) = channel::<()>();
    let release_rx = Mutex::new(release_rx);
    rig.worker.loader = Loader::new(
        rig.worker.emitter.clone(),
        Arc::new(move |request: &LoadRequest| {
            started_tx.send(()).unwrap();
            let wait = Duration::from_secs(30);
            release_rx.lock().unwrap().recv_timeout(wait).unwrap();
            Err(Error::Image(request.path.display().to_string()))
        }),
    );
    rig.send(Cmd::LoadImage {
        path: PathBuf::from("running.gif"),
        fit: Fit::Cover,
        delay: None,
    });
    rig.step(1);
    started.recv_timeout(Duration::from_secs(30)).unwrap();

    fake.disconnect();
    rig.step(REFRESH_MS);
    rig.send(Cmd::LoadImage {
        path: PathBuf::from("refused.gif"),
        fit: Fit::Cover,
        delay: None,
    });
    rig.step(REFRESH_MS + 1);
    release.send(()).unwrap();

    // Dropping the worker ends the loader's thread once it has nothing left to do, and with that
    // the channel closes: everything it was going to tell has been told.
    let Rig {
        events,
        worker,
        cmds,
        ..
    } = rig;
    drop(worker);
    drop(cmds);
    let told: Vec<Event> = events.iter().collect();
    assert_eq!(
        told,
        [Event::Disconnected, Event::Error(Problem::NoKeyboard)]
    );
}

#[test]
fn rgb565_frames_expand_to_rgb_for_previews() {
    assert_eq!(
        rgb_from_565(&[0xF8, 0x00, 0x07, 0xE0, 0x00, 0x1F, 0xFF, 0xFF, 0x00, 0x00]),
        [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0]
    );
    // The low bits repeat the high ones, so that it spreads over the whole range.
    assert_eq!(rgb_from_565(&[0x80, 0x10]), [132, 0, 132]);
    assert_eq!(rgb_from_565(&[]), [] as [u8; 0]);
}

#[test]
fn a_preview_frame_has_three_bytes_per_pixel_of_the_screen() {
    let image = still();
    let spec = s85().screen.unwrap();
    let frame = rgb_from_565(&image.frames()[0]);
    assert_eq!(
        frame.len(),
        usize::from(spec.width) * usize::from(spec.height) * 3
    );
    assert_eq!(frame[..3], [255, 0, 0]);
}

// Reports

#[test]
fn a_report_reads_the_config_and_has_the_last_errors() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    queue_reads(&fake, 1);
    rig.send(Cmd::SetLighting(brightness(5)));
    rig.step(1);
    rig.drain();
    queue_reads(&fake, 1);
    rig.send(Cmd::Report);
    rig.step(2);
    let events = rig.drain();
    let Event::ReportReady(data) = &events[0] else {
        panic!("{events:?}");
    };
    assert_eq!(data.model, Some(s85()));
    assert_eq!(data.config, Ok(block().to_vec()));
    assert_eq!(data.errors.len(), 1);
    assert!(data.errors[0].contains("brightness 5"), "{:?}", data.errors);
}

#[test]
fn the_errors_in_a_report_include_every_kind_of_failure() {
    // A keyboard that wouldn't open, then one that goes away while being polled.
    let first = FakeTransport::new();
    queue_reads(&first, 1);
    let opens = VecDeque::from([Err(Error::Hid("denied".to_string())), Ok(first.clone())]);
    let mut rig = Rig::with_opens(opens, false, Arc::new(|| {}));
    rig.step(0);
    assert_eq!(report_of(&mut rig, 1).errors, ["USB error: denied"]);

    rig.step(2000);
    first.disconnect();
    rig.step(7000);
    let errors = report_of(&mut rig, 7001).errors;
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[1].starts_with("USB error: "), "{errors:?}");
}

#[test]
fn a_failed_upload_is_in_the_report() {
    let fake = FakeTransport::with_auto_ack();
    fake.ack_fault(3, AckFault::Drop);
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    rig.drain();
    queue_reads(&fake, 1);
    assert_eq!(
        report_of(&mut rig, 2).errors,
        [Error::NoAck { frame: 0, idx: 2 }.to_string()]
    );
}

#[test]
fn a_report_whose_read_fails_says_why_and_a_gone_keyboard_is_lost() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    fake.disconnect();
    rig.send(Cmd::Report);
    rig.step(1);
    let events = rig.drain();
    let Event::ReportReady(data) = &events[0] else {
        panic!("{events:?}");
    };
    assert_eq!(data.model, Some(s85()));
    let why = data.config.clone().unwrap_err();
    assert!(why.starts_with("USB error: "), "{why}");
    assert_eq!(data.errors, [why]);
    assert_eq!(events[1], Event::Disconnected);
    assert_eq!(events.len(), 2);
}

#[test]
fn a_report_that_reads_a_bad_block_stays_connected() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    bad_read(&fake);
    rig.send(Cmd::Report);
    rig.step(1);
    let events = rig.drain();
    assert_eq!(events.len(), 1, "{events:?}");
    let Event::ReportReady(data) = &events[0] else {
        panic!("{events:?}");
    };
    assert!(data.config.is_err());
    assert_eq!(data.errors.len(), 1);
}

#[test]
fn a_report_with_no_keyboard_says_so() {
    let mut rig = Rig::with_opens(VecDeque::new(), false, Arc::new(|| {}));
    rig.step(0);
    rig.drain();
    rig.send(Cmd::Report);
    rig.step(1);
    assert_eq!(
        rig.drain(),
        [Event::ReportReady(ReportData {
            model: None,
            config: Err("no keyboard connected".to_string()),
            errors: Vec::new(),
        })]
    );
}

#[test]
fn a_report_during_an_upload_sends_nothing() {
    let fake = FakeTransport::with_auto_ack();
    let mut rig = Rig::connected(&fake);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::Report);
    rig.step(1);
    let events = without_progress(rig.drain());
    let Event::ReportReady(data) = &events[1] else {
        panic!("{events:?}");
    };
    assert_eq!(data.model, Some(s85()));
    assert_eq!(
        data.config,
        Err("not read: an upload is running".to_string())
    );
    assert_eq!(count(&fake, CONFIG_READ), 1);
}

#[test]
fn the_error_list_keeps_only_the_newest() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    for value in 0..(MAX_ERRORS as u8 + 3) {
        queue_reads(&fake, 1);
        // Not one of this model's values.
        rig.send(Cmd::SetLighting(brightness(100 + value)));
        rig.step(1);
    }
    rig.drain();
    queue_reads(&fake, 1);
    rig.send(Cmd::Report);
    rig.step(2);
    let events = rig.drain();
    let Event::ReportReady(data) = &events[0] else {
        panic!("{events:?}");
    };
    assert_eq!(data.errors.len(), MAX_ERRORS);
    assert!(
        data.errors[0].contains("brightness 103"),
        "{:?}",
        data.errors
    );
}

// The real thread, with the same factory seam that a demo build would use.

const WAIT: Duration = Duration::from_secs(30);

fn plain_options(auto_clock: bool) -> Options {
    Options {
        auto_clock,
        palettes: BTreeMap::new(),
        rk_app_running: Box::new(|| false),
    }
}

fn wait_for(events: &Receiver<Event>, wanted: fn(&Event) -> bool) -> Event {
    loop {
        let event = events.recv_timeout(WAIT).unwrap();
        if wanted(&event) {
            return event;
        }
    }
}

/// Fast enough for the watchdog to show within a test, and a shutdown that doesn't wait for long.
const QUICK: Timings = Timings {
    watch: WatchTimes {
        hung_after: Duration::from_millis(250),
        check_every: Duration::from_millis(5),
    },
    shutdown_wait: Duration::from_millis(300),
};

/// Sets the flag when the last copy goes, taking its time, so that a test can see whether something
/// waited for that.
struct SlowDrop(Arc<AtomicBool>);

impl Drop for SlowDrop {
    fn drop(&mut self) {
        thread::sleep(Duration::from_millis(100));
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A keyboard whose config reads wait for as long as the test wants.
#[derive(Clone)]
struct HeldUp {
    fake: FakeTransport,
    /// Config reads wait while this is set.
    hold: Arc<AtomicBool>,
    /// Each input report takes this long to arrive, in real time.
    read_delay: Duration,
    gone: Arc<SlowDrop>,
}

impl HeldUp {
    fn new(fake: &FakeTransport) -> HeldUp {
        HeldUp {
            fake: fake.clone(),
            hold: Arc::default(),
            read_delay: Duration::ZERO,
            gone: Arc::new(SlowDrop(Arc::default())),
        }
    }
}

impl Transport for HeldUp {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        self.fake.send_feature(report)
    }

    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        while self.hold.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        self.fake.get_feature(buf)
    }

    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize, Error> {
        thread::sleep(self.read_delay);
        self.fake.read_timeout(buf, ms)
    }

    fn sleep(&self, duration: Duration) {
        self.fake.sleep(duration);
    }

    fn now(&self) -> Instant {
        self.fake.now()
    }
}

#[test]
fn a_stuck_read_is_told_as_hung_once_and_the_end_of_it_too() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 50);
    let held = HeldUp::new(&fake);
    let handing = held.clone();
    let open = move || Device::new(handing.clone(), None, s85());
    let (handle, events) = spawn_with(open, plain_options(false), Arc::new(|| {}), QUICK);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));

    held.hold.store(true, Ordering::SeqCst);
    handle.send(Cmd::Refresh);
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Hung));
    // Over twice as long as it took to say so: it doesn't say it again.
    thread::sleep(Duration::from_millis(600));
    assert!(events.try_recv().is_err());

    held.hold.store(false, Ordering::SeqCst);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Responding));

    // It can happen again.
    held.hold.store(true, Ordering::SeqCst);
    handle.send(Cmd::Refresh);
    assert_eq!(events.recv_timeout(WAIT), Ok(Event::Hung));
    held.hold.store(false, Ordering::SeqCst);
    handle.shutdown();
}

#[test]
fn a_worker_waiting_for_commands_isnt_hung() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 50);
    let handing = fake.clone();
    let open = move || Device::new(handing.clone(), None, s85());
    let (handle, events) = spawn_with(open, plain_options(false), Arc::new(|| {}), QUICK);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    // Over twice as long as it takes to count as stuck.
    thread::sleep(Duration::from_millis(600));
    assert!(events.try_recv().is_err());
    handle.shutdown();
}

#[test]
fn a_long_upload_that_keeps_moving_isnt_hung() {
    let fake = FakeTransport::with_auto_ack();
    queue_reads(&fake, 50);
    let mut held = HeldUp::new(&fake);
    held.read_delay = Duration::from_millis(20);
    let open = move || Device::new(held.clone(), None, s85());
    let (handle, events) = spawn_with(open, plain_options(false), Arc::new(|| {}), QUICK);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    handle.send(Cmd::UploadScreen(still()));
    // Well past the limit, spent in the upload.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(600) {
        let event = events.recv_timeout(WAIT).unwrap();
        assert_ne!(event, Event::Hung);
        assert!(!matches!(event, Event::UploadDone));
    }
    handle.shutdown();
}

#[test]
fn shutting_down_waits_for_the_thread_to_finish() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 50);
    let held = HeldUp::new(&fake);
    let dropped = Arc::clone(&held.gone.0);
    let open = move || Device::new(held.clone(), None, s85());
    let timings = Timings {
        shutdown_wait: Duration::from_secs(10),
        ..QUICK
    };
    let (handle, events) = spawn_with(open, plain_options(false), Arc::new(|| {}), timings);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    handle.shutdown();
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn shutting_down_doesnt_wait_for_a_stuck_keyboard() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 50);
    let held = HeldUp::new(&fake);
    let handing = held.clone();
    let open = move || Device::new(handing.clone(), None, s85());
    let (handle, events) = spawn_with(open, plain_options(false), Arc::new(|| {}), QUICK);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));

    held.hold.store(true, Ordering::SeqCst);
    handle.send(Cmd::Refresh);
    let started = Instant::now();
    handle.shutdown();
    let took = started.elapsed();
    assert!(took >= QUICK.shutdown_wait, "{took:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
    held.hold.store(false, Ordering::SeqCst);
}

#[test]
fn the_thread_ends_when_the_handle_is_gone() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 50);
    let handing = fake.clone();
    let open = move || Device::new(handing.clone(), None, s85());
    let (mut handle, events) = spawn_with(open, plain_options(false), Arc::new(|| {}), QUICK);
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    let thread = handle.thread.take().unwrap();
    drop(handle);
    let started = Instant::now();
    while !thread.is_finished() {
        assert!(started.elapsed() < Duration::from_secs(10), "still running");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_handle_that_goes_away_during_an_upload_aborts_it() {
    let fake = FakeTransport::with_auto_ack();
    queue_reads(&fake, 50);
    let handing = fake.clone();
    let open = move || Device::new(handing.clone(), None, s85());
    let (handle, events) = spawn(open, plain_options(false), Arc::new(|| {}));
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    let spec = s85().screen.unwrap();
    let long = Animation::from_bytes(&gif(60), &spec, LoadOptions::default()).unwrap();
    handle.send(Cmd::UploadScreen(Arc::new(long)));
    wait_for(&events, |event| matches!(event, Event::Progress { .. }));
    drop(handle);
    let end = wait_for(&events, |event| {
        matches!(
            event,
            Event::UploadCancelled | Event::UploadDone | Event::UploadFailed(_)
        )
    });
    assert_eq!(end, Event::UploadCancelled);
    let controls: Vec<Vec<u8>> = fake
        .sent()
        .into_iter()
        .filter(|report| report[1] == SCREEN_CONTROL)
        .collect();
    assert_eq!(controls.last().unwrap()[HEADER], 2 << 6);
}

#[test]
fn a_waiting_slider_is_written_on_a_clean_quit() {
    let fake = FakeTransport::new();
    queue_reads(&fake, 50);
    let handing = fake.clone();
    let open = move || Device::new(handing.clone(), None, s85());
    let (handle, events) = spawn(open, plain_options(false), Arc::new(|| {}));
    wait_for(&events, |event| matches!(event, Event::Lighting(_)));
    handle.send(Cmd::AdjustLighting(brightness(4)));
    handle.shutdown();
    assert_eq!(config_writes(&fake).len(), 1);
}

#[test]
fn the_thread_runs_commands_and_tells_the_ui_through_notify() {
    let fake = FakeTransport::with_auto_ack();
    // More than it needs, in case the machine is slow enough for polls to come round.
    queue_reads(&fake, 50);
    let handed_out = fake.clone();
    let open = move || Device::new(handed_out.clone(), None, s85());
    let woken = Arc::new(AtomicUsize::new(0));
    let woken_by_worker = Arc::clone(&woken);
    let options = Options {
        auto_clock: true,
        palettes: BTreeMap::new(),
        rk_app_running: Box::new(|| false),
    };
    let notify: Notify = Arc::new(move || {
        woken_by_worker.fetch_add(1, Ordering::SeqCst);
    });
    let (handle, events) = spawn(open, options, notify);

    let wait = |wanted: Event| loop {
        let event = events.recv_timeout(Duration::from_secs(30)).unwrap();
        if event == wanted {
            break;
        }
    };
    wait(Event::Connected { model: s85() });
    wait(Event::ClockSynced);
    assert!(woken.load(Ordering::SeqCst) >= 3);

    handle.send(Cmd::SyncClock);
    wait(Event::ClockSynced);
    handle.send(Cmd::UploadScreen(still()));
    wait(Event::UploadDone);
    handle.shutdown();
    fake.assert_allowed_commands_only();
}

#[test]
fn shutting_down_stops_an_upload_saying_so_with_an_abort() {
    let fake = FakeTransport::with_auto_ack();
    queue_reads(&fake, 50);
    let handed_out = fake.clone();
    let open = move || Device::new(handed_out.clone(), None, s85());
    let (handle, events) = spawn(open, plain_options(false), Arc::new(|| {}));
    wait_for(&events, |event| matches!(event, Event::Connected { .. }));
    // Enough frames that it can't be over before the next line.
    let spec = s85().screen.unwrap();
    let long = Animation::from_bytes(&gif(60), &spec, LoadOptions::default()).unwrap();
    handle.send(Cmd::UploadScreen(Arc::new(long)));
    wait_for(&events, |event| matches!(event, Event::Progress { .. }));
    handle.shutdown();

    let told = without_progress(events.try_iter().collect());
    assert_eq!(told, [Event::Cancelling, Event::UploadCancelled]);
    let controls: Vec<Vec<u8>> = fake
        .sent()
        .into_iter()
        .filter(|report| report[1] == SCREEN_CONTROL)
        .collect();
    assert_eq!(controls.last().unwrap()[HEADER], 2 << 6);
}
