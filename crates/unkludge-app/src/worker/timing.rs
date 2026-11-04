//! Keeping time: how long a chunk of an upload takes, and whether the keyboard's thread has got stuck.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use unkludge_protocol::command_byte::SCREEN_DATA;
use unkludge_protocol::device::PacketEvent;

use super::{Emitter, Event};

/// The time per chunk to assume until one has been measured: uploads were measured at 5 to 6 chunks
/// a second on an S85 ES.
pub(super) const DEFAULT_CHUNK_TIME: Duration = Duration::from_millis(180);

/// How long a chunk takes, start to end: the time between one chunk's ack and the next's, resends
/// included. The start and finish reports don't count, which keeps their pauses out of it. Measured
/// over the whole session, so reconnecting doesn't lose it.
#[derive(Default)]
pub(super) struct ChunkTime {
    count: AtomicU64,
    total_micros: AtomicU64,
}

impl ChunkTime {
    fn record(&self, time: Duration) {
        let micros = u64::try_from(time.as_micros()).unwrap_or(u64::MAX);
        self.total_micros.fetch_add(micros, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn mean(&self) -> Duration {
        let total = self.total_micros.load(Ordering::Relaxed);
        match self.count.load(Ordering::Relaxed) {
            0 => DEFAULT_CHUNK_TIME,
            count => Duration::from_micros(total / count),
        }
    }
}

/// Turns what a device reports of its packets into [`ChunkTime`]s.
#[derive(Default)]
pub(super) struct ChunkTimer {
    /// The last report sent is a chunk that hasn't been accepted yet.
    waiting: bool,
    last_ack: Option<Instant>,
}

impl ChunkTimer {
    pub(super) fn on_packet(&mut self, event: PacketEvent<'_>, now: Instant, time: &ChunkTime) {
        match event {
            PacketEvent::Sent { report, .. } => {
                self.waiting = report.get(1) == Some(&SCREEN_DATA);
                if !self.waiting {
                    self.last_ack = None;
                }
            }
            PacketEvent::Ack { accepted: true, .. } if self.waiting => {
                if let Some(last) = self.last_ack {
                    time.record(now.saturating_duration_since(last));
                }
                self.last_ack = Some(now);
                self.waiting = false;
            }
            PacketEvent::Ack { .. } => {}
        }
    }
}

/// How long until things count as stuck, and how often to look.
#[derive(Clone, Copy)]
pub(super) struct WatchTimes {
    /// No single device operation should come near this: an ack waits 8 s at most.
    pub hung_after: Duration,
    pub check_every: Duration,
}

/// What the keyboard's thread says about itself, for the watchdog.
#[derive(Default)]
pub(super) struct Beat {
    /// Since when it has been working with no sign of progress. `None` when it's idle.
    busy_since: Mutex<Option<Instant>>,
    closed: AtomicBool,
}

impl Beat {
    /// It's at work, and has just got somewhere.
    pub(super) fn busy(&self) {
        self.set(Some(Instant::now()));
    }

    pub(super) fn idle(&self) {
        self.set(None);
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn set(&self, since: Option<Instant>) {
        *self
            .busy_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = since;
    }

    fn stuck(&self, limit: Duration) -> bool {
        self.busy_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|since| since.elapsed() >= limit)
    }
}

/// A read on Windows has no timeout, so a keyboard that stops answering holds the worker's thread up
/// with no error to show for it. This thread notices, and tells once for each time it happens.
pub(super) fn watch(beat: &Arc<Beat>, emitter: &Emitter, times: WatchTimes) {
    let mut hung = false;
    while !beat.closed.load(Ordering::SeqCst) {
        thread::sleep(times.check_every);
        let stuck = beat.stuck(times.hung_after);
        if stuck != hung {
            hung = stuck;
            emitter.emit(if stuck {
                Event::Hung
            } else {
                Event::Responding
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::device::Collection;

    const CHUNK: &[u8] = &[0x09, 0x0C];
    const CONTROL: &[u8] = &[0x09, 0x0D];

    fn sent(report: &[u8]) -> PacketEvent<'_> {
        PacketEvent::Sent {
            collection: Collection::Main,
            report,
        }
    }

    fn ack(accepted: bool) -> PacketEvent<'static> {
        PacketEvent::Ack {
            idx: 0,
            accepted,
            latency: Duration::from_secs(99),
        }
    }

    #[test]
    fn until_something_is_measured_the_usual_figure_stands() {
        assert_eq!(ChunkTime::default().mean(), DEFAULT_CHUNK_TIME);
    }

    #[test]
    fn a_chunks_time_runs_from_the_ack_before_it_and_the_mean_is_over_all() {
        let time = ChunkTime::default();
        let mut timer = ChunkTimer::default();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);

        // The start report and its ack aren't chunks.
        timer.on_packet(sent(CONTROL), at(0), &time);
        timer.on_packet(ack(true), at(500), &time);
        // The first chunk has no chunk before it to count from.
        timer.on_packet(sent(CHUNK), at(600), &time);
        timer.on_packet(ack(true), at(650), &time);
        assert_eq!(time.mean(), DEFAULT_CHUNK_TIME);

        // A chunk that had to go twice takes as long as that took, however soon it was acked.
        timer.on_packet(sent(CHUNK), at(660), &time);
        timer.on_packet(ack(false), at(700), &time);
        timer.on_packet(sent(CHUNK), at(710), &time);
        timer.on_packet(ack(true), at(750), &time);
        assert_eq!(time.mean(), Duration::from_millis(100));

        timer.on_packet(sent(CHUNK), at(751), &time);
        timer.on_packet(ack(true), at(950), &time);
        assert_eq!(time.mean(), Duration::from_millis(150));

        // The finish, with whatever pause comes before it, doesn't count.
        timer.on_packet(sent(CONTROL), at(5000), &time);
        timer.on_packet(ack(true), at(9000), &time);
        assert_eq!(time.mean(), Duration::from_millis(150));

        // Nor does the gap up to the next upload's first chunk.
        timer.on_packet(sent(CHUNK), at(20_000), &time);
        timer.on_packet(ack(true), at(20_100), &time);
        assert_eq!(time.mean(), Duration::from_millis(150));
    }

    fn watched(beat: &Arc<Beat>) -> std::sync::mpsc::Receiver<Event> {
        let (events_tx, events) = std::sync::mpsc::channel();
        let emitter = Emitter {
            events: events_tx,
            notify: Arc::new(|| {}),
        };
        let times = WatchTimes {
            hung_after: Duration::from_millis(40),
            check_every: Duration::from_millis(2),
        };
        let beat = Arc::clone(beat);
        thread::spawn(move || watch(&beat, &emitter, times));
        events
    }

    const WAIT: Duration = Duration::from_secs(30);

    #[test]
    fn being_stuck_is_told_once_until_things_move_again() {
        let beat = Arc::<Beat>::default();
        let events = watched(&beat);

        beat.busy();
        assert_eq!(events.recv_timeout(WAIT), Ok(Event::Hung));
        // Longer than it took to tell: it doesn't say it again.
        thread::sleep(Duration::from_millis(120));
        assert!(events.try_recv().is_err());

        beat.busy();
        assert_eq!(events.recv_timeout(WAIT), Ok(Event::Responding));
        assert_eq!(events.recv_timeout(WAIT), Ok(Event::Hung));
        beat.idle();
        assert_eq!(events.recv_timeout(WAIT), Ok(Event::Responding));
        beat.close();
    }

    #[test]
    fn being_idle_for_long_isnt_being_stuck() {
        let beat = Arc::<Beat>::default();
        let events = watched(&beat);
        beat.idle();
        thread::sleep(Duration::from_millis(120));
        assert!(events.try_recv().is_err());
        beat.close();
    }

    #[test]
    fn signs_of_progress_keep_it_from_counting_as_stuck() {
        let beat = Arc::<Beat>::default();
        let events = watched(&beat);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(200) {
            beat.busy();
            thread::sleep(Duration::from_millis(2));
        }
        beat.idle();
        assert!(events.try_recv().is_err());
        beat.close();
    }

    #[test]
    fn the_watchdog_stops_when_closed() {
        let beat = Arc::<Beat>::default();
        let (events_tx, _events) = std::sync::mpsc::channel();
        let emitter = Emitter {
            events: events_tx,
            notify: Arc::new(|| {}),
        };
        let times = WatchTimes {
            hung_after: Duration::from_secs(60),
            check_every: Duration::from_millis(1),
        };
        let watching = {
            let beat = Arc::clone(&beat);
            thread::spawn(move || watch(&beat, &emitter, times))
        };
        beat.close();
        watching.join().unwrap();
    }
}
