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
        let worker = Worker::new(open, cmd_rx, options, emitter, Arc::default());
        Rig {
            cmds: cmd_tx,
            events: event_rx,
            worker,
            opens_tried,
            start: Instant::now(),
        }
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
        self.worker.step(self.start + Duration::from_millis(ms))
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
    assert_eq!(
        i32::from(year),
        chrono::Datelike::year(&chrono::Local::now())
    );
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
fn a_block_without_its_end_marker_connects_but_gives_no_lighting_and_writes_nothing() {
    let fake = FakeTransport::new();
    for _ in 0..5 {
        fake.queue_feature_reply(&reply(&[0; CONFIG_LEN]));
    }
    let mut rig = Rig::with_opens(VecDeque::from([Ok(fake.clone())]), false, Arc::new(|| {}));
    rig.step(0);
    let events = rig.drain();
    assert_eq!(events[1], Event::Connected { model: s85() });
    assert!(
        matches!(events[2], Event::Error(Problem::Device(Error::Invalid(_)))),
        "{events:?}"
    );
    assert_eq!(events.len(), 3);
    assert_eq!(count(&fake, CONFIG_WRITE), 0);
}

#[test]
fn an_unknown_error_while_polling_stays_connected_and_is_told_once() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    for _ in 0..10 {
        fake.queue_feature_reply(&reply(&[0; CONFIG_LEN]));
    }
    rig.step(5000);
    let events = rig.drain();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        Event::Error(Problem::Device(Error::Invalid(_)))
    ));
    rig.step(10_000);
    assert_eq!(rig.drain(), []);
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
    rig.step(1);
    assert_eq!(rig.drain(), [Event::Lighting(current())]);
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
fn resuming_sets_the_clock_only_with_auto_clock_on() {
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
}

#[test]
fn resuming_checks_the_keyboard_is_still_there_right_away() {
    let fake = FakeTransport::new();
    let mut rig = Rig::connected(&fake);
    fake.disconnect();
    rig.send(Cmd::Resumed);
    rig.step(1);
    assert_eq!(rig.drain(), [Event::Disconnected]);
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

    rig.send(Cmd::SyncClock);
    rig.step(2);
    assert_eq!(rig.drain(), [Event::ClockSynced]);
}

#[test]
fn the_estimate_comes_from_the_measured_ack_latency() {
    let fake = FakeTransport::with_auto_ack();
    fake.set_input_latency(Duration::from_millis(10));
    let mut rig = Rig::connected(&fake);

    // Nothing measured yet, so the first upload goes by the usual figure.
    rig.send(Cmd::UploadScreen(still()));
    rig.step(1);
    let events = rig.drain();
    assert_eq!(
        events[0],
        Event::UploadStarted {
            total: S85_STEPS,
            estimate: 215 * DEFAULT_ACK_LATENCY,
        }
    );
    // Every ack took the same, and one step is left after the last chunk.
    let Event::Progress { remaining, .. } = events[S85_STEPS - 1] else {
        panic!("{:?}", events[S85_STEPS - 1]);
    };
    assert_eq!(remaining, Duration::from_millis(10));

    rig.send(Cmd::UploadScreen(still()));
    rig.step(2);
    assert_eq!(
        rig.drain()[0],
        Event::UploadStarted {
            total: S85_STEPS,
            estimate: Duration::from_millis(2150),
        }
    );
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
    let handle = WorkerHandle {
        cmds: rig.cmds.clone(),
        shared: Arc::clone(&rig.worker.shared),
        emitter: rig.worker.emitter.clone(),
        thread: None,
    };

    // Nothing is going on, so this does nothing. Later uploads don't see it.
    handle.send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), []);

    // The worker hasn't got to this yet, and is as good as stuck in an ack wait.
    handle.send(Cmd::UploadScreen(still()));
    handle.send(Cmd::CancelUpload);
    handle.send(Cmd::CancelUpload);
    assert_eq!(rig.drain(), [Event::Cancelling]);

    rig.step(1);
    let events = rig.drain();
    assert_eq!(events.last(), Some(&Event::UploadCancelled));
    // A cancel that's already set stops it before anything is sent.
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
    let unplugged = fake.clone();
    let events_seen = Arc::new(AtomicUsize::new(0));
    let notify: Notify = Arc::new(move || {
        if events_seen.fetch_add(1, Ordering::SeqCst) == 20 {
            unplugged.disconnect();
        }
    });
    let mut rig = Rig::connected_with(&fake, false, notify);
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
    let fake = FakeTransport::with_auto_ack();
    let unplugged = fake.clone();
    let events_seen = Arc::new(AtomicUsize::new(0));
    let notify: Notify = Arc::new(move || {
        if events_seen.fetch_add(1, Ordering::SeqCst) == 20 {
            unplugged.disconnect();
        }
    });
    let mut rig = Rig::connected_with(&fake, false, notify);
    rig.send(Cmd::UploadScreen(still()));
    rig.send(Cmd::SyncClock);
    rig.step(1);
    assert_eq!(rig.drain().last(), Some(&Event::Disconnected));
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
    assert_eq!(image.estimate, 3 * 215 * DEFAULT_ACK_LATENCY);
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
fn only_the_latest_load_gets_told() {
    let (tx, rx) = channel();
    let emitter = Emitter {
        events: tx,
        notify: Arc::new(|| {}),
    };
    let latest = AtomicU64::new(2);
    deliver_load(&emitter, &latest, 1, Err(Error::NotFound));
    assert!(rx.try_recv().is_err());
    deliver_load(&emitter, &latest, 2, Err(Error::NotFound));
    assert_eq!(rx.try_recv(), Ok(Event::ImageFailed(Error::NotFound)));
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
fn shutting_down_stops_an_upload_with_an_abort() {
    let fake = FakeTransport::with_auto_ack();
    queue_reads(&fake, 50);
    let handed_out = fake.clone();
    let open = move || Device::new(handed_out.clone(), None, s85());
    let options = Options {
        auto_clock: false,
        palettes: BTreeMap::new(),
        rk_app_running: Box::new(|| false),
    };
    let (handle, events) = spawn(open, options, Arc::new(|| {}));
    loop {
        let event = events.recv_timeout(Duration::from_secs(30)).unwrap();
        if matches!(event, Event::Connected { .. }) {
            break;
        }
    }
    // Enough frames that it can't be over before the next line.
    let spec = s85().screen.unwrap();
    let long = Animation::from_bytes(&gif(60), &spec, LoadOptions::default()).unwrap();
    handle.send(Cmd::UploadScreen(Arc::new(long)));
    loop {
        let event = events.recv_timeout(Duration::from_secs(30)).unwrap();
        if matches!(event, Event::Progress { .. }) {
            break;
        }
    }
    handle.shutdown();
    let controls: Vec<Vec<u8>> = fake
        .sent()
        .into_iter()
        .filter(|report| report[1] == SCREEN_CONTROL)
        .collect();
    assert_eq!(controls.last().unwrap()[HEADER], 2 << 6);
}
