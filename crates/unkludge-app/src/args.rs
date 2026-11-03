//! The command line. A flag this doesn't know stops the app from starting: `--dmeo` must not end up
//! as the normal app, which would write the clock to a real keyboard. For the same reason any
//! `--demo-…` flag means the demo, with or without `--demo` itself.

use std::path::PathBuf;
use std::time::Duration;

use unkludge_protocol::model;

use crate::demo::DemoState;

/// The model a demo shows unless told otherwise: the one the project is developed on.
const DEFAULT_DEMO_PID: u16 = 0x0265;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Spanish,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoArgs {
    pub pid: u16,
    pub state: DemoState,
    /// Draws at this DPI instead of the monitor's, to check other scales on one monitor.
    pub dpi: Option<u32>,
    /// Lays out for a work area of this many pixels instead of the monitor's.
    pub work_area: Option<(i32, i32)>,
    /// Writes a PNG of the window here and quits.
    pub shot: Option<PathBuf>,
    /// How long after the demo reached its state to take the picture.
    pub shot_delay: Duration,
    /// Quits by itself after this long, so that scripted runs leave nothing behind.
    pub exit_after: Option<Duration>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Start hidden in the tray, which is how the app starts with the system.
    pub tray: bool,
    pub language: Option<Language>,
    /// Never opens a keyboard, and leaves the real settings and autostart alone.
    pub demo: Option<DemoArgs>,
}

impl Default for DemoArgs {
    fn default() -> Self {
        DemoArgs {
            pid: DEFAULT_DEMO_PID,
            state: DemoState::Connected,
            dpi: None,
            work_area: None,
            shot: None,
            shot_delay: Duration::from_millis(600),
            exit_after: None,
        }
    }
}

/// `args` without the program's own name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        // Anything naming the demo means the demo, even if the rest of the line turns out wrong.
        let mut value = |what: &str| {
            args.next()
                .ok_or_else(|| format!("{flag} needs {what} after it"))
        };
        match flag.as_str() {
            "--tray" => parsed.tray = true,
            "--lang" => parsed.language = Some(parse_language(&value("es or en")?)?),
            "--demo" => {
                parsed.demo.get_or_insert_with(DemoArgs::default);
            }
            "--demo-model" => {
                let pid = parse_pid(&value("a product ID")?)?;
                parsed.demo.get_or_insert_with(DemoArgs::default).pid = pid;
            }
            "--demo-state" => {
                let name = value("a state's name")?;
                let state = DemoState::parse(&name).ok_or_else(|| {
                    format!("no demo state {name}: {}", DemoState::names().join(", "))
                })?;
                parsed.demo.get_or_insert_with(DemoArgs::default).state = state;
            }
            "--demo-dpi" => {
                let dpi = parse_number(&value("a DPI")?)?;
                if !(48..=480).contains(&dpi) {
                    return Err(format!("a DPI of {dpi} isn't useful"));
                }
                parsed.demo.get_or_insert_with(DemoArgs::default).dpi = Some(dpi);
            }
            "--demo-work-area" => {
                let area = parse_area(&value("a size, such as 1366x708")?)?;
                parsed.demo.get_or_insert_with(DemoArgs::default).work_area = Some(area);
            }
            "--demo-shot" => {
                let path = PathBuf::from(value("a file")?);
                parsed.demo.get_or_insert_with(DemoArgs::default).shot = Some(path);
            }
            "--demo-shot-delay" => {
                let ms = parse_number(&value("milliseconds")?)?;
                parsed.demo.get_or_insert_with(DemoArgs::default).shot_delay =
                    Duration::from_millis(u64::from(ms));
            }
            "--demo-exit-after" => {
                let ms = parse_number(&value("milliseconds")?)?;
                parsed.demo.get_or_insert_with(DemoArgs::default).exit_after =
                    Some(Duration::from_millis(u64::from(ms)));
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(parsed)
}

fn parse_language(text: &str) -> Result<Language, String> {
    match text {
        "en" => Ok(Language::English),
        "es" => Ok(Language::Spanish),
        other => Err(format!("no language {other}: es or en")),
    }
}

fn parse_number(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("{text} isn't a number"))
}

fn parse_area(text: &str) -> Result<(i32, i32), String> {
    let problem = || format!("{text} isn't a size: it goes like 1366x708");
    let (width, height) = text.split_once('x').ok_or_else(problem)?;
    let number = |part: &str| part.parse().ok().filter(|n| *n > 0).ok_or_else(problem);
    Ok((number(width)?, number(height)?))
}

/// A supported model's product ID, in hexadecimal.
fn parse_pid(text: &str) -> Result<u16, String> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    let pid = u16::from_str_radix(digits, 16).map_err(|_| format!("{text} isn't a product ID"))?;
    match model::find(pid) {
        Some(model) if model.is_supported() => Ok(pid),
        Some(model) => Err(format!("{} ({pid:04X}) isn't supported", model.name)),
        None => Err(format!("no model {pid:04X}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(ToString::to_string))
    }

    #[test]
    fn no_flags_is_the_normal_app() {
        let args = parsed(&[]).unwrap();
        assert_eq!(args, Args::default());
        assert!(args.demo.is_none());
    }

    #[test]
    fn the_tray_flag_starts_hidden_and_isnt_a_demo() {
        let args = parsed(&["--tray"]).unwrap();
        assert!(args.tray);
        assert!(args.demo.is_none());
    }

    #[test]
    fn a_mistyped_flag_doesnt_start_the_normal_app() {
        assert!(parsed(&["--dmeo"]).is_err());
        assert!(parsed(&["-demo"]).is_err());
        assert!(parsed(&["demo"]).is_err());
        assert!(parsed(&["--demo-modle", "0265"]).is_err());
    }

    #[test]
    fn every_demo_flag_means_the_demo_on_its_own() {
        for flags in [
            &["--demo"][..],
            &["--demo-model", "01A2"],
            &["--demo-state", "image"],
            &["--demo-dpi", "144"],
            &["--demo-work-area", "1366x708"],
            &["--demo-shot", "a.png"],
            &["--demo-shot-delay", "10"],
            &["--demo-exit-after", "10"],
        ] {
            assert!(parsed(flags).unwrap().demo.is_some(), "{flags:?}");
        }
    }

    #[test]
    fn the_demo_defaults_to_a_connected_s85_es() {
        let demo = parsed(&["--demo"]).unwrap().demo.unwrap();
        assert_eq!(demo.pid, 0x0265);
        assert_eq!(demo.state, DemoState::Connected);
    }

    #[test]
    fn a_demo_model_is_read_in_hexadecimal() {
        for (text, pid) in [("01A2", 0x01A2), ("0x01b8", 0x01B8), ("187", 0x0187)] {
            let demo = parsed(&["--demo-model", text]).unwrap().demo.unwrap();
            assert_eq!(demo.pid, pid);
        }
    }

    #[test]
    fn a_demo_model_must_be_one_this_version_supports() {
        assert!(parsed(&["--demo-model", "FFFF"]).is_err());
        assert!(parsed(&["--demo-model", "zz"]).is_err());
        assert!(parsed(&["--demo-model", "002A"]).is_err(), "a V1 model");
    }

    #[test]
    fn the_language_can_be_chosen() {
        let args = parsed(&["--lang", "es"]).unwrap();
        assert_eq!(args.language, Some(Language::Spanish));
        assert!(args.demo.is_none());
        let args = parsed(&["--lang", "en"]).unwrap();
        assert_eq!(args.language, Some(Language::English));
        assert!(parsed(&["--lang", "fr"]).is_err());
    }

    #[test]
    fn a_flag_missing_its_value_is_refused() {
        for flag in ["--lang", "--demo-model", "--demo-state", "--demo-dpi"] {
            let error = parsed(&[flag]).unwrap_err();
            assert!(error.contains(flag), "{error}");
        }
    }

    #[test]
    fn a_wrong_demo_state_lists_the_right_ones() {
        let error = parsed(&["--demo-state", "nope"]).unwrap_err();
        assert!(error.contains("uploading"), "{error}");
    }

    #[test]
    fn a_work_area_is_a_size() {
        let demo = parsed(&["--demo-work-area", "1366x708"])
            .unwrap()
            .demo
            .unwrap();
        assert_eq!(demo.work_area, Some((1366, 708)));
        for wrong in ["1366", "x", "0x708", "ax708", "1366x"] {
            assert!(parsed(&["--demo-work-area", wrong]).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_silly_dpi_is_refused() {
        assert!(parsed(&["--demo-dpi", "9000"]).is_err());
        assert!(parsed(&["--demo-dpi", "abc"]).is_err());
        let demo = parsed(&["--demo-dpi", "144"]).unwrap().demo.unwrap();
        assert_eq!(demo.dpi, Some(144));
    }
}
