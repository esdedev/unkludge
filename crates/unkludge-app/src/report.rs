//! The report someone testing a model can send: what the app knows, as a text file with nothing
//! personal in it. The wording is shared with `unkludge report`.

use std::io;
use std::path::{Path, PathBuf};

use unkludge_protocol::Error;
use unkludge_protocol::device::{HidCollection, list_collections};
use unkludge_shared::report_lines::{config_report, host_lines, keyboard_header, report_header};

use crate::worker::ReportData;

/// The form a report gets attached to. It doesn't exist yet, as the project isn't public: until it
/// does, show the saved file instead of opening a page.
pub const FORM_URL: Option<&str> = None;

/// The report's text. The caller finds out what the computer has to say, so this stays testable on
/// any machine.
pub fn text(
    data: &ReportData,
    rk_app_running: bool,
    collections: &Result<Vec<HidCollection>, Error>,
) -> String {
    let mut lines = report_header();
    lines.extend(host_lines(rk_app_running, collections));
    let block = data.config.as_deref().map_err(Clone::clone);
    match (data.model, block) {
        (Some(model), block) => {
            lines.extend(keyboard_header(model));
            lines.extend(config_report(block));
        }
        (None, Ok(_)) => lines.push("Keyboard: unknown".to_string()),
        (None, Err(why)) => lines.push(format!("Keyboard: {why}")),
    }
    if data.errors.is_empty() {
        lines.push("Last errors: none".to_string());
    } else {
        lines.push("Last errors:".to_string());
        lines.extend(data.errors.iter().map(|error| format!("  {error}")));
    }
    lines.join("\n") + "\n"
}

/// Saves the report in the data folder and returns the file's path. This asks the system what's
/// running and what it lists, but opens no device.
pub fn save(data: &ReportData) -> io::Result<PathBuf> {
    let dir = crate::sys::data_dir().ok_or(io::ErrorKind::NotFound)?;
    let collections = list_collections();
    save_in(
        &dir,
        &text(data, crate::sys::rk_app_running(), &collections),
    )
}

/// Writes the report into `dir`, making it if needed, and returns the file's path.
pub fn save_in(dir: &Path, text: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("unkludge-report.txt");
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::model;

    fn collections() -> Result<Vec<HidCollection>, Error> {
        Ok(vec![HidCollection {
            pid: 0x0265,
            usage_page: 0xFF02,
            usage: 2,
            interface: 1,
        }])
    }

    fn block() -> Vec<u8> {
        let mut block = vec![0; 128];
        block[0x0A] = 5;
        block[0x7E..].copy_from_slice(&[0x5A, 0xA5]);
        block
    }

    #[test]
    fn a_report_has_everything_someone_needs_to_look_into_a_model() {
        let data = ReportData {
            model: model::find(0x0265),
            config: Ok(block()),
            errors: vec!["USB error: gone".to_string(), "cancelled".to_string()],
        };
        let text = text(&data, true, &collections());
        for wanted in [
            format!("Unkludge: {}", env!("CARGO_PKG_VERSION")),
            format!(
                "System: {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
            "RK app running: yes".to_string(),
            "PID 0265, usage page FF02, usage 0002, interface 1".to_string(),
            "Model: RK S85 ES".to_string(),
            "PID: 0265".to_string(),
            "Status: ".to_string(),
            "Config block:".to_string(),
            "  00: 00 00 00 00 00 00 00 00 00 00 05 00".to_string(),
            "5A A5".to_string(),
            "Last errors:".to_string(),
            "  USB error: gone".to_string(),
            "  cancelled".to_string(),
        ] {
            assert!(text.contains(&wanted), "no {wanted:?} in:\n{text}");
        }
    }

    #[test]
    fn a_report_says_when_something_is_missing() {
        let data = ReportData {
            model: None,
            config: Err("no keyboard connected".to_string()),
            errors: Vec::new(),
        };
        let text = text(&data, false, &Err(Error::Hid("no access".to_string())));
        assert!(text.contains("RK app running: no"), "{text}");
        assert!(
            text.contains("Collections: couldn't list them: USB error: no access"),
            "{text}"
        );
        assert!(text.contains("Keyboard: no keyboard connected"), "{text}");
        assert!(text.contains("Last errors: none"), "{text}");
    }

    #[test]
    fn a_config_that_couldnt_be_read_is_in_the_report() {
        let data = ReportData {
            model: model::find(0x0265),
            config: Err("not read: an upload is running".to_string()),
            errors: Vec::new(),
        };
        let text = text(&data, false, &collections());
        assert!(text.contains("Model: RK S85 ES"), "{text}");
        assert!(
            text.contains("Config block: couldn't read it: not read: an upload is running"),
            "{text}"
        );
    }

    #[test]
    fn a_report_is_saved_where_it_is_asked_to_be() {
        let dir = std::env::temp_dir().join(format!("unkludge-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_in(&dir, "the text").unwrap();
        assert_eq!(path, dir.join("unkludge-report.txt"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the text");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_form_doesnt_exist_yet() {
        assert_eq!(FORM_URL, None);
    }
}
