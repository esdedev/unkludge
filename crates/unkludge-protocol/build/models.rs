//! Parses one `data/models/<PID>.toml` and renders the table as Rust source. Used by `build.rs`
//! and by `tests/build_models.rs`.
//!
//! Fields not named here (key positions, wireless IDs) are ignored: this version doesn't use them,
//! and compiling them in would only grow the binary. Two are read only to be refused on a model this
//! version supports, as ignoring them would give it wrong effect numbers: `hw`, an effect number other
//! than the effect's own, and `led_mask`, which hides effects and so changes which are offered.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde::Deserialize;

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum RawProtocol {
    V1,
    V2,
    V2alt,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawStatus {
    Tested,
    Experimental,
}

#[derive(Deserialize)]
struct RawScreen {
    width: u16,
    height: u16,
    max_frames: u16,
    kind: u8,
    crc: bool,
    draw_frame_time: u16,
}

#[derive(Deserialize)]
struct RawSpeed {
    min: u8,
    max: u8,
    default: u8,
}

#[derive(Deserialize)]
struct RawEffect {
    id: u8,
    name: String,
    color: bool,
    speed: bool,
    brightness: bool,
    rainbow: bool,
    /// Absent means 0, which means the effect's number is its `id`.
    #[serde(default)]
    hw: u32,
}

#[derive(Deserialize)]
struct RawLighting {
    #[serde(default)]
    brightness: Vec<u8>,
    speed: Option<RawSpeed>,
    sleep: bool,
    effects: Vec<RawEffect>,
    #[serde(default)]
    led_mask: u32,
}

/// One model, checked on its own. [`render`] checks the models against each other.
#[derive(Deserialize)]
pub struct RawModel {
    pid: u16,
    name: String,
    protocol: RawProtocol,
    fw: u8,
    status: RawStatus,
    layout: Option<String>,
    screen: Option<RawScreen>,
    lighting: RawLighting,
}

/// Parses and checks one file. Errors start with the file name.
pub fn parse_model(file: &str, text: &str) -> Result<RawModel, String> {
    let model: RawModel = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;

    let expected_file = format!("{:04X}.toml", model.pid);
    if file != expected_file {
        return Err(format!(
            "{file}: pid {:04X} belongs in {expected_file}",
            model.pid
        ));
    }

    let expected_protocol = match model.fw {
        0 => RawProtocol::V1,
        1 => RawProtocol::V2,
        2 => RawProtocol::V2alt,
        fw => return Err(format!("{file}: unknown fw {fw}")),
    };
    if model.protocol != expected_protocol {
        return Err(format!(
            "{file}: fw {} doesn't match the protocol",
            model.fw
        ));
    }

    if let Some(screen) = &model.screen {
        if screen.width == 0 || screen.height == 0 || screen.max_frames == 0 {
            return Err(format!("{file}: screen with a zero size or frame limit"));
        }
        if !matches!(screen.kind, 1 | 2) {
            return Err(format!("{file}: unknown screen kind {}", screen.kind));
        }
    }

    if let Some(layout) = &model.layout {
        parse_layout(layout).map_err(|e| format!("{file}: {e}"))?;
    }

    if model.fw == 1 && model.screen.is_some() {
        check_supported_lighting(&model.lighting).map_err(|e| format!("{file}: {e}"))?;
    }

    Ok(model)
}

/// What a model this version supports can't have, as its effect numbers would come out wrong.
fn check_supported_lighting(lighting: &RawLighting) -> Result<(), String> {
    if lighting.led_mask != 0 {
        return Err(format!(
            "led_mask {:#06X} hides effects, which this version doesn't do",
            lighting.led_mask
        ));
    }
    if let Some(effect) = lighting.effects.iter().find(|effect| effect.hw != 0) {
        return Err(format!(
            "effect {} has hw = {}, so its number isn't its id, which this version doesn't do",
            effect.id, effect.hw
        ));
    }
    Ok(())
}

fn parse_layout(layout: &str) -> Result<u16, String> {
    u16::from_str_radix(layout, 16).map_err(|_| format!("layout {layout:?} isn't a hex PID"))
}

/// Renders the table, sorted by PID. Models with the same effects share one table.
pub fn render(mut models: Vec<RawModel>) -> Result<String, String> {
    if models.is_empty() {
        return Err("no model files found".to_string());
    }

    models.sort_by_key(|m| m.pid);
    for pair in models.windows(2) {
        if pair[0].pid == pair[1].pid {
            return Err(format!("duplicate pid {:04X}", pair[0].pid));
        }
    }

    let mut effect_tables = BTreeMap::new();
    let mut table = String::from("pub static MODELS: &[Model] = &[\n");
    for model in &models {
        let effects = render_effects(&model.lighting.effects);
        let next = effect_tables.len();
        let table_id = *effect_tables.entry(effects).or_insert(next);
        let line = render_model(model, table_id, &models)?;
        table.push_str(&line);
    }
    table.push_str("];\n");

    let mut out = String::new();
    for (effects, id) in &effect_tables {
        writeln!(out, "static EFFECTS_{id}: &[Effect] = &[{effects}];").unwrap();
    }
    out.push_str(&table);
    Ok(out)
}

fn render_effects(effects: &[RawEffect]) -> String {
    let mut out = String::new();
    for e in effects {
        write!(
            out,
            "Effect {{ id: {}, name: {:?}, color: {}, speed: {}, brightness: {}, rainbow: {} }},",
            e.id, e.name, e.color, e.speed, e.brightness, e.rainbow
        )
        .unwrap();
    }
    out
}

fn render_model(model: &RawModel, effects: usize, all: &[RawModel]) -> Result<String, String> {
    let protocol = match model.protocol {
        RawProtocol::V1 => "V1",
        RawProtocol::V2 => "V2",
        RawProtocol::V2alt => "V2Alt",
    };
    let status = match model.status {
        RawStatus::Tested => "Tested",
        RawStatus::Experimental => "Experimental",
    };
    let screen = match &model.screen {
        Some(s) => format!(
            "Some(ScreenSpec {{ width: {}, height: {}, max_frames: {}, kind: {}, crc: {}, draw_frame_time: {} }})",
            s.width, s.height, s.max_frames, s.kind, s.crc, s.draw_frame_time
        ),
        None => "None".to_string(),
    };
    let layout = match &model.layout {
        Some(layout) => {
            let pid = parse_layout(layout)?;
            if all.iter().all(|m| m.pid != pid) {
                return Err(format!(
                    "{:04X}.toml: layout {pid:04X} has no model",
                    model.pid
                ));
            }
            format!("Some(0x{pid:04X})")
        }
        None => "None".to_string(),
    };
    let lighting = &model.lighting;
    let (speed_min, speed_max, speed_default) = lighting
        .speed
        .as_ref()
        .map_or((0, 0, 0), |s| (s.min, s.max, s.default));

    Ok(format!(
        "Model {{ pid: 0x{:04X}, name: {:?}, protocol: Protocol::{protocol}, status: Status::{status}, screen: {screen}, lighting: Lighting {{ brightness: &{:?}, speed_min: {speed_min}, speed_max: {speed_max}, speed_default: {speed_default}, effects: EFFECTS_{effects}, sleep: {} }}, layout: {layout} }},\n",
        model.pid, model.name, lighting.brightness, lighting.sleep,
    ))
}
