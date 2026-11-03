//! What the commands print.

use std::fmt::Display;
use std::io::Write;

use unkludge_protocol::config::SleepTime;
use unkludge_protocol::lighting::{ColorSetting, CurrentLighting, LightMode, SleepSetting};
use unkludge_protocol::model::{Effect, Lighting, Model, Status};

use crate::packets::hex;

/// Where a command's output goes. Output that can't be written, as when a pipe has closed, is no
/// reason to stop talking to a keyboard halfway through.
pub struct Console<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

impl Console<'_> {
    pub fn line(&mut self, text: impl Display) {
        let _ = writeln!(self.out, "{text}");
    }

    pub fn lines(&mut self, lines: impl IntoIterator<Item = impl Display>) {
        for line in lines {
            self.line(line);
        }
    }

    pub fn note(&mut self, text: impl Display) {
        let _ = writeln!(self.err, "{text}");
    }

    pub fn warn(&mut self, text: impl Display) {
        self.note(format_args!("warning: {text}"));
    }

    /// Rewrites one line on a terminal.
    pub fn progress(&mut self, done: usize, total: usize) {
        let _ = write!(self.err, "\rSent {done} of {total}");
    }
}

/// The supported models as a table, one per row after the heading.
pub fn models_table(models: impl IntoIterator<Item = &'static Model>) -> Vec<String> {
    let models: Vec<&Model> = models.into_iter().collect();
    let name_width = models.iter().map(|m| m.name.len()).max().unwrap_or(0);
    let mut lines = vec![format!(
        "PID  {:<name_width$}  Screen   Frames  Status",
        "Name"
    )];
    lines.extend(models.iter().map(|model| {
        let (size, frames) = model.screen.map_or((String::new(), 0), |s| {
            (format!("{}x{}", s.width, s.height), s.max_frames)
        });
        format!(
            "{:04X} {:<name_width$}  {size:<7}  {frames:<6}  {}",
            model.pid,
            model.name,
            status_label(model.status)
        )
    }));
    lines
}

pub fn status_label(status: Status) -> &'static str {
    match status {
        Status::Tested => "tested",
        Status::Experimental => "experimental",
    }
}

/// What a model is and what it offers.
pub fn model_lines(model: &Model) -> Vec<String> {
    let lighting = &model.lighting;
    let mut lines = vec![
        format!("Model: {}", model.name),
        format!("PID: {:04X}", model.pid),
        format!("Status: {}", status_label(model.status)),
    ];
    if let Some(screen) = model.screen {
        lines.push(format!(
            "Screen: {}x{}, up to {} frames",
            screen.width, screen.height, screen.max_frames
        ));
    }
    let brightness: Vec<String> = lighting.brightness.iter().map(u8::to_string).collect();
    lines.push(format!("Brightness: {}", brightness.join(", ")));
    lines.push(format!(
        "Speed: {} to {}",
        lighting.speed_min, lighting.speed_max
    ));
    if lighting.sleep {
        let labels: Vec<&str> = SleepTime::ALL.iter().map(|s| s.label()).collect();
        lines.push(format!("Sleep: {}", labels.join(", ")));
    }
    lines.push("Effects:".to_string());
    lines.extend(lighting.effects.iter().map(effect_line));
    lines
}

fn effect_line(effect: &Effect) -> String {
    let controls = [
        (effect.brightness, "brightness"),
        (effect.speed, "speed"),
        (effect.color, "colour"),
        (effect.rainbow, "rainbow"),
    ]
    .into_iter()
    .filter_map(|(has, name)| has.then_some(name))
    .collect::<Vec<_>>()
    .join(", ");
    format!("  {:>2} {:<12}  {controls}", effect.id, effect.name)
}

/// The lighting settings, each on a line. A control the current effect doesn't have shows as `-`.
pub fn lighting_lines(current: &CurrentLighting, lighting: &Lighting) -> Vec<String> {
    let number = |value: Option<u8>| value.map_or("-".to_string(), |value| value.to_string());
    vec![
        format!("Mode: {}", mode_text(current.mode, lighting)),
        format!("Brightness: {}", number(current.brightness)),
        format!("Speed: {}", number(current.speed)),
        format!("Colour: {}", color_text(current.color)),
        format!("Sleep: {}", sleep_text(current.sleep)),
    ]
}

fn mode_text(mode: LightMode, lighting: &Lighting) -> String {
    match mode {
        LightMode::Off => "off".to_string(),
        LightMode::Effect(id) => match lighting.effects.iter().find(|e| e.id == id) {
            Some(effect) => format!("{id} ({})", effect.name),
            None => id.to_string(),
        },
        LightMode::Custom => "custom, set with another tool".to_string(),
        LightMode::Unknown(byte) => format!("unknown effect ({byte:#04X})"),
    }
}

fn color_text(color: Option<ColorSetting>) -> String {
    match color {
        None => "-".to_string(),
        Some(ColorSetting::Rainbow) => "rainbow".to_string(),
        Some(ColorSetting::Slot(slot)) => format!("palette slot {slot}"),
        Some(ColorSetting::Unknown(value)) => format!("unknown ({value:#04X})"),
    }
}

fn sleep_text(sleep: SleepSetting) -> String {
    match sleep {
        SleepSetting::Known(time) => format!("{} ({:#04X})", time.label(), time.byte()),
        SleepSetting::Unknown(byte) => format!("unknown ({byte:#04X})"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::model;

    #[test]
    fn the_table_has_a_row_per_supported_model() {
        let lines = models_table(model::supported());
        assert_eq!(lines.len(), 1 + 29);
        assert!(lines[0].starts_with("PID  Name"));
        let s85 = lines.iter().find(|l| l.starts_with("0265 ")).unwrap();
        assert!(s85.starts_with("0265 RK S85 ES"), "{s85}");
        assert!(s85.contains("320x172"), "{s85}");
        assert!(s85.contains("60"), "{s85}");
        assert!(s85.ends_with("experimental"), "{s85}");
    }

    #[test]
    fn the_columns_line_up() {
        let lines = models_table(model::supported());
        let at = lines[0].find("Screen").unwrap();
        for line in &lines[1..] {
            let size = &line[at..at + 7];
            assert!(size.starts_with(['1', '2', '3']), "{line}");
        }
    }

    #[test]
    fn model_lines_say_what_the_model_offers() {
        let lines = model_lines(model::find(0x0265).unwrap());
        assert_eq!(lines[0], "Model: RK S85 ES");
        assert_eq!(lines[1], "PID: 0265");
        assert!(lines.contains(&"Screen: 320x172, up to 60 frames".to_string()));
        assert!(lines.contains(&"Brightness: 0, 4, 8, 12, 16, 20".to_string()));
        assert!(lines.contains(&"Speed: 0 to 5".to_string()));
        assert!(lines.contains(&"Sleep: 5 min, 10 min, 20 min, No sleep".to_string()));
        assert!(lines.contains(&"   1 Solid         brightness, colour, rainbow".to_string()));
        assert!(lines.contains(&"  17 Blossom       brightness, speed".to_string()));
    }

    #[test]
    fn lighting_shows_the_current_effect_and_the_raw_sleep_byte() {
        let lighting = &model::find(0x0265).unwrap().lighting;
        let current = CurrentLighting {
            mode: LightMode::Effect(1),
            brightness: Some(20),
            speed: None,
            color: Some(ColorSetting::Slot(0)),
            sleep: SleepSetting::Known(SleepTime::Min10),
        };
        assert_eq!(
            lighting_lines(&current, lighting),
            [
                "Mode: 1 (Solid)",
                "Brightness: 20",
                "Speed: -",
                "Colour: palette slot 0",
                "Sleep: 10 min (0x14)",
            ]
        );
    }

    #[test]
    fn values_the_keyboard_may_hold_that_have_no_name_still_show() {
        let lighting = &model::find(0x0265).unwrap().lighting;
        let current = CurrentLighting {
            mode: LightMode::Unknown(0x40),
            brightness: None,
            speed: None,
            color: Some(ColorSetting::Unknown(0x0F)),
            sleep: SleepSetting::Unknown(0x07),
        };
        assert_eq!(
            lighting_lines(&current, lighting),
            [
                "Mode: unknown effect (0x40)",
                "Brightness: -",
                "Speed: -",
                "Colour: unknown (0x0F)",
                "Sleep: unknown (0x07)",
            ]
        );
    }

    #[test]
    fn off_and_custom_have_their_own_words() {
        let lighting = &model::find(0x0265).unwrap().lighting;
        assert_eq!(mode_text(LightMode::Off, lighting), "off");
        assert_eq!(
            mode_text(LightMode::Custom, lighting),
            "custom, set with another tool"
        );
        assert_eq!(color_text(Some(ColorSetting::Rainbow)), "rainbow");
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
