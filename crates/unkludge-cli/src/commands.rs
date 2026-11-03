//! The commands, on any transport. `info`, `report` and `light` with no options only read.

use std::error::Error as StdError;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use unkludge_protocol::clock::set_clock;
use unkludge_protocol::config::Config;
use unkludge_protocol::device::HidCollection;
use unkludge_protocol::lighting::{self, CurrentLighting, LightingChange, Palettes};
use unkludge_protocol::model::{self, Model};
use unkludge_protocol::screen::Animation;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};

use crate::args::{KeyboardCommand, LightArgs, ScreenArgs};
use crate::local_time::device_time;
use crate::output::{self, Console};

pub type Failure = Box<dyn StdError>;

/// How a command runs, apart from the device it runs on.
pub struct Options<'a> {
    /// Changes the wording, and leaves out what only makes sense on a real keyboard.
    pub dry_run: bool,
    /// Whether stderr is a terminal, which decides how progress shows.
    pub terminal: bool,
    /// Setting this stops an upload.
    pub cancel: &'a AtomicBool,
    /// What a report says about the computer.
    pub host: &'a [String],
}

/// Runs a command on an open device.
pub fn run<T: Transport>(
    device: &mut Device<T>,
    command: &KeyboardCommand,
    options: &Options,
    console: &mut Console,
) -> Result<(), Failure> {
    match command {
        KeyboardCommand::Info => info(device, console),
        KeyboardCommand::Light(args) => light(device, args, options, console),
        KeyboardCommand::Clock => clock(device, options, console),
        KeyboardCommand::Screen(args) => screen(device, args, options, console),
        KeyboardCommand::Report => {
            console.lines(report_header());
            console.lines(options.host);
            report_keyboard(device, console);
            Ok(())
        }
    }
}

/// What to know before a dry run's output.
pub fn dry_run_notes(model: &Model, command: &KeyboardCommand, timing: bool) -> Vec<String> {
    let mut notes = vec![format!(
        "Dry run for {} ({:04X}): no keyboard is opened and nothing is sent.",
        model.name, model.pid
    )];
    if let KeyboardCommand::Info | KeyboardCommand::Light(_) | KeyboardCommand::Report = command {
        notes.push(
            "The config read gets a blank block, which reads as the lights being off.".to_string(),
        );
    }
    if timing {
        notes.push("A dry run makes up its acks, so timing says nothing about a keyboard.".into());
    }
    notes
}

pub fn warn_if_rk_app(running: bool, console: &mut Console) {
    if running {
        console.warn(
            "DeviceDriver.exe, which may be RK's app, is running. It polls the keyboard and sends \
             its own settings back, so close it first",
        );
    }
}

/// What to say when a keyboard won't open, given what the system lists.
pub fn open_failure(
    error: &Error,
    pid: Option<u16>,
    listed: &[HidCollection],
    linux: bool,
) -> String {
    match error {
        Error::NotFound => {
            let present = listed
                .iter()
                .filter(|collection| pid.is_none_or(|pid| pid == collection.pid))
                .find_map(|collection| {
                    model::find(collection.pid).filter(|model| model.is_supported())
                });
            match (present, pid) {
                (Some(model), _) => format!(
                    "{} ({:04X}) is connected, but not with the HID collections Unkludge can use. \
                     `unkludge report` lists them",
                    model.name, model.pid
                ),
                (None, Some(pid)) => {
                    format!("keyboard {pid:04X} not found. Is it connected by USB cable?")
                }
                (None, None) => format!(
                    "{error}. Is it connected by USB cable? `unkludge models` lists the supported \
                     ones"
                ),
            }
        }
        Error::Hid(_) if linux => format!(
            "{error}. On Linux this usually means no permission to open the keyboard: \
             packaging/linux/70-unkludge.rules grants it"
        ),
        other => other.to_string(),
    }
}

/// The model data comes first, so that it shows even if the read fails.
fn info<T: Transport>(device: &mut Device<T>, console: &mut Console) -> Result<(), Failure> {
    let model = device.model();
    console.lines(output::model_lines(model));
    let config = Config::read(device)?;
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
    options: &Options,
    console: &mut Console,
) -> Result<(), Failure> {
    let model = device.model();
    let change = args.change();
    if change == LightingChange::default() {
        let current = lighting::read(device)?;
        console.lines(output::lighting_lines(&current, &model.lighting));
        return Ok(());
    }

    let current = lighting::apply(device, &change, &mut Palettes::default())?;
    console.lines(output::lighting_lines(&current, &model.lighting));
    if args.rgb.is_some() {
        // The keyboard's palettes can't be read and this tool keeps no copy between runs, so this
        // is true of every custom colour it sends.
        console.warn(
            "a custom colour gives every other effect Unkludge's default colours. Colours set in \
             RK's app are replaced, as they can't be read first",
        );
    }
    if !options.dry_run {
        console.note("These are the values sent. To check them, run `unkludge light` again.");
    }
    Ok(())
}

fn clock<T: Transport>(
    device: &mut Device<T>,
    options: &Options,
    console: &mut Console,
) -> Result<(), Failure> {
    let now = chrono::Local::now();
    set_clock(device, &device_time(&now)?)?;
    console.line(format_args!(
        "Clock: {}",
        now.format("%Y-%m-%d %H:%M:%S, %A")
    ));
    if !options.dry_run {
        console.note("The keyboard doesn't acknowledge this: check its screen.");
    }
    Ok(())
}

fn screen<T: Transport>(
    device: &mut Device<T>,
    args: &ScreenArgs,
    options: &Options,
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

    // A dry run has no wait to show progress for. A terminal gets one line that keeps changing, and
    // a file or a pipe a line per frame.
    let uploaded = animation.upload(device, options.cancel, |progress| {
        if options.dry_run {
            return;
        }
        if options.terminal {
            console.progress(progress.done, progress.total);
        } else if let Some(frame) = finished_frame(progress.done, plan.chunks_per_frame)
            && frame <= plan.frame_count()
        {
            console.note(format_args!("Frame {frame} of {} sent", plan.frame_count()));
        }
    });
    if options.terminal && !options.dry_run {
        console.note("");
    }

    if !uploaded?.finish_confirmed {
        console.warn(
            "the keyboard took every frame but didn't acknowledge the finish, so it may not show \
             the new animation",
        );
    }
    console.line("Done");
    Ok(())
}

/// Which frame the upload's step `done` finished, counting from 1, if any. The first step is the
/// start report.
fn finished_frame(done: usize, chunks_per_frame: usize) -> Option<usize> {
    let chunks = done.checked_sub(1)?;
    (chunks > 0 && chunks.is_multiple_of(chunks_per_frame)).then(|| chunks / chunks_per_frame)
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
    use crate::args::{Cli, Command, FitArg};
    use crate::dry_run::{self, DryRunTransport};
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

    static IDLE: AtomicBool = AtomicBool::new(false);

    /// As for a dry run.
    fn dry_options() -> Options<'static> {
        Options {
            dry_run: true,
            terminal: false,
            cancel: &IDLE,
            host: &[],
        }
    }

    /// As for a real keyboard.
    fn real_options(terminal: bool) -> Options<'static> {
        Options {
            dry_run: false,
            terminal,
            cancel: &IDLE,
            host: &[],
        }
    }

    fn on_device<T: Transport>(
        device: &mut Device<T>,
        command: &KeyboardCommand,
    ) -> (Result<(), Failure>, Outcome) {
        with_options(device, command, &dry_options())
    }

    fn with_options<T: Transport>(
        device: &mut Device<T>,
        command: &KeyboardCommand,
        options: &Options,
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
        let result = run(device, command, options, &mut console);
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

    fn command(args: &[&str]) -> KeyboardCommand {
        let cli = Cli::try_parse_from(std::iter::once("unkludge").chain(args.iter().copied()));
        match cli.unwrap().command {
            Command::Keyboard(command) => command,
            Command::Models => panic!("not a keyboard command"),
        }
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
    fn a_report_starts_with_the_version_system_and_host() {
        let host = ["RK app running: no".to_string()];
        let options = Options {
            host: &host,
            ..dry_options()
        };
        let (result, outcome) =
            with_options(&mut dry_device(0x0265), &command(&["report"]), &options);
        result.unwrap();
        let lines: Vec<&str> = outcome.out.lines().collect();
        assert_eq!(lines[0], format!("Unkludge: {}", env!("CARGO_PKG_VERSION")));
        assert_eq!(
            lines[1],
            format!(
                "System: {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
        assert_eq!(lines[2], "RK app running: no");
        assert_eq!(lines[3], "Model: RK S85 ES");
    }

    #[test]
    fn a_report_shows_a_read_that_failed_and_still_succeeds() {
        // Nothing queued, so every read fails.
        let mut device =
            Device::new(FakeTransport::new(), None, model::find(0x0265).unwrap()).unwrap();
        let (result, outcome) = on_device(&mut device, &command(&["report"]));
        result.unwrap();
        assert!(
            outcome.out.contains("Config block: couldn't read it: "),
            "{}",
            outcome.out
        );
    }

    #[test]
    fn info_shows_the_model_even_if_the_read_fails() {
        let mut device =
            Device::new(FakeTransport::new(), None, model::find(0x0265).unwrap()).unwrap();
        let (result, outcome) = on_device(&mut device, &command(&["info"]));
        assert!(result.is_err());
        assert!(outcome.out.contains("Model: RK S85 ES"), "{}", outcome.out);
        assert!(!outcome.out.contains("Current settings"), "{}", outcome.out);
    }

    #[test]
    fn dry_run_notes_say_what_applies() {
        let s85 = model::find(0x0265).unwrap();
        let clock = dry_run_notes(s85, &command(&["clock"]), false);
        assert_eq!(
            clock,
            ["Dry run for RK S85 ES (0265): no keyboard is opened and nothing is sent."]
        );
        let info = dry_run_notes(s85, &command(&["info"]), false);
        assert!(info[1].contains("blank block"), "{info:?}");
        let timed = dry_run_notes(s85, &command(&["clock"]), true);
        assert!(timed[1].contains("makes up its acks"), "{timed:?}");
    }

    #[test]
    fn the_rk_app_is_warned_about_only_if_running() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut console = Console {
            out: &mut out,
            err: &mut err,
        };
        warn_if_rk_app(false, &mut console);
        assert!(err.is_empty());

        let mut console = Console {
            out: &mut out,
            err: &mut err,
        };
        warn_if_rk_app(true, &mut console);
        let text = String::from_utf8(err).unwrap();
        assert!(text.starts_with("warning: DeviceDriver.exe"), "{text}");
        assert!(out.is_empty());
    }

    fn listed(pid: u16, usage_page: u16) -> HidCollection {
        HidCollection {
            pid,
            usage_page,
            usage: 1,
            interface: 0,
        }
    }

    #[test]
    fn nothing_connected_asks_about_the_cable() {
        let text = open_failure(&Error::NotFound, None, &[], false);
        assert!(text.starts_with("no supported keyboard found."), "{text}");
        assert!(text.contains("USB cable"), "{text}");

        // A device of the vendor's that isn't supported doesn't count, and neither does one with
        // another product ID than the one asked for.
        let other = [listed(0x002A, 0xFF00), listed(0x0265, 0xFF00)];
        let text = open_failure(&Error::NotFound, Some(0x01A2), &other, false);
        assert_eq!(
            text,
            "keyboard 01A2 not found. Is it connected by USB cable?"
        );
        let text = open_failure(&Error::NotFound, None, &other[..1], false);
        assert!(text.contains("USB cable"), "{text}");
    }

    #[test]
    fn a_supported_keyboard_with_other_collections_says_so() {
        let found = [listed(0x002A, 0xFF00), listed(0x0265, 0xFF00)];
        for pid in [None, Some(0x0265)] {
            let text = open_failure(&Error::NotFound, pid, &found, false);
            assert!(text.starts_with("RK S85 ES (0265) is connected"), "{text}");
            assert!(text.contains("unkludge report"), "{text}");
            assert!(!text.contains("USB cable"), "{text}");
        }
    }

    #[test]
    fn a_failure_to_open_hints_at_permissions_on_linux_only() {
        let error = Error::Hid("failed to open".to_string());
        let linux = open_failure(&error, None, &[], true);
        assert!(linux.starts_with("USB error: failed to open."), "{linux}");
        assert!(linux.contains("70-unkludge.rules"), "{linux}");
        assert_eq!(
            open_failure(&error, None, &[], false),
            "USB error: failed to open"
        );
        assert_eq!(
            open_failure(&Error::Unsupported(0x002A), None, &[], true),
            "keyboard 002A isn't supported"
        );
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
        assert!(
            outcome.err.contains("Unkludge's default colours"),
            "{}",
            outcome.err
        );
        assert!(outcome.err.contains("RK's app"), "{}", outcome.err);
        let palettes = outcome.sent.last().unwrap();
        assert_eq!(palettes[1], 0x0A);
        let entry = 8 + 2 * 21;
        let color = Rgb::new(palettes[entry], palettes[entry + 1], palettes[entry + 2]);
        assert_eq!(color, Rgb::new(0xFF, 0x10, 0x80));
    }

    #[test]
    fn a_refused_custom_colour_warns_about_nothing() {
        // Blossom has no colour option.
        let (result, outcome) = dry(&["light", "--mode", "17", "--rgb", "FF1080"]);
        assert!(result.is_err());
        assert!(outcome.err.is_empty(), "{}", outcome.err);
        assert_only_config_reads(&outcome.sent);
    }

    #[test]
    fn after_a_write_it_says_how_to_check() {
        let args = command(&["light", "--mode", "2"]);
        let (result, outcome) = with_options(&mut dry_device(0x0265), &args, &real_options(false));
        result.unwrap();
        assert!(outcome.err.contains("values sent"), "{}", outcome.err);
        assert!(
            outcome.err.contains("`unkludge light` again"),
            "{}",
            outcome.err
        );

        let args = command(&["light"]);
        let (result, outcome) = with_options(&mut dry_device(0x0265), &args, &real_options(false));
        result.unwrap();
        assert!(
            outcome.err.is_empty(),
            "a read says nothing: {}",
            outcome.err
        );
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
        assert!(outcome.err.is_empty(), "{}", outcome.err);
    }

    #[test]
    fn on_a_keyboard_the_clock_says_to_look_at_the_screen() {
        let args = command(&["clock"]);
        let (result, outcome) = with_options(&mut dry_device(0x0265), &args, &real_options(false));
        result.unwrap();
        assert!(outcome.err.contains("check its screen"), "{}", outcome.err);
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
        assert!(outcome.out.ends_with("Done\n"), "{}", outcome.out);
        assert!(outcome.err.is_empty(), "no progress: {}", outcome.err);
    }

    #[test]
    fn a_file_or_pipe_gets_a_line_per_frame() {
        let path = temp_gif("per-frame", 2);
        let screen = command(&["screen", path.to_str().unwrap()]);
        let (result, outcome) =
            with_options(&mut dry_device(0x0265), &screen, &real_options(false));
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        assert_eq!(outcome.err, "Frame 1 of 2 sent\nFrame 2 of 2 sent\n");
        assert!(outcome.out.ends_with("Done\n"), "{}", outcome.out);
    }

    #[test]
    fn a_terminal_gets_one_line_that_keeps_changing() {
        let path = temp_gif("terminal", 1);
        let screen = command(&["screen", path.to_str().unwrap()]);
        let (result, outcome) = with_options(&mut dry_device(0x0265), &screen, &real_options(true));
        std::fs::remove_file(&path).unwrap();
        result.unwrap();
        assert!(
            outcome.err.starts_with("\rSent 1 of 217"),
            "{}",
            outcome.err
        );
        assert!(
            outcome.err.ends_with("\rSent 217 of 217\n"),
            "{}",
            outcome.err
        );
        assert!(!outcome.err.contains("Frame"), "{}", outcome.err);
    }

    #[test]
    fn finished_frames_are_counted_from_the_steps() {
        let finished: Vec<Option<usize>> = (0..=8).map(|done| finished_frame(done, 3)).collect();
        assert_eq!(
            finished,
            [None, None, None, None, Some(1), None, None, Some(2), None]
        );
    }

    #[test]
    fn cancelling_before_the_upload_sends_nothing_and_isnt_done() {
        let path = temp_gif("cancel-first", 1);
        let cancel = AtomicBool::new(true);
        let options = Options {
            cancel: &cancel,
            ..dry_options()
        };
        let screen = command(&["screen", path.to_str().unwrap()]);
        let (result, outcome) = with_options(&mut dry_device(0x0265), &screen, &options);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(result.unwrap_err().to_string(), "cancelled");
        assert!(outcome.sent.is_empty());
        assert!(!outcome.out.contains("Done"), "{}", outcome.out);
    }

    #[test]
    fn cancelling_during_the_upload_ends_with_the_abort() {
        let path = temp_gif("cancel-during", 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (hook_cancel, hook_sent) = (Arc::clone(&cancel), Arc::clone(&sent));
        let mut device = dry_device(0x0265);
        device.on_packet = Some(Box::new(move |event| {
            if let PacketEvent::Sent { report, .. } = event {
                let mut sent = hook_sent.lock().unwrap();
                sent.push(report.to_vec());
                if sent.len() == 5 {
                    hook_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut console = Console {
            out: &mut out,
            err: &mut err,
        };
        let options = Options {
            cancel: &cancel,
            ..dry_options()
        };
        let screen = command(&["screen", path.to_str().unwrap()]);
        let result = run(&mut device, &screen, &options, &mut console);
        std::fs::remove_file(&path).unwrap();

        assert_eq!(result.unwrap_err().to_string(), "cancelled");
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 6);
        assert_eq!(&sent[5][1..2], &[0x0D]);
        assert_eq!(sent[5][8], 0x80, "the abort");
        assert!(!String::from_utf8(out).unwrap().contains("Done"));
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
        assert!(outcome.out.ends_with("Done\n"), "{}", outcome.out);
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
        assert_eq!(
            result.unwrap_err().to_string(),
            "the keyboard didn't acknowledge chunk 4 of frame 0. An abort was sent"
        );
        assert_eq!(outcome.sent.last().unwrap()[8], 0x80, "the abort is last");
        assert!(!outcome.out.contains("Done"), "{}", outcome.out);
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
    fn fit_defaults_to_cover() {
        let KeyboardCommand::Screen(screen) = command(&["screen", "a.gif"]) else {
            panic!("not screen");
        };
        assert_eq!(screen.fit, FitArg::Cover);
    }
}
