//! The command line.

use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand, ValueEnum};
use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::{ColorChoice, LightingChange, ModeChoice, PALETTE_SLOTS, Rgb};
use unkludge_protocol::screen::{Fit, LoadOptions};

const GLOBAL: &str = "Options for every command";

/// Configure the screen, clock and lighting of Royal Kludge keyboards.
#[derive(Debug, Parser)]
#[command(name = "unkludge", version)]
pub struct Cli {
    /// Keyboard to use, by USB product ID in hex, such as 0265. Required with --dry-run
    #[arg(long, global = true, help_heading = GLOBAL, value_name = "HEX", value_parser = parse_pid)]
    pub pid: Option<u16>,

    /// Print the reports instead of opening a keyboard. Nothing is sent
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub dry_run: bool,

    /// Print how long sending and waiting for the keyboard took, with chunks per second
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub timing: bool,

    /// Print every report in full with --dry-run, and every ack with --timing
    #[arg(long, global = true, help_heading = GLOBAL)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// What can't be left to the parser, which sees global options differently depending on which
    /// side of the command they're on.
    pub fn check(&self) -> Result<(), String> {
        if self.verbose && !self.dry_run && !self.timing {
            return Err("--verbose needs --dry-run or --timing".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(flatten)]
    Keyboard(KeyboardCommand),
    /// List the supported models
    Models,
}

/// The commands that need a keyboard.
#[derive(Debug, Subcommand)]
pub enum KeyboardCommand {
    /// Show the model, what it offers and its current settings. Only reads
    Info,
    /// Show the lighting settings, or change them. With no options it only reads
    Light(LightArgs),
    /// Set the keyboard's clock to the local time
    Clock,
    /// Send an image or GIF to the keyboard's screen, replacing what's there
    Screen(ScreenArgs),
    /// Print the details a bug report needs. Only reads
    Report,
}

#[derive(Debug, Args)]
pub struct LightArgs {
    /// Effect number, or "off". `unkludge info` lists the effects
    #[arg(long, value_name = "N|off", value_parser = parse_mode)]
    pub mode: Option<ModeChoice>,

    /// Brightness byte, one of those `unkludge info` lists
    #[arg(long, value_name = "BYTE", value_parser = parse_number::<u8>)]
    pub brightness: Option<u8>,

    /// Speed, within the range `unkludge info` lists
    #[arg(long, value_name = "N", value_parser = parse_number::<u8>)]
    pub speed: Option<u8>,

    /// Palette slot from 0 to 6, or "rainbow"
    #[arg(long, value_name = "SLOT|rainbow", value_parser = parse_color, conflicts_with = "rgb")]
    pub color: Option<ColorChoice>,

    /// A colour of your own, as RRGGBB. The other effects get Unkludge's default colours
    #[arg(long, value_name = "RRGGBB", value_parser = parse_rgb)]
    pub rgb: Option<Rgb>,

    /// Sleep time: 5, 10, 20 or never. What "never" does on the keyboard is unconfirmed
    #[arg(long, value_name = "TIME", value_parser = parse_sleep)]
    pub sleep: Option<SleepTime>,
}

impl LightArgs {
    /// The settings to change. The default value means nothing is to change.
    pub fn change(&self) -> LightingChange {
        LightingChange {
            mode: self.mode,
            brightness: self.brightness,
            speed: self.speed,
            color: self.rgb.map(ColorChoice::Custom).or(self.color),
            sleep: self.sleep,
        }
    }
}

#[derive(Debug, Args)]
pub struct ScreenArgs {
    /// A GIF, PNG, JPEG, BMP or WebP file
    pub file: PathBuf,

    /// How the image is made to fit the screen
    #[arg(long, value_enum, default_value_t = FitArg::Cover)]
    pub fit: FitArg,

    /// Milliseconds per frame, instead of what the file says
    #[arg(long, value_name = "MS", value_parser = parse_number::<u16>)]
    pub delay: Option<u16>,

    /// Keep the frames that fit the keyboard's limit, instead of failing
    #[arg(long)]
    pub truncate: bool,
}

impl ScreenArgs {
    pub fn load_options(&self) -> LoadOptions {
        LoadOptions {
            fit: self.fit.into(),
            delay_ms: self.delay,
            truncate: self.truncate,
        }
    }
}

/// [`Fit`], as the command line spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FitArg {
    /// Fill the screen and crop what's left over
    Cover,
    /// Show the whole image, with black bars
    Contain,
    /// Fill the screen, changing the image's shape
    Stretch,
}

impl From<FitArg> for Fit {
    fn from(fit: FitArg) -> Fit {
        match fit {
            FitArg::Cover => Fit::Cover,
            FitArg::Contain => Fit::Contain,
            FitArg::Stretch => Fit::Stretch,
        }
    }
}

/// `from_str_radix` allows a leading sign, and these values have none.
fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A number written in plain digits. The standard parsers also allow a leading sign.
fn parse_number<T: FromStr>(text: &str) -> Result<T, String> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| text.parse().ok())
        .flatten()
        .ok_or_else(|| format!("{text:?} isn't a number in range"))
}

/// A product ID: up to four hex digits, with or without `0x`.
pub fn parse_pid(text: &str) -> Result<u16, String> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if !is_hex(digits) {
        return Err(format!("{text:?} isn't a hex product ID, such as 0265"));
    }
    if digits.len() > 4 {
        return Err(format!(
            "{text:?} is too long for a product ID, which has four hex digits"
        ));
    }
    u16::from_str_radix(digits, 16).map_err(|error| error.to_string())
}

/// A colour as `RRGGBB`, with or without `#`.
pub fn parse_rgb(text: &str) -> Result<Rgb, String> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !is_hex(digits) {
        return Err(format!("{text:?} isn't a colour as RRGGBB, such as FF1080"));
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
    Ok(Rgb::new(byte(0), byte(2), byte(4)))
}

/// An effect number, or `off`. The keyboard's model decides which numbers exist.
pub fn parse_mode(text: &str) -> Result<ModeChoice, String> {
    if text.eq_ignore_ascii_case("off") {
        return Ok(ModeChoice::Off);
    }
    parse_number(text)
        .map(ModeChoice::Effect)
        .map_err(|_| format!("{text:?} isn't an effect number or \"off\""))
}

/// A palette slot, or `rainbow`.
pub fn parse_color(text: &str) -> Result<ColorChoice, String> {
    if text.eq_ignore_ascii_case("rainbow") {
        return Ok(ColorChoice::Rainbow);
    }
    match parse_number(text) {
        Ok(slot) if slot < PALETTE_SLOTS => Ok(ColorChoice::Slot(slot)),
        _ => Err(format!(
            "{text:?} isn't a palette slot from 0 to {} or \"rainbow\"",
            PALETTE_SLOTS - 1
        )),
    }
}

/// A sleep time: the options the keyboard has.
pub fn parse_sleep(text: &str) -> Result<SleepTime, String> {
    match text.to_ascii_lowercase().as_str() {
        "5" => Ok(SleepTime::Min5),
        "10" => Ok(SleepTime::Min10),
        "20" => Ok(SleepTime::Min20),
        "never" => Ok(SleepTime::NoSleep),
        _ => Err(format!("{text:?} isn't one of 5, 10, 20 or never")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("unkludge").chain(args.iter().copied()))
    }

    fn light(args: &[&str]) -> LightArgs {
        match parse(args).unwrap().command {
            Command::Keyboard(KeyboardCommand::Light(light)) => light,
            other => panic!("not light: {other:?}"),
        }
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn pids_are_hex_with_or_without_a_prefix() {
        assert_eq!(parse_pid("0265"), Ok(0x0265));
        assert_eq!(parse_pid("0x0265"), Ok(0x0265));
        assert_eq!(parse_pid("0X01a2"), Ok(0x01A2));
        assert_eq!(parse_pid("1A2"), Ok(0x01A2));
    }

    #[test]
    fn other_pids_are_refused() {
        for bad in [
            "", "0x", "xyz", "12345", "00265", "0x00265", "+265", "-265", "0265 ", "0x0x1",
        ] {
            assert!(parse_pid(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn colours_are_six_hex_digits() {
        assert_eq!(parse_rgb("FF1080"), Ok(Rgb::new(0xFF, 0x10, 0x80)));
        assert_eq!(parse_rgb("ff1080"), Ok(Rgb::new(0xFF, 0x10, 0x80)));
        assert_eq!(parse_rgb("#000102"), Ok(Rgb::new(0, 1, 2)));
    }

    #[test]
    fn other_colours_are_refused() {
        for bad in ["", "#", "F", "FF108", "FF10800", "GG1080", "+F1080", "ÑÑÑ"] {
            assert!(parse_rgb(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn modes_are_numbers_or_off() {
        assert_eq!(parse_mode("off"), Ok(ModeChoice::Off));
        assert_eq!(parse_mode("OFF"), Ok(ModeChoice::Off));
        assert_eq!(parse_mode("2"), Ok(ModeChoice::Effect(2)));
        assert!(parse_mode("solid").is_err());
        assert!(parse_mode("256").is_err());
        assert!(parse_mode("+2").is_err());
        assert!(parse_mode("").is_err());
    }

    #[test]
    fn colours_by_slot_stop_at_the_palette_size() {
        assert_eq!(parse_color("rainbow"), Ok(ColorChoice::Rainbow));
        assert_eq!(parse_color("0"), Ok(ColorChoice::Slot(0)));
        assert_eq!(parse_color("6"), Ok(ColorChoice::Slot(6)));
        assert!(parse_color("7").is_err());
        assert!(parse_color("+1").is_err());
        assert!(parse_color("red").is_err());
    }

    #[test]
    fn sleep_takes_the_four_options() {
        assert_eq!(parse_sleep("5"), Ok(SleepTime::Min5));
        assert_eq!(parse_sleep("10"), Ok(SleepTime::Min10));
        assert_eq!(parse_sleep("20"), Ok(SleepTime::Min20));
        assert_eq!(parse_sleep("never"), Ok(SleepTime::NoSleep));
        assert!(parse_sleep("15").is_err());
        assert!(parse_sleep("0").is_err());
    }

    #[test]
    fn light_with_no_options_changes_nothing() {
        assert_eq!(light(&["light"]).change(), LightingChange::default());
    }

    #[test]
    fn light_options_become_a_change() {
        let change = light(&[
            "light",
            "--mode",
            "2",
            "--brightness",
            "8",
            "--speed",
            "3",
            "--color",
            "rainbow",
            "--sleep",
            "never",
        ])
        .change();
        assert_eq!(
            change,
            LightingChange {
                mode: Some(ModeChoice::Effect(2)),
                brightness: Some(8),
                speed: Some(3),
                color: Some(ColorChoice::Rainbow),
                sleep: Some(SleepTime::NoSleep),
            }
        );
    }

    #[test]
    fn rgb_is_a_custom_colour() {
        let change = light(&["light", "--rgb", "FF1080"]).change();
        assert_eq!(
            change.color,
            Some(ColorChoice::Custom(Rgb::new(0xFF, 0x10, 0x80)))
        );
    }

    #[test]
    fn a_slot_and_a_custom_colour_conflict() {
        let error = parse(&["light", "--rgb", "FF1080", "--color", "1"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn numbers_have_no_sign() {
        for args in [
            &["light", "--brightness", "+8"][..],
            &["light", "--speed", "+2"][..],
            &["light", "--brightness", "256"][..],
            &["light", "--mode", "+2"][..],
            &["screen", "a.gif", "--delay", "+50"][..],
            &["screen", "a.gif", "--delay", "65536"][..],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        assert_eq!(light(&["light", "--brightness", "8"]).brightness, Some(8));
        assert_eq!(light(&["light", "--speed", "2"]).speed, Some(2));
    }

    #[test]
    fn verbose_needs_a_dry_run_or_timing_in_any_order() {
        for args in [
            &["--verbose", "clock"][..],
            &["clock", "--verbose"][..],
            &["--verbose", "--pid", "0265", "clock"][..],
            &["--verbose", "models"][..],
        ] {
            assert!(parse(args).unwrap().check().is_err(), "{args:?}");
        }
        for args in [
            &["--verbose", "--dry-run", "clock"][..],
            &["--dry-run", "--verbose", "clock"][..],
            &["--verbose", "clock", "--dry-run"][..],
            &["--dry-run", "clock", "--verbose"][..],
            &["clock", "--verbose", "--dry-run"][..],
            &["clock", "--dry-run", "--verbose"][..],
            &["--verbose", "clock", "--timing"][..],
            &["clock", "--timing", "--verbose"][..],
            &["clock"][..],
        ] {
            assert!(parse(args).unwrap().check().is_ok(), "{args:?}");
        }
    }

    #[test]
    fn global_options_go_on_either_side_of_the_command() {
        let before = parse(&[
            "--pid",
            "0265",
            "--dry-run",
            "--timing",
            "--verbose",
            "clock",
        ])
        .unwrap();
        let after = parse(&[
            "clock",
            "--pid",
            "0265",
            "--dry-run",
            "--timing",
            "--verbose",
        ])
        .unwrap();
        for cli in [before, after] {
            assert_eq!(cli.pid, Some(0x0265));
            assert!(cli.dry_run && cli.timing && cli.verbose);
        }
    }

    #[test]
    fn screen_options_become_load_options() {
        let cli = parse(&[
            "screen",
            "a.gif",
            "--fit",
            "contain",
            "--delay",
            "50",
            "--truncate",
        ])
        .unwrap();
        let Command::Keyboard(KeyboardCommand::Screen(screen)) = cli.command else {
            panic!("not screen");
        };
        assert_eq!(screen.file, PathBuf::from("a.gif"));
        assert_eq!(
            screen.load_options(),
            LoadOptions {
                fit: Fit::Contain,
                delay_ms: Some(50),
                truncate: true,
            }
        );
    }

    #[test]
    fn screen_defaults_to_cover_and_the_files_own_delay() {
        let cli = parse(&["screen", "a.png"]).unwrap();
        let Command::Keyboard(KeyboardCommand::Screen(screen)) = cli.command else {
            panic!("not screen");
        };
        assert_eq!(screen.load_options(), LoadOptions::default());
    }
}
