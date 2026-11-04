//! The demo's pictures, drawn with no window at all: a platform of Slint's that has no desktop to show
//! anything on, so that nothing else can end up in a picture and it works with no display. Slint draws
//! to a bitmap here as it does to a window, as the real window is drawn in software too. The tests use
//! this platform as well.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::{EventLoopError, PlatformError, SharedPixelBuffer};

use crate::winres;

/// How long to sleep at most with nothing to do, in case the timers didn't say.
const LONGEST_NAP: Duration = Duration::from_millis(100);

type Task = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Queue {
    tasks: Mutex<(VecDeque<Task>, bool)>,
    woken: Condvar,
}

impl Queue {
    fn tasks(&self) -> std::sync::MutexGuard<'_, (VecDeque<Task>, bool)> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Proxy(Arc<Queue>);

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        self.0.tasks().1 = true;
        self.0.woken.notify_all();
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: Task) -> Result<(), EventLoopError> {
        self.0.tasks().0.push_back(event);
        self.0.woken.notify_all();
        Ok(())
    }
}

/// See the module's description.
#[derive(Default)]
struct Headless {
    queue: Arc<Queue>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        loop {
            slint::platform::update_timers_and_animations();
            let wait = slint::platform::duration_until_next_timer_update()
                .map_or(LONGEST_NAP, |next| next.min(LONGEST_NAP));
            let mut tasks = self.queue.tasks();
            if tasks.0.is_empty() && !tasks.1 {
                tasks = self
                    .queue
                    .woken
                    .wait_timeout(tasks, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            let quit = tasks.1;
            let due: Vec<Task> = tasks.0.drain(..).collect();
            drop(tasks);
            for task in due {
                task();
            }
            if quit {
                return Ok(());
            }
        }
    }

    /// There's one of these per process, and tests have a thread and a platform each, with no event
    /// loop to reach.
    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        if cfg!(test) {
            return None;
        }
        Some(Box::new(Proxy(Arc::clone(&self.queue))))
    }
}

/// Has this thread use the platform with no desktop. Only the first call on a thread counts, and this
/// has to come before making any window.
pub fn use_headless_platform() {
    let _ = slint::platform::set_platform(Box::<Headless>::default());
}

/// `R G B A` pixels as the `B G R A` that the PNG writer takes, all opaque.
fn bgra_from_rgba(rgba: &[u8]) -> Vec<u8> {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, _]| [b, g, r, 255])
        .collect()
}

/// Writes a window's picture to a PNG file.
pub fn save(window: &slint::Window, path: &Path) -> io::Result<()> {
    let snapshot: SharedPixelBuffer<slint::Rgba8Pixel> = window
        .take_snapshot()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let pixels = bgra_from_rgba(snapshot.as_bytes());
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(
        path,
        winres::png(
            snapshot.width() as usize,
            snapshot.height() as usize,
            &pixels,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_go_into_the_order_the_png_writer_takes() {
        assert_eq!(bgra_from_rgba(&[1, 2, 3, 0]), [3, 2, 1, 255]);
        assert_eq!(
            bgra_from_rgba(&[1, 2, 3, 9, 4, 5, 6, 200]),
            [3, 2, 1, 255, 6, 5, 4, 255]
        );
    }
}
