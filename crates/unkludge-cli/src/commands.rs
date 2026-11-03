//! The commands, on any transport. `info`, `report` and `light` with no options only read.

use std::error::Error as StdError;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use chrono::{Datelike, Timelike};
use unkludge_protocol::clock::{DateTime, set_clock};
use unkludge_protocol::config::Config;
use unkludge_protocol::lighting::{self, CurrentLighting, LightingChange, Palettes};
use unkludge_protocol::model;
use unkludge_protocol::screen::Animation;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};

use crate::args::{Command, LightArgs, ScreenArgs};
use crate::output::{self, Console};

pub type Failure = Box<dyn StdError>;

/// Runs a command on an open device. `dry_run` only changes the wording.
pub fn run<T: Transport>(
    device: &mut Device<T>,
    command: &Command,
    dry_run: bool,
    console: &mut Console,
) -> Result<(), Failure> {
    match command {
        Command::Info => info(device, console),
        Command::Models => {
            console.lines(output::models_table(model::supported()));
            Ok(())
        }
        Command::Light(args) => light(device, args, console),
        Command::Clock => clock(device, dry_run, console),
        Command::Screen(args) => screen(device, args, dry_run, console),
        Command::Report => {
            console.lines(report_header());
            report_keyboard(device, console);
            Ok(())
        }
    }
}

fn info<T: Transport>(device: &mut Device<T>, console: &mut Console) -> Result<(), Failure> {
    let model = device.model();
    let config = Config::read(device)?;
    console.lines(output::model_lines(model));
    console.line("Current settings:");
    let current = CurrentLighting::from_config(&config, &model.lighting);
    console.lines(output::lighting_lines(&current, &model.lighting));
    console.line("Config block:");
    console.lines(output::config_lines(config.as_bytes()));
    Ok(())
}

/// With no options this stays a read: it never reaches `apply`.
fn light<T: Transport>(
    device: &mut Device<T>,
    args: &LightArgs,
    console: &mut Console,
) -> Result<(), Failure> {
    let change = args.change();
    let current = if change == LightingChange::default() {
        lighting::read(device)?
    } else {
        if args.rgb.is_some() {
            // The keyboard's palettes can't be read and this tool keeps no copy between runs, so
            // this is true of every custom colour it sends.
            console.warn(
                "a custom colour writes the palettes of all effects at once, and the keyboard's \
                 can't be read, so the other effects' colours go back to the defaults",
            );
        }
        lighting::apply(device, &change, &mut Palettes::default())?
    };
    console.lines(output::lighting_lines(&current, &device.model().lighting));
    Ok(())
}

fn clock<T: Transport>(
    device: &mut Device<T>,
    dry_run: bool,
    console: &mut Console,
) -> Result<(), Failure> {
    let now = chrono::Local::now();
    set_clock(device, &device_time(&now)?)?;
    console.line(format_args!(
        "Clock: {}",
        now.format("%Y-%m-%d %H:%M:%S, %A")
    ));
    if !dry_run {
        console.note("The keyboard doesn't acknowledge this: check its screen.");
    }
    Ok(())
}

/// A local time, as the keyboard's clock takes it.
fn device_time<T: Datelike + Timelike>(time: &T) -> Result<DateTime, Error> {
    let year = u16::try_from(time.year())
        .map_err(|_| Error::Invalid(format!("the year {} doesn't fit", time.year())))?;
    // The rest are small enough for their fields, and `set_clock` checks them all.
    Ok(DateTime {
        year,
        month: time.month() as u8,
        day: time.day() as u8,
        hour: time.hour() as u8,
        minute: time.minute() as u8,
        second: time.second() as u8,
        weekday: time.weekday().num_days_from_sunday() as u8,
    })
}

fn screen<T: Transport>(
    device: &mut Device<T>,
    args: &ScreenArgs,
    dry_run: bool,
    console: &mut Console,
) -> Result<(), Failure> {
    let model = device.model();
    let spec = model.screen.ok_or(Error::Unsupported(model.pid))?;
    let animation = Animation::from_file(&args.file, &spec, args.load_options())
        .map_err(|error| load_error(&args.file, error))?;
    let plan = animation.plan(model)?;
    let frames = animation.frames().len();

    if animation.truncated() {
        console.warn(format_args!(
            "the file has more frames than this keyboard's limit: only the first {frames} are used"
        ));
    }
    if plan.frame_count() < frames {
        console.warn(format_args!(
            "at {} ms per frame, {} of its {frames} frames are sent, as this keyboard's draw time \
             is {} ms",
            animation.delay_ms(),
            plan.frame_count(),
            spec.draw_frame_time
        ));
    }
    console.line(format_args!(
        "Frames: {} at {}x{}, {} ms each",
        plan.frame_count(),
        animation.width(),
        animation.height(),
        animation.delay_ms()
    ));
    console.line(format_args!("Chunks: {}", plan.total_chunks()));

    // A dry run has no wait to show progress for, and a file or a pipe doesn't want it.
    let show_progress = !dry_run && std::io::stderr().is_terminal();
    let no_cancel = AtomicBool::new(false);
    let uploaded = animation.upload(device, &no_cancel, |progress| {
        if show_progress {
            console.progress(progress.done, progress.total);
        }
    });
    if show_progress {
        console.note("");
    }

    if !uploaded?.finish_confirmed {
        console.warn(
            "the keyboard took every frame but didn't acknowledge the finish, so it may not show \
             the new animation",
        );
    }
    Ok(())
}

fn load_error(file: &Path, error: Error) -> Failure {
    match error {
        Error::TooManyFrames { max, .. } => {
            format!("{error}. --truncate keeps the first {max}").into()
        }
        other => format!("{}: {other}", file.display()).into(),
    }
}

/// The first lines of a report, which need no keyboard.
pub fn report_header() -> Vec<String> {
    vec![
        format!("Unkludge: {}", env!("CARGO_PKG_VERSION")),
        format!(
            "System: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    ]
}

/// The part of a report that comes from the keyboard. A read that fails is what a report is for, so
/// it goes in the report rather than ending it.
fn report_keyboard<T: Transport>(device: &mut Device<T>, console: &mut Console) {
    let model = device.model();
    console.line(format_args!("Model: {}", model.name));
    console.line(format_args!("PID: {:04X}", model.pid));
    console.line(format_args!(
        "Status: {}",
        output::status_label(model.status)
    ));
    match Config::read(device) {
        Ok(config) => {
            console.line("Config block:");
            console.lines(output::config_lines(config.as_bytes()));
        }
        Err(error) => console.line(format_args!("Config block: couldn't read it: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Cli, FitArg};
    use crate::dry_run::{self, DryRunTransport};
    use chrono::NaiveDate;
    use clap::Parser;
    use unkludge_protocol::REPORT_LEN;
    use unkludge_protocol::device::PacketEvent;
    use unkludge_protocol::lighting::Rgb;
    use unkludge_protocol::model;
    use unkludge_protocol::transport::fake::{AckFault, FakeTransport};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// What a command did: the reports that went out, and what it printed.
    #[derive(Default)]
    struct Outcome {
        sent: Vec<Vec<u8>>,
        out: String,
        err: String,
    }

    fn on_device<T: Transport>(
        device: &mut Device<T>,
        command: &Command,
    ) -> (Result<(), Failure>, Outcome) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let hook = Arc::clone(&sent);
        device.on_packet = Some(Box::new(move |event| {
            if let PacketEvent::Sent { report, .. } = event {
                hook.lock().unwrap().push(report.to_vec());
            }
        }));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut console = Console {
            out: &mut out,
            err: &mut err,
        };
        let result = run(device, command, true, &mut console);
        let outcome = Outcome {
            sent: sent.lock().unwrap().clone(),
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
        };
        (result, outcome)
    }

    fn dry_device(pid: u16) -> Device<DryRunTransport> {
        dry_run::device(model::find(pid).unwrap()).unwrap()
    }

    fn command(args: &[&str]) -> Command {
        Cli::try_parse_from(std::iter::once("unkludge").chain(args.iter().copied()))
            .unwrap()
            .command
    }

    fn dry(args: &[&str]) -> (Result<(), Failure>, Outcome) {
        on_device(&mut dry_device(0x0265), &command(args))
    }

    #[track_caller]
    fn assert_only_config_reads(sent: &[Vec<u8>]) {
        assert!(!sent.is_empty());
        for report in sent {
            assert_eq!(report.len(), REPORT_LEN);
            assert_eq!(&report[..8], &[0x09, 0x84, 0, 0, 1, 0, 0x80, 0]);
            assert!(report[8..].iter().all(|&b| b == 0));
        }
    }

    /// A GIF of solid frames, in a file of its own.
    fn temp_gif(name: &str, frames: usize) -> PathBuf {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};

        let path = std::env::temp_dir().join(format!(
            "unkludge-cli-test-{}-{name}.gif",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = GifEncoder::new(file);
        for value in 0..frames {
            let image = RgbaImage::from_pixel(8, 8, Rgba([value as u8 * 4, 0, 0, 255]));
            let delay = Delay::from_numer_denom_ms(200, 1);
            encoder
                .encode_frame(Frame::from_parts(image, 0, 0, delay))
                .unwrap();
        }
        path
    }

    #[test]
    fn info_only_reads_the_config() {
        let (result, outcome) = dry(&["info"]);
        result.unwrap();
        assert_only_config_reads(&outcome.sent);
        assert_eq!(outcome.sent.len(), 1);
        assert!(outcome.out.contains("Model: RK S85 ES"));
        assert!(outcome.out.contains("Mode: off"));
        assert!(outcome.out.contains("  70: "));
    }

    #[test]
    fn light_with_no_options_only_reads_the_config() {
        let (result, outcome) = dry(&["light"]);
        result.unwrap();
        assert_only_config_reads(&outcome.sent);
        assert_eq!(outcome.sent.len(), 1);
        assert!(outcome.out.contains("Mode: off"));
        assert!(outcome.out.contains("Sleep: unknown (0x00)"));
    }

    #[test]
    fn report_only_reads_the_config() {
        let (result, outcome) = dry(&["report"]);
        result.unwrap();
        assert_only_config_reads(&outcome.sent);
        assert!(outcome.out.contains("Model: RK S85 ES"));
        assert!(outcome.out.contains("PID: 0265"));
        assert!(outcome.out.contains("5A A5"));
    }

    #[test]
    fn the_report_header_has_the_version_and_system() {
        let header = report_header();
        assert!(header[0].contains(env!("CARGO_PKG_VERSION")), "{header:?}");
        assert!(header[1].contains(std::env::consts::OS), "{header:?}");
    }

    #[test]
    fn light_with_a_mode_writes_the_config_after_reading_it() {
        let (result, outcome) = dry(&["light", "--mode", "2"]);
        result.unwrap();
        assert_eq!(outcome.sent.len(), 2);
        assert_only_config_reads(&outcome.sent[..1]);
        let write = &outcome.sent[1];
        assert_eq!(&write[..8], &[0x09, 0x04, 0, 0, 1, 0, 0x80, 0]);
        assert_eq!(write[8 + 0x0A], 2);
        assert_eq!(&write[8 + 0x7E..8 + 0x80], &[0x5A, 0xA5]);
        assert!(outcome.out.contains("Mode: 2 (Breathing)"));
        assert!(outcome.err.is_empty(), "{}", outcome.err);
    }

    #[test]
    fn an_invalid_change_writes_nothing() {
        let (result, outcome) = dry(&["light", "--mode", "2", "--brightness", "5"]);
        assert!(result.unwrap_err().to_string().contains("brightness 5"));
        assert_only_config_reads(&outcome.sent);
    }

    #[test]
    fn changing_a_setting_with_the_lights_off_says_so() {
        let (result, outcome) = dry(&["light", "--brightness", "8"]);
        assert!(result.unwrap_err().to_string().contains("lights are off"));
        assert_only_config_reads(&outcome.sent);
    }

    #[test]
    fn rgb_warns_and_writes_the_palettes() {
        let (result, outcome) = dry(&["light", "--mode", "2", "--rgb", "FF1080"]);
        result.unwrap();
        assert!(outcome.err.contains("warning: "), "{}", outcome.err);
        assert!(outcome.err.contains("defaults"), "{}", outcome.err);
        let palettes = outcome.sent.last().unwrap();
        assert_eq!(palettes[1], 0x0A);
        let entry = 8 + 2 * 21;
        let color = Rgb::new(palettes[entry], palettes[entry + 1], palettes[entry + 2]);
        assert_eq!(color, Rgb::new(0xFF, 0x10, 0x80));
    }

    #[test]
    fn other_changes_dont_warn_about_palettes() {
        let (result, outcome) = dry(&["light", "--mode", "2", "--color", "3"]);
        result.unwrap();
        assert!(outcome.sent.iter().all(|report| report[1] != 0x0A));
        assert!(!outcome.err.contains("warning"), "{}", outcome.err);
    }

    #[test]
    fn sleep_times_write_their_bytes() {
        for (arg, byte) in [("5", 0x0A), ("10", 0x14), ("20", 0x28), ("never", 0x3C)] {
            let (result, outcome) = dry(&["light", "--sleep", arg]);
            result.unwrap();
            assert_eq!(outcome.sent[1][8 + 0x18], byte, "{arg}");
        }
    }

    #[test]
    fn the_clock_sends_one_clock_report() {
        let (result, outcome) = dry(&["clock"]);
        result.unwrap();
        assert_eq!(outcome.sent.len(), 1);
        assert_eq!(&outcome.sent[0][..8], &[0x09, 0x0B, 0, 0, 1, 0, 14, 0]);
        assert!(outcome.out.contains("Clock: "), "{}", outcome.out);
    }

    #[test]
    fn device_time_takes_the_fields_of_a_local_time() {
        // 2026-10-29 was a Thursday.
        let time = NaiveDate::from_ymd_opt(2026, 10, 29)
            .unwrap()
            .and_hms_opt(1, 24, 7)
            .unwrap();
        assert_eq!(
            device_time(&time).unwrap(),
            DateTime {
                year: 2026,
                month: 10,
                day: 29,
                hour: 1,
                minute: 24,
                second: 7,
                weekday: 4,
            }
        );
        let sunday = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(device_time(&sunday).unwrap().weekday, 0);
    }

    #[test]
    fn a_year_that_doesnt_fit_is_an_error() {
        let time = NaiveDate::from_ymd_opt(70000, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert!(device_time(&time).is_err());
    }

    #[test]
    fn screen_uploads_every_chunk_of_the_file() {
        let path = temp_gif("upload", 2);
        let screen = command(&["screen", path.to_str().unwrap(), "--fit", "stretch"]);
        let (result, outcome) = on_device(&mut dry_device(0x0265), &screen);
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        // The start, 215 chunks per frame, and the finish.
        assert_eq!(outcome.sent.len(), 432);
        assert_eq!(outcome.sent[0][1], 0x0D);
        assert_eq!(outcome.sent[431][1], 0x0D);
        assert!(outcome.sent[1..431].iter().all(|report| report[1] == 0x0C));
        assert!(
            outcome.out.contains("Frames: 2 at 320x172, 200 ms each"),
            "{}",
            outcome.out
        );
        assert!(outcome.out.contains("Chunks: 430"), "{}", outcome.out);
    }

    #[test]
    fn too_many_frames_is_an_error_that_tells_about_truncating() {
        let path = temp_gif("too-many", 61);
        let screen = command(&["screen", path.to_str().unwrap()]);
        let (result, outcome) = on_device(&mut dry_device(0x0265), &screen);
        std::fs::remove_file(&path).unwrap();
        let text = result.unwrap_err().to_string();
        assert!(text.contains("limit is 60"), "{text}");
        assert!(text.contains("--truncate"), "{text}");
        assert!(outcome.sent.is_empty());
    }

    #[test]
    fn truncating_warns_and_sends_what_fits() {
        let path = temp_gif("truncate", 61);
        let screen = command(&["screen", path.to_str().unwrap(), "--truncate"]);
        let (result, outcome) = on_device(&mut dry_device(0x0265), &screen);
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        assert!(outcome.err.contains("warning: "), "{}", outcome.err);
        assert!(outcome.err.contains("first 60"), "{}", outcome.err);
        assert_eq!(outcome.sent.len(), 60 * 215 + 2);
    }

    #[test]
    fn other_load_errors_name_the_file() {
        let screen = command(&["screen", "no-such-file.gif"]);
        let (result, outcome) = on_device(&mut dry_device(0x0265), &screen);
        let text = result.unwrap_err().to_string();
        assert!(text.contains("no-such-file.gif"), "{text}");
        assert!(outcome.sent.is_empty());
    }

    #[test]
    fn a_finish_the_keyboard_doesnt_confirm_is_a_warning() {
        let path = temp_gif("finish", 1);
        let fake = FakeTransport::with_auto_ack();
        // The start is ack 0, then 215 chunks. So the finish is ack 216.
        fake.ack_fault(216, AckFault::Drop);
        let mut device = Device::new(fake, None, model::find(0x0265).unwrap()).unwrap();
        let screen = command(&["screen", path.to_str().unwrap()]);
        let (result, outcome) = on_device(&mut device, &screen);
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        assert!(outcome.err.contains("warning: "), "{}", outcome.err);
        assert!(outcome.err.contains("finish"), "{}", outcome.err);
    }

    #[test]
    fn a_rejected_upload_fails_after_the_abort() {
        let path = temp_gif("abort", 1);
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(5, AckFault::Drop);
        let mut device = Device::new(fake, None, model::find(0x0265).unwrap()).unwrap();
        let screen = command(&["screen", path.to_str().unwrap()]);
        let (result, outcome) = on_device(&mut device, &screen);
        std::fs::remove_file(&path).unwrap();
        assert!(result.is_err());
        assert_eq!(outcome.sent.last().unwrap()[8], 0x80, "the abort is last");
    }

    #[test]
    fn fast_animations_say_how_many_frames_go_out() {
        // On this model frames shorter than 180 ms get thinned out, once there are more than six.
        let path = temp_gif("thinned", 8);
        let screen = command(&["screen", path.to_str().unwrap(), "--delay", "90"]);
        let (result, outcome) = on_device(&mut dry_device(0x0265), &screen);
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        assert!(outcome.err.contains("4 of its 8 frames"), "{}", outcome.err);
    }

    #[test]
    fn models_print_their_table_with_no_report_sent() {
        let (result, outcome) = dry(&["models"]);
        result.unwrap();
        assert!(outcome.sent.is_empty());
        assert!(outcome.out.contains("0265 RK S85 ES"));
    }

    #[test]
    fn fit_defaults_to_cover() {
        let Command::Screen(screen) = command(&["screen", "a.gif"]) else {
            panic!("not screen");
        };
        assert_eq!(screen.fit, FitArg::Cover);
    }
}
