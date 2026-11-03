//! The text of a report, the same from this tool and from the app, which compiles this same file.
//! Reports are for whoever looks into a model, so they're always in English.

use unkludge_protocol::Error;
use unkludge_protocol::device::HidCollection;
use unkludge_protocol::model::{Model, Status, VID};

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

/// What a report says about the computer: whether the official app is running, and every collection
/// the system lists for this vendor.
pub fn host_lines(
    rk_app_running: bool,
    collections: &Result<Vec<HidCollection>, Error>,
) -> Vec<String> {
    let yes_no = if rk_app_running { "yes" } else { "no" };
    let mut lines = vec![format!("RK app running: {yes_no}")];
    match collections {
        Ok(collections) if collections.is_empty() => {
            lines.push(format!("Collections for vendor {VID:04X}: none"));
        }
        Ok(collections) => {
            lines.push(format!("Collections for vendor {VID:04X}:"));
            lines.extend(collections.iter().map(|c| {
                format!(
                    "  PID {:04X}, usage page {:04X}, usage {:04X}, interface {}",
                    c.pid, c.usage_page, c.usage, c.interface
                )
            }));
        }
        Err(error) => lines.push(format!("Collections: couldn't list them: {error}")),
    }
    lines
}

/// The part of a report that comes from the keyboard. A read that failed is just what a report is
/// for, so it's told like anything else.
pub fn keyboard_lines(model: &Model, config: Result<&[u8], String>) -> Vec<String> {
    let mut lines = vec![
        format!("Model: {}", model.name),
        format!("PID: {:04X}", model.pid),
        format!("Status: {}", status_label(model.status)),
    ];
    match config {
        Ok(block) => {
            lines.push("Config block:".to_string());
            lines.extend(config_lines(block));
        }
        Err(error) => lines.push(format!("Config block: couldn't read it: {error}")),
    }
    lines
}

pub fn status_label(status: Status) -> &'static str {
    match status {
        Status::Tested => "tested",
        Status::Experimental => "experimental",
    }
}

/// The config block in rows of 16 bytes, each with its offset.
pub fn config_lines(block: &[u8]) -> Vec<String> {
    block
        .chunks(16)
        .enumerate()
        .map(|(row, bytes)| format!("  {:02X}: {}", row * 16, hex(bytes)))
        .collect()
}

/// `09 0B 00`, in capitals, with a space between bytes.
pub fn hex(bytes: &[u8]) -> String {
    let pairs: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    pairs.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::model;

    #[test]
    fn host_lines_list_every_collection() {
        let collections = vec![
            HidCollection {
                pid: 0x0265,
                usage_page: 0xFF02,
                usage: 2,
                interface: 1,
            },
            HidCollection {
                pid: 0x0265,
                usage_page: 0x0001,
                usage: 6,
                interface: 0,
            },
        ];
        assert_eq!(
            host_lines(false, &Ok(collections)),
            [
                "RK app running: no",
                "Collections for vendor 258A:",
                "  PID 0265, usage page FF02, usage 0002, interface 1",
                "  PID 0265, usage page 0001, usage 0006, interface 0",
            ]
        );
    }

    #[test]
    fn host_lines_say_when_there_is_nothing_to_list() {
        assert_eq!(
            host_lines(true, &Ok(Vec::new())),
            ["RK app running: yes", "Collections for vendor 258A: none"]
        );
        assert_eq!(
            host_lines(false, &Err(Error::Hid("no access".to_string())))[1],
            "Collections: couldn't list them: USB error: no access"
        );
    }

    #[test]
    fn keyboard_lines_show_the_block_or_why_it_is_missing() {
        let s85 = model::find(0x0265).unwrap();
        let lines = keyboard_lines(s85, Ok(&[0x5A, 0xA5]));
        assert_eq!(
            lines,
            [
                "Model: RK S85 ES",
                "PID: 0265",
                "Status: experimental",
                "Config block:",
                "  00: 5A A5",
            ]
        );
        let lines = keyboard_lines(s85, Err("gone".to_string()));
        assert_eq!(lines[3], "Config block: couldn't read it: gone");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn the_config_prints_in_rows_with_offsets() {
        let mut block = [0; 128];
        block[0x7E] = 0x5A;
        block[0x7F] = 0xA5;
        let lines = config_lines(&block);
        assert_eq!(lines.len(), 8);
        assert_eq!(
            lines[0],
            "  00: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"
        );
        assert_eq!(
            lines[7],
            "  70: 00 00 00 00 00 00 00 00 00 00 00 00 00 00 5A A5"
        );
    }
}
