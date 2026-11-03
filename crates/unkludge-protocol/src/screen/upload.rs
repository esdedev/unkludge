//! Sending an animation to the keyboard: a start report, the chunks of each frame, then a finish
//! report. Each of them waits for an ack, so the keyboard sets the pace.
//!
//! On models with a second collection, chunks go only to that one, and the start and finish go to
//! the main collection and then to the second.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{Animation, chunks_per_frame};
use crate::device::AckOutcome;
use crate::model::Model;
use crate::packet::{MAX_DATA, ScreenState, cmd, screen_chunk, screen_control};
use crate::transport::Transport;
use crate::{Device, Error};

/// The frame number and the chunk index each have one byte in the report.
const MAX_ON_THE_WIRE: usize = 256;
/// Below this many frames, an animation is never thinned out.
const DECIMATION_MIN_FRAMES: usize = 7;

/// One send and three retries.
const START_TRIES: u32 = 4;
const START_RETRY_GAP: Duration = Duration::from_millis(200);
const BEFORE_START: Duration = Duration::from_millis(100);
const AFTER_START: Duration = Duration::from_millis(120);
const BEFORE_END: Duration = Duration::from_millis(50);
const AFTER_END: Duration = Duration::from_millis(500);

/// How far an upload has got. There is one step for the start, one for each chunk and one for the
/// finish, whatever the resends or the number of collections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// How a finished upload ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadOutcome {
    /// Whether the keyboard acknowledged the finish report. Every frame has been acknowledged
    /// either way, but without this the keyboard may not show the new animation.
    pub finish_confirmed: bool,
}

/// What an upload of an animation to a given model would send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadPlan {
    /// The animation's frames that get sent. They go out numbered from 0.
    pub frame_indexes: Vec<usize>,
    pub chunks_per_frame: usize,
    /// The delay in the start report: the delay less the model's draw time, and at least 0.
    pub start_delay_ms: u16,
    /// The delay in the finish report: the animation's own.
    pub finish_delay_ms: u16,
}

impl UploadPlan {
    /// The final number of frames, which may be fewer than the animation has.
    pub fn frame_count(&self) -> usize {
        self.frame_indexes.len()
    }

    /// The chunks to send, each of which waits for an ack. Multiply by the time an ack takes to
    /// estimate the upload's duration.
    pub fn total_chunks(&self) -> usize {
        self.frame_count() * self.chunks_per_frame
    }

    /// What [`Progress::total`] will be.
    pub fn total_steps(&self) -> usize {
        self.total_chunks() + 2
    }
}

impl Animation {
    /// Works out what uploading to `model` would send, without sending anything.
    ///
    /// [`Error::Invalid`] if the animation isn't the size of the model's screen, or would need
    /// more frames or chunks than fit in their one-byte fields. [`Error::TooManyFrames`] if it has
    /// more frames than the model's limit, counted before thinning out.
    pub fn plan(&self, model: &Model) -> Result<UploadPlan, Error> {
        let screen = model.screen.ok_or(Error::Unsupported(model.pid))?;
        if (self.width, self.height) != (screen.width, screen.height) {
            return Err(Error::Invalid(format!(
                "the animation is {}x{}, but this keyboard's screen is {}x{}",
                self.width, self.height, screen.width, screen.height
            )));
        }
        let max = usize::from(screen.max_frames);
        if self.frames.len() > max {
            return Err(Error::TooManyFrames {
                got: self.frames.len(),
                max,
            });
        }
        let frame_indexes = decimate(self.frames.len(), self.delay_ms, screen.draw_frame_time);
        if frame_indexes.len() > MAX_ON_THE_WIRE {
            return Err(Error::Invalid(format!(
                "{} frames don't fit the frame number's byte",
                frame_indexes.len()
            )));
        }
        let chunks = chunks_per_frame(self.width, self.height);
        if chunks > MAX_ON_THE_WIRE {
            return Err(Error::Invalid(format!(
                "{chunks} chunks per frame don't fit the chunk index's byte"
            )));
        }
        Ok(UploadPlan {
            frame_indexes,
            chunks_per_frame: chunks,
            start_delay_ms: self.delay_ms.saturating_sub(screen.draw_frame_time),
            finish_delay_ms: self.delay_ms,
        })
    }

    /// Sends the animation to the keyboard's screen, replacing what's there. `progress` is called
    /// after each step. Setting `cancel` stops the upload between chunks with [`Error::Cancelled`].
    ///
    /// Nothing is sent if [`Animation::plan`] refuses, or if `cancel` is already set. If the
    /// keyboard doesn't take the start report, no chunks are sent. Once it has, any failure or a
    /// cancel makes the keyboard abort, as far as it still answers, and the first error is returned.
    ///
    /// A cancel isn't noticed during an ack wait, which goes on until the ack or its timeout, and
    /// then the abort has an ack wait of its own. On a stalled keyboard that's up to 16 s, so a UI
    /// should show that it's cancelling.
    pub fn upload<T: Transport>(
        &self,
        device: &mut Device<T>,
        cancel: &AtomicBool,
        mut progress: impl FnMut(Progress),
    ) -> Result<UploadOutcome, Error> {
        let plan = self.plan(device.model())?;
        check_cancel(cancel)?;

        let frames = plan.frame_count() as u16;
        let total = plan.total_steps();
        let mut done = 0;
        let mut step = || {
            done += 1;
            progress(Progress { done, total });
        };

        device.sleep(BEFORE_START);
        if let Err(error) = start(device, frames, plan.start_delay_ms, cancel) {
            device.sleep(AFTER_END);
            return Err(error);
        }
        step();
        device.sleep(AFTER_START);

        let sent = self.send_frames(device, &plan, cancel, &mut step);
        device.sleep(BEFORE_END);
        if let Err(error) = sent {
            // Best effort: this may be why the upload failed, and the first error says more. With
            // two collections, one that doesn't confirm keeps the abort from reaching the other.
            let abort = screen_control(ScreenState::Abort, frames, plan.finish_delay_ms);
            let _ = device.send_screen_control(&abort);
            device.sleep(AFTER_END);
            return Err(error);
        }

        // The official app ignores how this ends, and every frame has been acknowledged by now, so
        // neither a timeout nor a refusal fails the upload: the outcome reports them. Not being able
        // to send it at all is still an error.
        let finish = screen_control(ScreenState::Finish, frames, plan.finish_delay_ms);
        let finished = device.send_screen_control(&finish);
        if finished.is_ok() {
            step();
        }
        device.sleep(AFTER_END);
        Ok(UploadOutcome {
            finish_confirmed: finished? == AckOutcome::Confirmed,
        })
    }

    fn send_frames<T: Transport>(
        &self,
        device: &mut Device<T>,
        plan: &UploadPlan,
        cancel: &AtomicBool,
        step: &mut impl FnMut(),
    ) -> Result<(), Error> {
        let crc = device.model().screen.is_some_and(|s| s.crc);
        for (number, &source) in plan.frame_indexes.iter().enumerate() {
            check_cancel(cancel)?;
            for (idx, payload) in self.frames[source].chunks(MAX_DATA).enumerate() {
                // The plan has checked that both fit in a byte.
                device.send_screen_chunk(&screen_chunk(number as u8, idx as u8, payload, crc))?;
                step();
                check_cancel(cancel)?;
            }
        }
        Ok(())
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Sends the start report. No ack in time tries again, and a refusal ends it there. No abort
/// follows a failure here, as in the official app, although with two collections the main one may
/// have confirmed the start and so stays started.
fn start<T: Transport>(
    device: &mut Device<T>,
    frames: u16,
    delay_ms: u16,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let report = screen_control(ScreenState::Start, frames, delay_ms);
    let command = cmd::SCREEN_CONTROL.byte();
    for attempt in 0..START_TRIES {
        if attempt > 0 {
            device.sleep(START_RETRY_GAP);
            check_cancel(cancel)?;
        }
        match device.send_screen_control(&report)? {
            AckOutcome::Confirmed => return Ok(()),
            AckOutcome::Rejected => {
                return Err(Error::Rejected {
                    cmd: command,
                    idx: 0,
                });
            }
            AckOutcome::TimedOut => {}
        }
    }
    Err(Error::NoAck {
        cmd: command,
        idx: 0,
    })
}

/// Which frames to send, worked out as the official app does: an animation faster than
/// `draw_frame_time` loses frames, keeping as many as that time fits into its running time, evenly
/// spaced and rounded to the nearest, with the first and last among them. Down to one frame, only
/// the first stays. Short animations are left alone. Why the firmware would want this is
/// unconfirmed.
fn decimate(frames: usize, delay_ms: u16, draw_frame_time: u16) -> Vec<usize> {
    let draw_time = usize::from(draw_frame_time);
    let running_time = usize::from(delay_ms) * frames;
    let keep = if frames >= DECIMATION_MIN_FRAMES && delay_ms < draw_frame_time {
        match running_time / draw_time {
            0 => frames,
            fitting => fitting,
        }
    } else {
        frames
    };
    match keep {
        1 => vec![0],
        _ if keep == frames => (0..frames).collect(),
        // `i * step + 0.5`, rounded down, with `step = (frames - 1) / (keep - 1)`.
        _ => (0..keep)
            .map(|i| (2 * i * (frames - 1) + keep - 1) / (2 * (keep - 1)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Collection, PacketEvent};
    use crate::model::{self, Protocol, ScreenSpec, Status};
    use crate::packet::REPORT_LEN;
    use crate::screen::LoadOptions;
    use crate::transport::fake::{AckFault, FakeTransport};
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const RED: [u8; 2] = [0xF8, 0x00];
    const BLUE: [u8; 2] = [0x00, 0x1F];

    fn s85() -> &'static Model {
        model::find(0x0265).unwrap()
    }

    fn m87() -> &'static Model {
        model::find(0x01A2).unwrap()
    }

    fn n99() -> &'static Model {
        model::find(0x01B8).unwrap()
    }

    /// An animation of solid frames, one per colour, of the size of `model`'s screen.
    fn animation(model: &Model, colors: &[[u8; 2]], delay_ms: u16) -> Animation {
        let screen = model.screen.unwrap();
        let pixels = usize::from(screen.width) * usize::from(screen.height);
        Animation {
            frames: colors.iter().map(|color| color.repeat(pixels)).collect(),
            delay_ms,
            width: screen.width,
            height: screen.height,
            truncated: false,
        }
    }

    /// A model like the S85 with another screen, for limits no real model reaches.
    fn with_screen(screen: ScreenSpec) -> &'static Model {
        Box::leak(Box::new(Model {
            pid: 0xFFFF,
            name: "Test",
            protocol: Protocol::V2,
            status: Status::Experimental,
            screen: Some(screen),
            lighting: s85().lighting,
            layout: None,
        }))
    }

    fn device(fake: &FakeTransport) -> Device<FakeTransport> {
        Device::new(fake.clone(), None, s85()).unwrap()
    }

    fn red_blue() -> Animation {
        animation(s85(), &[RED, BLUE], 200)
    }

    fn upload(fake: &FakeTransport, anim: &Animation) -> Result<UploadOutcome, Error> {
        anim.upload(&mut device(fake), &AtomicBool::new(false), |_| {})
    }

    /// An upload that cancels itself once `done` steps are done.
    fn upload_cancelling_at(
        fake: &FakeTransport,
        anim: &Animation,
        at: usize,
    ) -> Result<UploadOutcome, Error> {
        let cancel = AtomicBool::new(false);
        anim.upload(&mut device(fake), &cancel, |progress| {
            if progress.done == at {
                cancel.store(true, Ordering::Relaxed);
            }
        })
    }

    fn is_control(report: &[u8], first_byte: u8) -> bool {
        report[1] == 0x0D && report[8] == first_byte
    }

    /// Reads the fixture format: a name, then bytes, with `F800*256` as a repeat. Zero-filled.
    fn fixture() -> Vec<(String, Vec<u8>)> {
        include_str!("../../tests/fixtures/screen_red_blue.hex")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                let mut tokens = line.split_whitespace();
                let name = tokens.next().unwrap().to_string();
                let mut report = Vec::new();
                for token in tokens {
                    let (hex, count) = token.split_once('*').unwrap_or((token, "1"));
                    let bytes: Vec<u8> = (0..hex.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                        .collect();
                    report.extend(bytes.repeat(count.parse().unwrap()));
                }
                report.resize(REPORT_LEN, 0);
                (name, report)
            })
            .collect()
    }

    /// A GIF of solid frames, from the top of the pipeline.
    fn gif(colors: &[[u8; 4]], delay_ms: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for &color in colors {
                let image = RgbaImage::from_pixel(32, 32, Rgba(color));
                let delay = Delay::from_numer_denom_ms(delay_ms, 1);
                encoder
                    .encode_frame(Frame::from_parts(image, 0, 0, delay))
                    .unwrap();
            }
        }
        bytes
    }

    // The whole flow.

    #[test]
    fn a_two_frame_upload_matches_the_fixture_all_the_way_from_a_gif() {
        let bytes = gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 200);
        let screen = s85().screen.unwrap();
        let anim = Animation::from_bytes(&bytes, &screen, LoadOptions::default()).unwrap();
        let fake = FakeTransport::with_auto_ack();
        upload(&fake, &anim).unwrap();

        let sent = fake.sent();
        assert_eq!(sent.len(), 432);
        let positions = [
            ("start", 0),
            ("frame0-first", 1),
            ("frame0-last", 215),
            ("frame1-first", 216),
            ("frame1-last", 430),
            ("finish", 431),
        ];
        let expected = fixture();
        assert_eq!(expected.len(), positions.len());
        for ((name, position), (fixture_name, report)) in positions.iter().zip(&expected) {
            assert_eq!(name, fixture_name);
            assert!(
                sent[*position] == *report,
                "{name} differs from the fixture"
            );
        }
    }

    #[test]
    fn chunks_go_out_in_order_with_their_frame_and_index() {
        let fake = FakeTransport::with_auto_ack();
        upload(&fake, &red_blue()).unwrap();
        let sent = fake.sent();
        let chunks = &sent[1..431];
        let expected: Vec<(u8, u8)> = (0..2u8)
            .flat_map(|frame| (0..215u8).map(move |idx| (frame, idx)))
            .collect();
        let actual: Vec<(u8, u8)> = chunks.iter().map(|r| (r[3], r[4])).collect();
        assert_eq!(actual, expected);
        assert!(chunks.iter().all(|r| r[1] == 0x0C && r[5] == 0));
    }

    #[test]
    fn the_only_pauses_are_the_ones_of_the_protocol() {
        let fake = FakeTransport::with_auto_ack();
        upload(&fake, &red_blue()).unwrap();
        assert_eq!(
            fake.sleeps(),
            [100, 25, 120, 50, 25, 500].map(Duration::from_millis)
        );
    }

    #[test]
    fn one_frame_takes_the_same_flow() {
        let fake = FakeTransport::with_auto_ack();
        upload(&fake, &animation(s85(), &[RED], 200)).unwrap();
        let sent = fake.sent();
        assert_eq!(sent.len(), 217);
        assert_eq!(&sent[0][8..13], &[0x40, 1, 0, 20, 0]);
        assert_eq!(&sent[216][8..13], &[0x00, 1, 0, 200, 0]);
    }

    #[test]
    fn progress_counts_each_step_once() {
        let fake = FakeTransport::with_auto_ack();
        // A resend isn't a step.
        fake.ack_fault(3, AckFault::Reject);
        let mut seen = Vec::new();
        red_blue()
            .upload(&mut device(&fake), &AtomicBool::new(false), |progress| {
                seen.push(progress);
            })
            .unwrap();
        let expected: Vec<Progress> = (1..=432)
            .map(|done| Progress { done, total: 432 })
            .collect();
        assert_eq!(seen, expected);
    }

    // Trouble with chunks.

    #[test]
    fn a_refused_chunk_is_sent_again() {
        let fake = FakeTransport::with_auto_ack();
        // Ack 0 is the start's, so this is chunk 2.
        fake.ack_fault(3, AckFault::Reject);
        upload(&fake, &red_blue()).unwrap();
        let sent = fake.sent();
        assert_eq!(sent.len(), 433);
        assert_eq!(sent[3], sent[4]);
        assert_eq!(sent[3][4], 2);
        assert_eq!(sent[5][4], 3);
    }

    #[test]
    fn a_chunk_with_no_ack_aborts_and_returns_that_error() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(5, AckFault::Drop);
        let result = upload(&fake, &red_blue());
        assert_eq!(result, Err(Error::NoAck { cmd: 0x0C, idx: 4 }));

        let sent = fake.sent();
        assert_eq!(sent.len(), 7);
        assert_eq!(&sent[6][..13], &fixture_abort());
        assert_eq!(
            fake.sleeps(),
            [100, 25, 120, 50, 25, 500].map(Duration::from_millis)
        );
    }

    /// The abort report of the red and blue animation: state 2, 2 frames, delay 200.
    fn fixture_abort() -> [u8; 13] {
        [
            0x09, 0x0D, 0x00, 0x00, 0x01, 0x00, 0x05, 0x00, 0x80, 2, 0, 200, 0,
        ]
    }

    #[test]
    fn an_abort_carries_the_frame_count_after_decimation() {
        let colors = vec![RED; 7];
        // 7 frames at 60 ms leave 2.
        let fake = FakeTransport::with_auto_ack();
        let result = upload_cancelling_at(&fake, &animation(s85(), &colors, 60), 1);
        assert_eq!(result, Err(Error::Cancelled));
        assert_eq!(&fake.sent()[1][8..13], &[0x80, 2, 0, 60, 0]);
    }

    #[test]
    fn a_chunk_refused_every_time_aborts_and_returns_that_error() {
        let fake = FakeTransport::with_auto_ack();
        (1..7).for_each(|n| fake.ack_fault(n, AckFault::Reject));
        let result = upload(&fake, &red_blue());
        assert_eq!(result, Err(Error::Rejected { cmd: 0x0C, idx: 0 }));
        let sent = fake.sent();
        assert_eq!(sent.len(), 8);
        assert_eq!(&sent[7][..13], &fixture_abort());
    }

    #[test]
    fn a_failed_abort_doesnt_hide_the_error() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(1, AckFault::Drop);
        fake.ack_fault(2, AckFault::Drop);
        let result = upload(&fake, &red_blue());
        assert_eq!(result, Err(Error::NoAck { cmd: 0x0C, idx: 0 }));

        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(1, AckFault::Drop);
        fake.ack_fault(2, AckFault::Reject);
        let result = upload(&fake, &red_blue());
        assert_eq!(result, Err(Error::NoAck { cmd: 0x0C, idx: 0 }));
    }

    #[test]
    fn a_chunk_that_cant_be_sent_aborts_too() {
        let fake = FakeTransport::with_auto_ack();
        // Send 0 is the start.
        (1..4).for_each(|n| fake.fail_send(n));
        let result = upload(&fake, &red_blue());
        assert!(matches!(result, Err(Error::Hid(_))));
        let sent = fake.sent();
        assert_eq!(sent.len(), 2);
        assert!(is_control(&sent[1], 0x80));
    }

    // Cancelling.

    #[test]
    fn cancelling_after_a_chunk_aborts_and_sends_nothing_more() {
        let fake = FakeTransport::with_auto_ack();
        // The start and then 10 chunks.
        let result = upload_cancelling_at(&fake, &red_blue(), 11);
        assert_eq!(result, Err(Error::Cancelled));
        let sent = fake.sent();
        assert_eq!(sent.len(), 12);
        assert_eq!(sent[10][4], 9);
        assert_eq!(&sent[11][..13], &fixture_abort());
        assert_eq!(
            fake.sleeps(),
            [100, 25, 120, 50, 25, 500].map(Duration::from_millis)
        );
    }

    #[test]
    fn cancelling_before_the_first_frame_aborts_with_no_chunks() {
        let fake = FakeTransport::with_auto_ack();
        let result = upload_cancelling_at(&fake, &red_blue(), 1);
        assert_eq!(result, Err(Error::Cancelled));
        let sent = fake.sent();
        assert_eq!(sent.len(), 2);
        assert!(is_control(&sent[0], 0x40));
        assert!(is_control(&sent[1], 0x80));
    }

    #[test]
    fn cancelling_after_the_last_chunk_aborts_instead_of_finishing() {
        let fake = FakeTransport::with_auto_ack();
        let result = upload_cancelling_at(&fake, &red_blue(), 431);
        assert_eq!(result, Err(Error::Cancelled));
        let sent = fake.sent();
        assert_eq!(sent.len(), 432);
        assert!(is_control(&sent[431], 0x80));
    }

    #[test]
    fn a_cancel_already_set_sends_nothing() {
        let fake = FakeTransport::with_auto_ack();
        let result = red_blue().upload(&mut device(&fake), &AtomicBool::new(true), |_| {});
        assert_eq!(result, Err(Error::Cancelled));
        assert!(fake.events().is_empty());
    }

    #[test]
    fn cancelling_between_start_tries_stops_without_an_abort() {
        // Nothing has been confirmed, so there is nothing to abort.
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Drop);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut device = device(&fake);
        let setter = cancel.clone();
        device.on_packet = Some(Box::new(move |_| setter.store(true, Ordering::Relaxed)));
        let result = red_blue().upload(&mut device, &cancel, |_| {});
        assert_eq!(result, Err(Error::Cancelled));
        assert_eq!(fake.sent().len(), 1);
        assert_eq!(
            fake.sleeps(),
            [100, 25, 200, 500].map(Duration::from_millis)
        );
    }

    #[test]
    fn a_cancelled_upload_can_be_followed_by_another() {
        let fake = FakeTransport::with_auto_ack();
        let mut device = device(&fake);
        let anim = red_blue();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            anim.upload(&mut device, &cancel, |_| {}),
            Err(Error::Cancelled)
        );
        cancel.store(false, Ordering::Relaxed);
        anim.upload(&mut device, &cancel, |_| {}).unwrap();
        assert_eq!(fake.sent().len(), 432);
    }

    // Trouble with the start.

    #[test]
    fn a_refused_start_sends_no_chunks_and_no_finish() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(0, AckFault::Reject);
        let result = upload(&fake, &red_blue());
        assert_eq!(result, Err(Error::Rejected { cmd: 0x0D, idx: 0 }));
        assert_eq!(fake.sent().len(), 1);
        assert_eq!(fake.sleeps(), [100, 25, 500].map(Duration::from_millis));
    }

    #[test]
    fn a_start_with_no_ack_is_sent_4_times_200_ms_apart() {
        let fake = FakeTransport::with_auto_ack();
        (0..4).for_each(|n| fake.ack_fault(n, AckFault::Drop));
        let result = upload(&fake, &red_blue());
        assert_eq!(result, Err(Error::NoAck { cmd: 0x0D, idx: 0 }));
        let sent = fake.sent();
        assert_eq!(sent.len(), 4);
        assert!(sent.iter().all(|r| is_control(r, 0x40)));
        assert_eq!(
            fake.sleeps(),
            [100, 25, 200, 25, 200, 25, 200, 25, 500].map(Duration::from_millis)
        );
    }

    #[test]
    fn a_start_confirmed_on_a_later_try_carries_on() {
        let fake = FakeTransport::with_auto_ack();
        (0..3).for_each(|n| fake.ack_fault(n, AckFault::Drop));
        upload(&fake, &red_blue()).unwrap();
        assert_eq!(fake.sent().len(), 435);
    }

    #[test]
    fn a_disconnected_keyboard_fails_at_once() {
        let fake = FakeTransport::with_auto_ack();
        fake.disconnect();
        let result = upload(&fake, &red_blue());
        assert!(matches!(result, Err(Error::Hid(_))));
        assert_eq!(fake.sleeps(), [100, 25, 500].map(Duration::from_millis));
    }

    // Trouble with the finish.

    #[test]
    fn a_confirmed_finish_says_so() {
        let fake = FakeTransport::with_auto_ack();
        let outcome = upload(&fake, &red_blue()).unwrap();
        assert!(outcome.finish_confirmed);
    }

    #[test]
    fn a_finish_with_no_ack_still_counts_as_done_but_says_so() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(431, AckFault::Drop);
        let outcome = upload(&fake, &red_blue()).unwrap();
        assert!(!outcome.finish_confirmed);
        assert_eq!(fake.sent().len(), 432);
    }

    #[test]
    fn a_refused_finish_still_counts_as_done_but_says_so() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(431, AckFault::Reject);
        let outcome = upload(&fake, &red_blue()).unwrap();
        assert!(!outcome.finish_confirmed);
        assert_eq!(fake.sent().len(), 432);
    }

    #[test]
    fn a_finish_that_cant_be_sent_is_an_error() {
        let fake = FakeTransport::with_auto_ack();
        (431..434).for_each(|n| fake.fail_send(n));
        let result = upload(&fake, &red_blue());
        assert!(matches!(result, Err(Error::Hid(_))));
        assert_eq!(
            fake.sleeps(),
            [100, 25, 120, 50, 25, 50, 50, 500].map(Duration::from_millis)
        );
    }

    // Frame decimation.

    #[test]
    fn frames_are_dropped_evenly_keeping_the_first_and_last() {
        // 60 frames at 20 ms is 1200 ms, which is time for 6 frames of 180 ms.
        // Rounded to the nearest index, as the official app does.
        assert_eq!(decimate(60, 20, 180), [0, 12, 24, 35, 47, 59]);
        // 2 frames fit: only the ends.
        assert_eq!(decimate(7, 60, 180), [0, 6]);
    }

    #[test]
    fn decimating_down_to_one_frame_keeps_the_first() {
        // 9 frames at 20 ms is exactly one draw time.
        assert_eq!(decimate(9, 20, 180), [0]);
    }

    #[test]
    fn frames_stay_when_decimation_doesnt_apply() {
        let all = |count: usize| (0..count).collect::<Vec<_>>();
        // 6 frames or fewer, even with time for fewer than that.
        assert_eq!(decimate(6, 20, 180), all(6));
        assert_eq!(decimate(6, 179, 180), all(6));
        // The delay is no shorter than the draw time.
        assert_eq!(decimate(60, 180, 180), all(60));
        assert_eq!(decimate(60, 500, 180), all(60));
        // Models with no draw time.
        assert_eq!(decimate(60, 20, 0), all(60));
        // Not even one draw time in the whole animation.
        assert_eq!(decimate(7, 1, 180), all(7));
    }

    #[test]
    fn decimated_indexes_only_go_up() {
        for frames in 7..=256 {
            for delay in [1, 2, 7, 13, 25, 60, 90, 179] {
                let kept = decimate(frames, delay, 180);
                assert!(kept.windows(2).all(|pair| pair[0] < pair[1]));
                assert_eq!(kept[0], 0);
                if kept.len() > 1 {
                    assert_eq!(kept[kept.len() - 1], frames - 1);
                }
            }
        }
    }

    #[test]
    fn the_plan_tells_the_final_frame_count_and_delays() {
        let colors = vec![RED; 60];
        let plan = animation(s85(), &colors, 20).plan(s85()).unwrap();
        assert_eq!(plan.frame_count(), 6);
        assert_eq!(plan.chunks_per_frame, 215);
        assert_eq!(plan.total_chunks(), 6 * 215);
        assert_eq!(plan.total_steps(), 6 * 215 + 2);
        // The delay is shorter than the draw time: the start says 0, and the finish the delay.
        assert_eq!(plan.start_delay_ms, 0);
        assert_eq!(plan.finish_delay_ms, 20);
    }

    #[test]
    fn the_start_delay_takes_off_the_draw_time() {
        let plan = animation(s85(), &[RED], 200).plan(s85()).unwrap();
        assert_eq!((plan.start_delay_ms, plan.finish_delay_ms), (20, 200));
        let plan = animation(s85(), &[RED], 180).plan(s85()).unwrap();
        assert_eq!((plan.start_delay_ms, plan.finish_delay_ms), (0, 180));
        let plan = animation(s85(), &[RED], u16::MAX).plan(s85()).unwrap();
        assert_eq!(plan.start_delay_ms, u16::MAX - 180);
        // No draw time on this one.
        let plan = animation(n99(), &[RED], 40).plan(n99()).unwrap();
        assert_eq!((plan.start_delay_ms, plan.finish_delay_ms), (40, 40));
    }

    #[test]
    fn an_upload_sends_the_kept_frames_numbered_from_0() {
        let colors: Vec<[u8; 2]> = (1..=7).map(|i| [i, i]).collect();
        let fake = FakeTransport::with_auto_ack();
        // 7 frames at 60 ms leave 2.
        upload(&fake, &animation(s85(), &colors, 60)).unwrap();

        let sent = fake.sent();
        assert_eq!(sent.len(), 2 + 2 * 215);
        assert_eq!(&sent[0][8..13], &[0x40, 2, 0, 0, 0]);
        assert_eq!((sent[1][3], sent[1][8]), (0, 1));
        assert_eq!((sent[216][3], sent[216][8]), (1, 7));
        assert_eq!(&sent[431][8..13], &[0x00, 2, 0, 60, 0]);
    }

    // Limits.

    #[test]
    fn an_animation_of_the_wrong_size_is_refused_with_nothing_sent() {
        let fake = FakeTransport::with_auto_ack();
        let result = upload(&fake, &animation(m87(), &[RED], 100));
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert!(fake.events().is_empty());
    }

    #[test]
    fn more_frames_than_the_model_takes_are_refused_with_nothing_sent() {
        let fake = FakeTransport::with_auto_ack();
        let colors = vec![RED; 61];
        let result = upload(&fake, &animation(s85(), &colors, 100));
        assert_eq!(result, Err(Error::TooManyFrames { got: 61, max: 60 }));
        assert!(fake.events().is_empty());
    }

    #[test]
    fn the_limit_counts_the_frames_before_decimation() {
        let colors = vec![RED; 61];
        let anim = animation(s85(), &colors, 1);
        assert!(matches!(anim.plan(s85()), Err(Error::TooManyFrames { .. })));
    }

    fn tall_order(width: u16, height: u16, max_frames: u16) -> &'static Model {
        with_screen(ScreenSpec {
            width,
            height,
            max_frames,
            kind: 1,
            crc: false,
            draw_frame_time: 0,
        })
    }

    #[test]
    fn the_frame_byte_limits_an_animation_to_256_frames() {
        let model = tall_order(2, 2, 300);
        let at_limit = vec![RED; 256];
        let plan = animation(model, &at_limit, 100).plan(model).unwrap();
        assert_eq!(plan.frame_count(), 256);

        let over = vec![RED; 257];
        let fake = FakeTransport::with_auto_ack();
        let mut device = Device::new(fake.clone(), None, model).unwrap();
        let result =
            animation(model, &over, 100).upload(&mut device, &AtomicBool::new(false), |_| {});
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert!(fake.events().is_empty());
    }

    #[test]
    fn the_chunk_byte_limits_a_frame_to_256_chunks() {
        let model = tall_order(256, 256, 60);
        let plan = animation(model, &[RED], 100).plan(model).unwrap();
        assert_eq!(plan.chunks_per_frame, 256);

        let model = tall_order(256, 257, 60);
        let fake = FakeTransport::with_auto_ack();
        let mut device = Device::new(fake.clone(), None, model).unwrap();
        let result =
            animation(model, &[RED], 100).upload(&mut device, &AtomicBool::new(false), |_| {});
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert!(fake.events().is_empty());
    }

    // Checksum models.

    fn m87_device(fake: &FakeTransport) -> Device<FakeTransport> {
        Device::new(fake.clone(), None, m87()).unwrap()
    }

    #[test]
    fn checksum_models_carry_the_checksum_and_the_short_last_chunk() {
        let fake = FakeTransport::with_auto_ack();
        let anim = animation(m87(), &[RED], 100);
        anim.upload(&mut m87_device(&fake), &AtomicBool::new(false), |_| {})
            .unwrap();

        let sent = fake.sent();
        assert_eq!(sent.len(), 1 + 127 + 1);
        // 256 times F8 00: the sum is 0xF8 * 256, which is 0 mod 256.
        assert_eq!(&sent[1][..8], &[0x09, 0x0C, 0, 0, 0, 0xFF, 0x00, 0x02]);
        // The last chunk has 144 pixels: 0xF8 * 144 is 0x8B80, so 0x80 mod 256.
        assert_eq!(&sent[127][..8], &[0x09, 0x0C, 0, 0, 126, 0x7F, 0x20, 0x01]);
        assert_eq!(&sent[127][8..10], &RED);
        assert_eq!(sent[127][8 + 287], 0x00);
        assert!(sent[127][8 + 288..].iter().all(|&b| b == 0));
    }

    #[test]
    fn a_wrong_checksum_in_the_ack_sends_the_chunk_again() {
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(1, AckFault::WrongCheck);
        let anim = animation(m87(), &[RED], 100);
        anim.upload(&mut m87_device(&fake), &AtomicBool::new(false), |_| {})
            .unwrap();
        let sent = fake.sent();
        assert_eq!(sent.len(), 1 + 127 + 1 + 1);
        assert_eq!(sent[1], sent[2]);
    }

    // Models with a second collection.

    fn pair() -> (FakeTransport, FakeTransport, Device<FakeTransport>) {
        let main = FakeTransport::with_auto_ack();
        let screen = FakeTransport::with_auto_ack();
        let device = Device::new(main.clone(), Some(screen.clone()), n99()).unwrap();
        (main, screen, device)
    }

    /// Records each report's collection and command, across both collections.
    fn record(device: &mut Device<FakeTransport>) -> Arc<Mutex<Vec<(Collection, u8, u8)>>> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let writer = log.clone();
        device.on_packet = Some(Box::new(move |event| {
            if let PacketEvent::Sent { collection, report } = event {
                writer
                    .lock()
                    .unwrap()
                    .push((collection, report[1], report[8]));
            }
        }));
        log
    }

    #[test]
    fn chunks_go_to_the_screen_collection_and_controls_to_both_in_order() {
        let (main, screen, mut device) = pair();
        let log = record(&mut device);
        animation(n99(), &[RED, BLUE], 40)
            .upload(&mut device, &AtomicBool::new(false), |_| {})
            .unwrap();

        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2 + 2 * 225 + 2);
        assert_eq!(log[0], (Collection::Main, 0x0D, 0x40));
        assert_eq!(log[1], (Collection::Screen, 0x0D, 0x40));
        assert!(
            log[2..452].iter().all(
                |&(collection, command, _)| collection == Collection::Screen && command == 0x0C
            )
        );
        assert_eq!(log[452], (Collection::Main, 0x0D, 0x00));
        assert_eq!(log[453], (Collection::Screen, 0x0D, 0x00));

        assert_eq!(main.sent().len(), 2);
        assert_eq!(screen.sent().len(), 2 + 450);
        assert_eq!(main.sent()[0], screen.sent()[0]);
        assert_eq!(main.sent()[1], screen.sent()[451]);
    }

    #[test]
    fn a_finish_needs_both_collections_to_count_as_confirmed() {
        let (_main, screen, mut device) = pair();
        // The start's ack, one per chunk, and then the finish's.
        screen.ack_fault(226, AckFault::Drop);
        let outcome = animation(n99(), &[RED], 40)
            .upload(&mut device, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(!outcome.finish_confirmed);
    }

    #[test]
    fn two_collections_pause_before_each_copy_of_a_control_report() {
        let (main, screen, mut device) = pair();
        animation(n99(), &[RED], 40)
            .upload(&mut device, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(
            main.sleeps(),
            [100, 25, 120, 50, 25, 500].map(Duration::from_millis)
        );
        assert_eq!(screen.sleeps(), [25, 25].map(Duration::from_millis));
    }

    #[test]
    fn it_works_with_acks_arriving_on_the_other_collection() {
        let (main, screen, mut device) = pair();
        main.route_acks_to(&screen);
        screen.route_acks_to(&main);
        animation(n99(), &[RED], 40)
            .upload(&mut device, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(main.sent().len(), 2);
        assert_eq!(screen.sent().len(), 2 + 225);
    }

    #[test]
    fn a_start_refused_by_the_screen_collection_sends_no_chunks() {
        let (main, screen, mut device) = pair();
        screen.ack_fault(0, AckFault::Reject);
        let result =
            animation(n99(), &[RED], 40).upload(&mut device, &AtomicBool::new(false), |_| {});
        assert_eq!(result, Err(Error::Rejected { cmd: 0x0D, idx: 0 }));
        assert_eq!(main.sent().len(), 1);
        assert_eq!(screen.sent().len(), 1);
    }

    #[test]
    fn an_abort_goes_to_both_collections_in_order() {
        let (main, screen, mut device) = pair();
        let log = record(&mut device);
        // The start is ack 0 on the screen collection, and this is the first chunk.
        screen.ack_fault(1, AckFault::Drop);
        let result =
            animation(n99(), &[RED], 40).upload(&mut device, &AtomicBool::new(false), |_| {});
        assert_eq!(result, Err(Error::NoAck { cmd: 0x0C, idx: 0 }));

        let log = log.lock().unwrap();
        assert_eq!(log[log.len() - 2], (Collection::Main, 0x0D, 0x80));
        assert_eq!(log[log.len() - 1], (Collection::Screen, 0x0D, 0x80));
        assert_eq!(main.sent().len(), 2);
        assert_eq!(screen.sent().len(), 3);
    }

    // Every supported model takes the same flow.

    #[test]
    fn every_supported_model_uploads_a_frame() {
        for model in model::supported() {
            let main = FakeTransport::with_auto_ack();
            let screen = model
                .has_second_collection()
                .then(FakeTransport::with_auto_ack);
            let mut device = Device::new(main.clone(), screen.clone(), model).unwrap();
            let spec = model.screen.unwrap();
            let anim = animation(model, &[RED], 100);
            anim.upload(&mut device, &AtomicBool::new(false), |_| {})
                .unwrap();
            let chunks = chunks_per_frame(spec.width, spec.height);
            let all = main.sent().len() + screen.as_ref().map_or(0, |s| s.sent().len());
            let controls = if screen.is_some() { 4 } else { 2 };
            assert_eq!(all, chunks + controls, "{:04X}", model.pid);
        }
    }
}
