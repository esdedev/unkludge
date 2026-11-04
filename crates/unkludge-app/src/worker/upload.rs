//! Sending an image to the screen, and what the commands that come meanwhile mean. The top of
//! `worker.rs` has the rules.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::TryRecvError;
use std::time::Instant;

use unkludge_protocol::Error;
use unkludge_protocol::screen::{Animation, Progress, UploadOutcome};
use unkludge_protocol::transport::Transport;

use super::{Cmd, Device, Event, Problem, Worker, estimate};

impl<T: Transport + 'static, F: FnMut() -> Result<Device<T>, Error>> Worker<T, F> {
    /// No upload is going to run. If that came with a cancel, the cancel is what gets told.
    pub(super) fn refuse_upload(&mut self, problem: Problem) {
        if self.shared.end_upload() {
            self.emit(Event::UploadCancelled);
        } else {
            self.problem(problem);
        }
    }

    pub(super) fn upload(&mut self, animation: &Animation, now: Instant) {
        self.flush_adjustment(now);
        self.shared.begin_upload();
        if self.shared.cancelled() {
            self.shared.end_upload();
            self.emit(Event::UploadCancelled);
            return;
        }
        if let Err(problem) = self.writable() {
            self.refuse_upload(problem);
            return;
        }
        let Some(mut device) = self.device.take() else {
            return;
        };
        let plan = match animation.plan(device.model()) {
            Ok(plan) => plan,
            Err(error) => {
                self.device = Some(device);
                self.refuse_upload(Problem::Device(error));
                return;
            }
        };

        self.emit(Event::UploadStarted {
            total: plan.total_steps(),
            estimate: estimate(plan.total_steps(), self.chunk_time.mean()),
        });
        let shared = Arc::clone(&self.shared);
        let uploaded = animation.upload(&mut device, &shared.cancel, |progress| {
            self.between_chunks(progress);
        });
        // Anything that came during the pauses at the end was still during the upload.
        self.commands_during_upload();
        shared.end_upload();
        self.device = Some(device);
        // An upload can take many minutes.
        let now = (self.clock)();

        match uploaded {
            Ok(UploadOutcome { finish_confirmed }) => {
                if !finish_confirmed {
                    self.emit(Event::UploadUnconfirmed);
                }
                self.emit(Event::UploadDone);
            }
            Err(Error::Cancelled) => self.emit(Event::UploadCancelled),
            Err(error) => {
                self.record(&error);
                let maybe_gone = matches!(error, Error::Hid(_));
                self.emit(Event::UploadFailed(error));
                if maybe_gone {
                    self.lose_device_if_gone(now);
                    if self.device.is_none() {
                        return;
                    }
                }
            }
        }
        self.schedule_poll(now);
        self.set_clock_if_due(now);
    }

    fn between_chunks(&mut self, progress: Progress) {
        self.commands_during_upload();
        let steps_left = progress.total.saturating_sub(progress.done);
        self.emit(Event::Progress {
            done: progress.done,
            total: progress.total,
            remaining: estimate(steps_left, self.chunk_time.mean()),
        });
    }

    fn commands_during_upload(&mut self) {
        loop {
            match self.cmds.try_recv() {
                Ok(cmd) => self.during_upload(cmd),
                Err(TryRecvError::Empty) => break,
                // The interface is gone, and the keyboard shouldn't be left in the middle of this.
                Err(TryRecvError::Disconnected) => {
                    self.shared.cancel.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
    }

    /// What each command means while the upload has the keyboard. See the top of `worker.rs`.
    fn during_upload(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SetLighting(_) | Cmd::AdjustLighting(_) | Cmd::UploadScreen(_) => {
                self.problem(Problem::Busy);
            }
            Cmd::SyncClock => self.clock_asked = true,
            Cmd::Resumed => self.auto_clock_due = true,
            Cmd::LoadImage { path, fit, delay } => self.load_image(path, fit, delay),
            Cmd::CancelUpload => self.shared.request_cancel(&self.emitter),
            Cmd::Refresh => {}
            Cmd::SetAutoClock(on) => self.auto_clock = on,
            Cmd::Report => {
                self.tell_report(Err("not read: an upload is running".to_string()));
            }
        }
    }
}
