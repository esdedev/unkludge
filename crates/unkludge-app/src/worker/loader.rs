//! Decoding images, which can take seconds, off the keyboard's thread. One thread does it, one image
//! at a time, and only the newest request waits its turn. A decode can't be stopped once it's going,
//! but one that has been replaced isn't told.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use unkludge_protocol::Error;
use unkludge_protocol::model::Model;
use unkludge_protocol::screen::{Animation, Fit, LoadOptions};

use super::{Emitter, Event, LoadedImage, estimate};

/// What to decode, and what for.
pub(super) struct LoadRequest {
    pub path: PathBuf,
    pub fit: Fit,
    pub delay: Option<u16>,
    pub model: &'static Model,
    pub chunk_time: Duration,
}

/// What does the decoding, which a test can replace with something it can hold up.
pub(super) type Decode = Arc<dyn Fn(&LoadRequest) -> Result<LoadedImage, Error> + Send + Sync>;

#[derive(Default)]
struct Slot {
    /// The number of the newest request. Only its result gets told.
    newest: u64,
    waiting: Option<(u64, LoadRequest)>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The worker's end of the loading thread, which starts with the first request.
pub(super) struct Loader {
    shared: Arc<Shared>,
    emitter: Emitter,
    decode: Decode,
    running: bool,
}

impl Loader {
    pub(super) fn new(emitter: Emitter, decode: Decode) -> Loader {
        Loader {
            shared: Arc::default(),
            emitter,
            decode,
            running: false,
        }
    }

    /// Replaces any request that was waiting, and makes the result of one being decoded go untold.
    pub(super) fn request(&mut self, request: LoadRequest) {
        {
            let mut slot = self.shared.lock();
            slot.newest += 1;
            slot.waiting = Some((slot.newest, request));
        }
        self.shared.changed.notify_one();
        if !self.running {
            self.running = true;
            let (shared, emitter, decode) = (
                Arc::clone(&self.shared),
                self.emitter.clone(),
                Arc::clone(&self.decode),
            );
            thread::spawn(move || run(&shared, &emitter, &decode));
        }
    }

    /// For a request that was refused: whatever came before it is out of date all the same.
    pub(super) fn supersede(&self) {
        let mut slot = self.shared.lock();
        slot.newest += 1;
        slot.waiting = None;
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_one();
    }
}

fn run(shared: &Shared, emitter: &Emitter, decode: &Decode) {
    while let Some((number, request)) = next(shared) {
        let loaded = decode(&request);
        // Telling within the lock means a request that's refused can't be overtaken by this one.
        let slot = shared.lock();
        if slot.newest == number {
            emitter.emit(match loaded {
                Ok(image) => Event::ImageLoaded(image),
                Err(error) => Event::ImageFailed(error),
            });
        }
    }
}

/// Waits for a request. `None` means there will be no more.
fn next(shared: &Shared) -> Option<(u64, LoadRequest)> {
    let mut slot = shared.lock();
    loop {
        if slot.closed {
            return None;
        }
        if let Some(request) = slot.waiting.take() {
            return Some(request);
        }
        slot = shared
            .changed
            .wait(slot)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Decodes an image for its model's screen. Frames past the model's limit are cut.
pub(super) fn decode_image(request: &LoadRequest) -> Result<LoadedImage, Error> {
    let model = request.model;
    let screen = model.screen.ok_or(Error::Unsupported(model.pid))?;
    let options = LoadOptions {
        fit: request.fit,
        delay_ms: request.delay,
        truncate: true,
    };
    let animation = Animation::from_file(&request.path, &screen, options)?;
    let plan = animation.plan(model)?;
    let estimate = estimate(plan.total_steps(), request.chunk_time);
    Ok(LoadedImage {
        path: request.path.clone(),
        animation: Arc::new(animation),
        plan,
        estimate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::model;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};

    const WAIT: Duration = Duration::from_secs(30);

    fn request(name: &str) -> LoadRequest {
        LoadRequest {
            path: PathBuf::from(name),
            fit: Fit::Cover,
            delay: None,
            model: model::find(0x0265).unwrap(),
            chunk_time: Duration::ZERO,
        }
    }

    /// A decoder that holds each decode up until the test lets it go on, and fails with the file's
    /// name, which is an easy event to compare.
    struct Gate {
        started: Receiver<String>,
        release: Sender<()>,
        running: Arc<AtomicUsize>,
        most_at_once: Arc<AtomicUsize>,
        decode: Decode,
    }

    fn gate() -> Gate {
        let (started_tx, started) = channel();
        let (release, release_rx) = channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let running = Arc::new(AtomicUsize::new(0));
        let most_at_once = Arc::new(AtomicUsize::new(0));
        let (now, most) = (Arc::clone(&running), Arc::clone(&most_at_once));
        let decode: Decode = Arc::new(move |request: &LoadRequest| {
            let at_once = now.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(at_once, Ordering::SeqCst);
            let name = request.path.display().to_string();
            started_tx.send(name.clone()).unwrap();
            release_rx.lock().unwrap().recv_timeout(WAIT).unwrap();
            now.fetch_sub(1, Ordering::SeqCst);
            Err(Error::Image(name))
        });
        Gate {
            started,
            release,
            running,
            most_at_once,
            decode,
        }
    }

    fn loader(decode: Decode) -> (Loader, Receiver<Event>) {
        let (events_tx, events) = channel();
        let emitter = Emitter {
            events: events_tx,
            notify: Arc::new(|| {}),
        };
        (Loader::new(emitter, decode), events)
    }

    #[test]
    fn a_burst_of_requests_decodes_only_the_first_and_the_last_one_at_a_time() {
        let gate = gate();
        let (mut loader, events) = loader(Arc::clone(&gate.decode));

        loader.request(request("first"));
        assert_eq!(gate.started.recv_timeout(WAIT).unwrap(), "first");
        for number in 0..40 {
            loader.request(request(&format!("request {number}")));
        }
        assert_eq!(gate.running.load(Ordering::SeqCst), 1);

        // The first ends, and what was last asked for goes next. Nothing in between.
        gate.release.send(()).unwrap();
        assert_eq!(gate.started.recv_timeout(WAIT).unwrap(), "request 39");
        gate.release.send(()).unwrap();
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            Event::ImageFailed(Error::Image("request 39".to_string()))
        );
        assert_eq!(gate.most_at_once.load(Ordering::SeqCst), 1);
        assert!(gate.started.try_recv().is_err());
        assert!(events.try_recv().is_err(), "the first isn't told");
    }

    #[test]
    fn a_decode_that_nothing_replaced_is_told() {
        let gate = gate();
        let (mut loader, events) = loader(Arc::clone(&gate.decode));
        loader.request(request("only"));
        gate.started.recv_timeout(WAIT).unwrap();
        gate.release.send(()).unwrap();
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            Event::ImageFailed(Error::Image("only".to_string()))
        );
    }

    #[test]
    fn a_refused_request_replaces_those_before_it() {
        let gate = gate();
        let (mut loader, events) = loader(Arc::clone(&gate.decode));
        loader.request(request("running"));
        gate.started.recv_timeout(WAIT).unwrap();
        loader.request(request("waiting"));
        loader.supersede();
        gate.release.send(()).unwrap();

        // The one that was waiting doesn't get decoded for nothing.
        let not_started = gate.started.recv_timeout(Duration::from_millis(200));
        assert!(not_started.is_err(), "{not_started:?}");

        // A later request's result gets through, showing that the thread has moved on.
        loader.request(request("later"));
        assert_eq!(gate.started.recv_timeout(WAIT).unwrap(), "later");
        gate.release.send(()).unwrap();
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            Event::ImageFailed(Error::Image("later".to_string()))
        );
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn the_thread_ends_with_the_loader() {
        let finished = Arc::new(Mutex::new(false));
        let shared = Arc::<Shared>::default();
        let (events_tx, _events) = channel();
        let emitter = Emitter {
            events: events_tx,
            notify: Arc::new(|| {}),
        };
        let thread = {
            let (shared, finished) = (Arc::clone(&shared), Arc::clone(&finished));
            let decode: Decode = Arc::new(|_: &LoadRequest| Err(Error::NotFound));
            thread::spawn(move || {
                run(&shared, &emitter, &decode);
                *finished.lock().unwrap() = true;
            })
        };
        let (dropped_events, _rx) = channel();
        let dropping = Loader {
            shared: Arc::clone(&shared),
            emitter: Emitter {
                events: dropped_events,
                notify: Arc::new(|| {}),
            },
            decode: Arc::new(|_: &LoadRequest| Err(Error::NotFound)),
            running: true,
        };
        drop(dropping);
        thread.join().unwrap();
        assert!(*finished.lock().unwrap());
    }
}
