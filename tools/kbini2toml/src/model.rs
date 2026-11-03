//! Turns one parsed `KB.ini` into a model, checks the set, and writes the TOML.

use crate::ini::{Ini, Section, parse_list, parse_num};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// Used when a file doesn't set `DrawFrameTime`.
const DEFAULT_DRAW_FRAME_TIME: u32 = 25;

/// Our names for the effects, by `LedOpt` number. Index 0 is number 1.
const EFFECT_NAMES: [&str; 17] = [
    "Solid",
    "Breathing",
    "Rainbow",
    "Flash",
    "Raindrops",
    "Colour wheel",
    "Ripples",
    "Starlight",
    "Fade",
    "Snake",
    "Neon",
    "Reactive",
    "Sine wave",
    "Scanner",
    "Windmill",
    "Waterfall",
    "Blossom",
];

/// `[OPT]` keys whose meaning is known: the ones stored below, and the ones known to be UI hints
/// or pacing values that the model files don't need. Anything else gets a warning, so a new key in
/// a new RK release, or one nobody has worked out yet, doesn't pass silently.
const KNOWN_KEYS: [&str; 39] = [
    "AccessInter",
    "Channel",
    "CmdReset",
    "DataUnitSize",
    "DefBri",
    "DefLedIndex",
    "DrawFrameTime",
    "FrameViewSp",
    "Fw",
    "GaoshouIndex",
    "GifPID",
    "KbImgUse",
    "LatticeW",
    "LayerNum",
    "LedMask",
    "LightHW",
    "LightUI",
    "MaxScreenFrame",
    "MusicRGBIndex",
    "NoSyncTimeForWireless",
    "PID_Wireless",
    "PackageTime",
    "Psd",
    "RGBIndex",
    "RGBScreen",
    "ScreenCRC",
    "ScreenViewY",
    "SendOnlyDataChanged",
    "SendSysParam",
    "SendTimeOnOpenKB",
    "ShowHourFormat",
    "ShowScreen",
    "ShowSleepTime",
    "SpeedParam",
    "SysParamMask",
    "VID_Wireless",
    "VerNeedToUpdate",
    "Wired",
    "siScreen",
];

/// Known keys that come numbered, as `LedOpt1`, `LedOpt2`...
const KNOWN_NUMBERED_KEYS: [&str; 2] = ["LedOpt", "sf"];

#[derive(Debug)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub max_frames: u32,
    pub kind: u32,
    pub crc: bool,
    pub draw_frame_time: u32,
}

#[derive(Debug)]
pub struct Speed {
    pub min: u8,
    pub max: u8,
    pub default: u8,
}

#[derive(Debug)]
pub struct Effect {
    pub id: u32,
    pub name: String,
    pub speed: bool,
    pub brightness: bool,
    pub rainbow: bool,
    pub color: bool,
    /// Effect byte when it isn't the same as `id`. 0 means "same as `id`".
    pub hw: u32,
}

#[derive(Debug)]
pub struct Lighting {
    pub brightness: Option<Vec<u8>>,
    pub speed: Option<Speed>,
    pub sleep: bool,
    /// Bit `id - 1` set hides that effect.
    pub led_mask: Option<u32>,
    pub effects: Vec<Effect>,
}

#[derive(Debug)]
pub struct Wireless {
    pub vid: u16,
    pub pid: u16,
}

#[derive(Debug, PartialEq)]
pub struct Layer {
    pub kind: u32,
    pub code: u32,
    pub extra: u32,
}

#[derive(Debug)]
pub struct Key {
    pub id: u32,
    pub label: Option<String>,
    pub rect: [u32; 4],
    pub kind: u32,
    pub code: u32,
    pub extra: u32,
    pub matrix: u32,
    /// Columns after the matrix index, on the few keys that have them.
    pub matrix_extra: Vec<u32>,
    pub fn1: Option<Layer>,
    pub fn2: Option<Layer>,
}

#[derive(Debug)]
pub struct Model {
    pub pid: u16,
    pub name: String,
    pub fw: u32,
    pub layout: Option<u16>,
    pub screen: Option<Screen>,
    pub lighting: Lighting,
    pub wireless: Option<Wireless>,
    pub keys: Vec<Key>,
}

/// A model and what the generator has to say about its file.
#[derive(Debug)]
pub struct Parsed {
    pub model: Model,
    /// `[OPT]` keys the generator doesn't recognise.
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

/// Reads `DevNameN=<pid>,<name>` from `Cfg.ini`. The first entry for a PID wins.
pub fn parse_names(cfg: &Ini) -> BTreeMap<u16, String> {
    let mut names = BTreeMap::new();
    let Some(section) = cfg.section("OPT") else {
        return names;
    };
    for entry in section
        .entries
        .iter()
        .filter(|e| e.key.starts_with("DevName"))
    {
        if let Some((pid, name)) = entry.value.split_once(',')
            && let Ok(pid) = u16::from_str_radix(pid.trim(), 16)
        {
            names.entry(pid).or_insert_with(|| name.trim().to_string());
        }
    }
    names
}

/// Name of a model: RK's, without a trailing " Keyboard", or `RK <PID>` if it has none.
pub fn model_name(pid: u16, names: &BTreeMap<u16, String>) -> String {
    match names.get(&pid) {
        Some(name) => name.strip_suffix(" Keyboard").unwrap_or(name).to_string(),
        None => format!("RK {pid:04X}"),
    }
}

pub fn parse_model(pid: u16, name: String, ini: &Ini) -> Result<Parsed, String> {
    let fw = opt_num(ini, "Fw")?.unwrap_or(0);
    if fw > 2 {
        return Err(format!("Fw={fw} isn't a known value"));
    }
    let layout = opt_num(ini, "KbImgUse")?
        .map(|value| to_u16(value, "KbImgUse"))
        .transpose()?;
    let mut warnings = Vec::new();
    // A model with a layout shares the keys of the model it points to.
    let keys = match layout {
        Some(_) => Vec::new(),
        None => parse_keys(ini, &mut warnings)?,
    };
    let model = Model {
        pid,
        name,
        fw,
        layout,
        screen: parse_screen(ini)?,
        lighting: parse_lighting(ini)?,
        wireless: parse_wireless(ini)?,
        keys,
    };
    Ok(Parsed {
        model,
        unknown: unknown_keys(ini),
        warnings,
    })
}

fn opt_num(ini: &Ini, key: &str) -> Result<Option<u32>, String> {
    ini.get("OPT", key)
        .map(|value| parse_num(value).ok_or_else(|| format!("{key}={value} isn't a number")))
        .transpose()
}

fn opt_list(ini: &Ini, key: &str) -> Result<Option<Vec<u32>>, String> {
    ini.get("OPT", key)
        .map(|value| parse_list(value).ok_or_else(|| format!("{key}={value} isn't a list")))
        .transpose()
}

fn to_u16(value: u32, key: &str) -> Result<u16, String> {
    u16::try_from(value).map_err(|_| format!("{key}={value:#x} doesn't fit in 16 bits"))
}

fn parse_screen(ini: &Ini) -> Result<Option<Screen>, String> {
    let kind = opt_num(ini, "ShowScreen")?.unwrap_or(0);
    let size = opt_list(ini, "siScreen")?;
    let max_frames = opt_num(ini, "MaxScreenFrame")?;
    if kind == 0 {
        if size.is_some() || max_frames.is_some() {
            return Err("siScreen or MaxScreenFrame without ShowScreen".to_string());
        }
        return Ok(None);
    }
    let Some(&[width, height]) = size.as_deref() else {
        return Err("ShowScreen without a width,height siScreen".to_string());
    };
    let max_frames = max_frames.ok_or("ShowScreen without MaxScreenFrame")?;
    Ok(Some(Screen {
        width,
        height,
        max_frames,
        kind,
        crc: opt_num(ini, "ScreenCRC")?.unwrap_or(0) != 0,
        draw_frame_time: opt_num(ini, "DrawFrameTime")?.unwrap_or(DEFAULT_DRAW_FRAME_TIME),
    }))
}

fn parse_lighting(ini: &Ini) -> Result<Lighting, String> {
    let brightness = opt_list(ini, "LightHW")?
        .map(|list| {
            list.into_iter()
                .map(|value| {
                    u8::try_from(value).map_err(|_| format!("LightHW value {value} isn't a byte"))
                })
                .collect::<Result<Vec<u8>, String>>()
        })
        .transpose()?;
    // Packed as default<<16 | max<<8 | min.
    let speed = opt_num(ini, "SpeedParam")?.map(|value| {
        let [min, max, default, _] = value.to_le_bytes();
        Speed { min, max, default }
    });
    Ok(Lighting {
        brightness,
        speed,
        // Only a UI switch, and it's on when the file doesn't mention it.
        sleep: opt_num(ini, "ShowSleepTime")?.unwrap_or(1) != 0,
        led_mask: opt_num(ini, "LedMask")?,
        effects: parse_effects(ini)?,
    })
}

/// Reads `LedOpt1`, `LedOpt2`... and stops at the first missing number, like RK's own parser, so
/// whatever comes after a gap is never read.
///
/// Columns: effect byte override (0 = none), speed, brightness, direction, rainbow, colour.
fn parse_effects(ini: &Ini) -> Result<Vec<Effect>, String> {
    let mut effects = Vec::new();
    for id in 1u32.. {
        let key = format!("LedOpt{id}");
        let Some(columns) = opt_list(ini, &key)? else {
            break;
        };
        let &[hw, speed, brightness, _direction, rainbow, color] = columns.as_slice() else {
            return Err(format!("{key} doesn't have 6 columns"));
        };
        let name = EFFECT_NAMES
            .get(id as usize - 1)
            .map_or_else(|| format!("Effect {id}"), |name| name.to_string());
        effects.push(Effect {
            id,
            name,
            speed: speed != 0,
            brightness: brightness != 0,
            rainbow: rainbow != 0,
            color: color != 0,
            hw,
        });
    }
    Ok(effects)
}

fn parse_wireless(ini: &Ini) -> Result<Option<Wireless>, String> {
    let vid = opt_num(ini, "VID_Wireless")?;
    let pid = opt_num(ini, "PID_Wireless")?;
    match (vid, pid) {
        (None, None) => Ok(None),
        (Some(vid), Some(pid)) => Ok(Some(Wireless {
            vid: to_u16(vid, "VID_Wireless")?,
            pid: to_u16(pid, "PID_Wireless")?,
        })),
        _ => Err("VID_Wireless and PID_Wireless must come together".to_string()),
    }
}

/// `K<n>` number of a key entry.
fn key_id(key: &str) -> Option<u32> {
    key.strip_prefix('K')?.parse().ok()
}

fn parse_keys(ini: &Ini, warnings: &mut Vec<String>) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    let Some(section) = ini.section("KEY") else {
        return Ok(keys);
    };
    for entry in &section.entries {
        let Some(id) = key_id(&entry.key) else {
            warnings.push(format!("{} in KEY isn't a key", entry.key));
            continue;
        };
        let columns = parse_list(&entry.value);
        let Some(
            &[
                left,
                top,
                right,
                bottom,
                kind,
                code,
                extra,
                matrix,
                ref matrix_extra @ ..,
            ],
        ) = columns.as_deref()
        else {
            return Err(format!("{}={} isn't a key line", entry.key, entry.value));
        };
        keys.push(Key {
            id,
            label: entry.comment.clone(),
            rect: [left, top, right, bottom],
            kind,
            code,
            extra,
            matrix,
            matrix_extra: matrix_extra.to_vec(),
            fn1: None,
            fn2: None,
        });
    }
    keys.sort_by_key(|key| key.id);
    if let Some(section) = ini.section("FN1") {
        for (index, layer) in parse_layer(section, &keys, warnings)? {
            keys[index].fn1 = Some(layer);
        }
    }
    if let Some(section) = ini.section("FN2") {
        for (index, layer) in parse_layer(section, &keys, warnings)? {
            keys[index].fn2 = Some(layer);
        }
    }
    Ok(keys)
}

/// Reads an FN section. Returns each entry with the position in `keys` of the key it belongs to.
fn parse_layer(
    section: &Section,
    keys: &[Key],
    warnings: &mut Vec<String>,
) -> Result<Vec<(usize, Layer)>, String> {
    let mut layers = Vec::new();
    for entry in &section.entries {
        let columns = parse_list(&entry.value);
        let Some(&[kind, code, extra]) = columns.as_deref() else {
            return Err(format!(
                "{}={} in {} isn't a layer line",
                entry.key, entry.value, section.name
            ));
        };
        let index = key_id(&entry.key).and_then(|id| keys.iter().position(|key| key.id == id));
        match index {
            Some(index) => layers.push((index, Layer { kind, code, extra })),
            None => warnings.push(format!("{} in {} has no key", entry.key, section.name)),
        }
    }
    Ok(layers)
}

fn unknown_keys(ini: &Ini) -> Vec<String> {
    let Some(section) = ini.section("OPT") else {
        return Vec::new();
    };
    let mut unknown = BTreeSet::new();
    for entry in &section.entries {
        let key = entry.key.as_str();
        let base = key.trim_end_matches(|c: char| c.is_ascii_digit());
        if base == key {
            if !KNOWN_KEYS.contains(&key) {
                unknown.insert(key.to_string());
            }
        } else if !KNOWN_NUMBERED_KEYS.contains(&base) {
            unknown.insert(format!("{base}N"));
        }
    }
    unknown.into_iter().collect()
}

/// Checks the whole set. Returns one message per problem.
pub fn validate(models: &[Model]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut by_pid: BTreeMap<u16, &Model> = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for model in models {
        if by_pid.insert(model.pid, model).is_some() {
            duplicates.insert(model.pid);
        }
    }
    for pid in duplicates {
        errors.push(format!("{pid:04X}: duplicate PID"));
    }
    for model in models {
        let pid = model.pid;
        if let Some(layout) = model.layout {
            match by_pid.get(&layout) {
                None => errors.push(format!("{pid:04X}: layout {layout:04X} doesn't exist")),
                Some(target) if target.layout.is_some() => {
                    errors.push(format!("{pid:04X}: layout {layout:04X} has no keys"));
                }
                Some(_) => {}
            }
        }
        if model.screen.is_some() && model.fw != 1 {
            errors.push(format!("{pid:04X}: screen on a Fw={} model", model.fw));
        }
    }
    errors
}

pub fn to_toml(model: &Model) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "pid = 0x{:04X}", model.pid);
    let _ = writeln!(out, "name = {}", quote(&model.name));
    let _ = writeln!(out, "protocol = \"{}\"", protocol(model.fw));
    let _ = writeln!(out, "fw = {}", model.fw);
    out.push_str("status = \"experimental\"\n");
    if let Some(layout) = model.layout {
        let _ = writeln!(out, "layout = \"{layout:04X}\"");
    }
    if let Some(screen) = &model.screen {
        out.push_str("\n[screen]\n");
        let _ = writeln!(out, "width = {}", screen.width);
        let _ = writeln!(out, "height = {}", screen.height);
        let _ = writeln!(out, "max_frames = {}", screen.max_frames);
        let _ = writeln!(out, "kind = {}", screen.kind);
        let _ = writeln!(out, "crc = {}", screen.crc);
        let _ = writeln!(out, "draw_frame_time = {}", screen.draw_frame_time);
    }
    write_lighting(&mut out, &model.lighting);
    if let Some(wireless) = &model.wireless {
        out.push_str("\n[wireless]\n");
        let _ = writeln!(out, "vid = 0x{:04X}", wireless.vid);
        let _ = writeln!(out, "pid = 0x{:04X}", wireless.pid);
    }
    for key in &model.keys {
        write_key(&mut out, key);
    }
    out
}

fn write_lighting(out: &mut String, lighting: &Lighting) {
    out.push_str("\n[lighting]\n");
    if let Some(brightness) = &lighting.brightness {
        let _ = writeln!(out, "brightness = {}", number_list(brightness));
    }
    if let Some(speed) = &lighting.speed {
        let _ = writeln!(
            out,
            "speed = {{ min = {}, max = {}, default = {} }}",
            speed.min, speed.max, speed.default
        );
    }
    let _ = writeln!(out, "sleep = {}", lighting.sleep);
    if let Some(mask) = lighting.led_mask {
        let _ = writeln!(out, "led_mask = 0x{mask:04X}");
    }
    out.push_str("effects = [\n");
    for effect in &lighting.effects {
        let _ = write!(
            out,
            "  {{ id = {}, name = {}, speed = {}, brightness = {}, rainbow = {}, color = {}",
            effect.id,
            quote(&effect.name),
            effect.speed,
            effect.brightness,
            effect.rainbow,
            effect.color
        );
        if effect.hw != 0 {
            let _ = write!(out, ", hw = {}", effect.hw);
        }
        out.push_str(" },\n");
    }
    out.push_str("]\n");
}

fn write_key(out: &mut String, key: &Key) {
    out.push_str("\n[[keys]]\n");
    let _ = writeln!(out, "id = {}", key.id);
    if let Some(label) = &key.label {
        let _ = writeln!(out, "label = {}", quote(label));
    }
    let _ = writeln!(out, "rect = {}", number_list(&key.rect));
    let _ = writeln!(out, "kind = 0x{:02X}", key.kind);
    let _ = writeln!(out, "code = 0x{:02X}", key.code);
    let _ = writeln!(out, "extra = 0x{:08X}", key.extra);
    let _ = writeln!(out, "matrix = {}", key.matrix);
    if !key.matrix_extra.is_empty() {
        let _ = writeln!(out, "matrix_extra = {}", number_list(&key.matrix_extra));
    }
    for (name, layer) in [("fn1", &key.fn1), ("fn2", &key.fn2)] {
        if let Some(layer) = layer {
            let _ = writeln!(
                out,
                "{name} = {{ kind = 0x{:02X}, code = 0x{:02X}, extra = 0x{:08X} }}",
                layer.kind, layer.code, layer.extra
            );
        }
    }
}

fn number_list<T: std::fmt::Display>(values: &[T]) -> String {
    let items: Vec<String> = values.iter().map(T::to_string).collect();
    format!("[{}]", items.join(", "))
}

/// Protocol family a model's `Fw` value selects.
fn protocol(fw: u32) -> &'static str {
    match fw {
        0 => "v1",
        1 => "v2",
        _ => "v2alt",
    }
}

/// Writes a TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LED: &str = "LedOpt1=0,0,1,0,1,1\nLedOpt2=0,1,1,0,0,0\n";

    fn parse(opt: &str) -> Parsed {
        let text = format!("[OPT]\n{opt}\n[KEY]\n;Esc\nK1=1,2,3,4,0x02,0x1B,0x00,0\n");
        parse_model(0x0265, "RK Test".to_string(), &Ini::parse(&text)).unwrap()
    }

    fn model(opt: &str) -> Model {
        parse(opt).model
    }

    #[test]
    fn fw_selects_the_protocol_not_data_unit_size() {
        assert_eq!(model("").fw, 0);
        assert_eq!(protocol(model("").fw), "v1");
        assert_eq!(protocol(model("Fw=0").fw), "v1");
        assert_eq!(protocol(model("DataUnitSize=512").fw), "v1");
        assert_eq!(protocol(model("Fw=1").fw), "v2");
        assert_eq!(protocol(model("Fw=2").fw), "v2alt");
    }

    #[test]
    fn unknown_fw_is_an_error() {
        let ini = Ini::parse("[OPT]\nFw=3\n[KEY]\n");
        let error = parse_model(1, "x".to_string(), &ini).unwrap_err();
        assert!(error.contains("Fw"), "{error}");
    }

    #[test]
    fn screen_fields() {
        let screen = model("Fw=1\nShowScreen=1\nsiScreen=320,172\nMaxScreenFrame=60\n")
            .screen
            .unwrap();
        assert_eq!((screen.width, screen.height), (320, 172));
        assert_eq!(screen.max_frames, 60);
        assert_eq!(screen.kind, 1);
        assert!(!screen.crc);
    }

    #[test]
    fn screen_crc_kind_and_draw_frame_time() {
        let opt = "Fw=1\nShowScreen=2\nsiScreen=240,240\nMaxScreenFrame=35\nScreenCRC=1\n";
        let screen = model(opt).screen.unwrap();
        assert_eq!(screen.kind, 2);
        assert!(screen.crc);
        assert_eq!(screen.draw_frame_time, 25, "default when absent");

        let opt = format!("{opt}DrawFrameTime=180\n");
        assert_eq!(model(&opt).screen.unwrap().draw_frame_time, 180);

        // Zero is a real value, not "absent".
        let opt = "Fw=1\nShowScreen=2\nsiScreen=1,1\nMaxScreenFrame=1\nDrawFrameTime=0\n";
        assert_eq!(model(opt).screen.unwrap().draw_frame_time, 0);
    }

    #[test]
    fn no_show_screen_means_no_screen() {
        assert!(model("Fw=1").screen.is_none());
        assert!(model("Fw=1\nShowScreen=0").screen.is_none());
    }

    #[test]
    fn half_a_screen_is_an_error() {
        for opt in [
            "ShowScreen=1\nsiScreen=320,172",
            "ShowScreen=1\nMaxScreenFrame=60",
            "ShowScreen=1\nsiScreen=320\nMaxScreenFrame=60",
            "siScreen=320,172\nMaxScreenFrame=60",
        ] {
            let ini = Ini::parse(&format!("[OPT]\nFw=1\n{opt}\n[KEY]\n"));
            assert!(parse_model(1, "x".to_string(), &ini).is_err(), "{opt}");
        }
    }

    #[test]
    fn layout_comes_from_kb_img_use() {
        assert_eq!(model("KbImgUse=0x01fa").layout, Some(0x01FA));
        assert_eq!(model("").layout, None);
    }

    #[test]
    fn wireless_ids() {
        let wireless = model("VID_Wireless=0x258a\nPID_Wireless=0x0150")
            .wireless
            .unwrap();
        assert_eq!((wireless.vid, wireless.pid), (0x258A, 0x0150));
        assert!(model("").wireless.is_none());
    }

    #[test]
    fn half_a_wireless_pair_is_an_error() {
        let ini = Ini::parse("[OPT]\nVID_Wireless=0x258a\n[KEY]\n");
        assert!(parse_model(1, "x".to_string(), &ini).is_err());
    }

    #[test]
    fn speed_param_is_default_max_min() {
        let speed = model("SpeedParam=0x010500").lighting.speed.unwrap();
        assert_eq!((speed.min, speed.max, speed.default), (0, 5, 1));
        let speed = model("SpeedParam=0x030402").lighting.speed.unwrap();
        assert_eq!((speed.min, speed.max, speed.default), (2, 4, 3));
        assert!(model("").lighting.speed.is_none());
    }

    #[test]
    fn light_hw_is_the_brightness_list() {
        let lighting = model("LightUI=9,9\nLightHW=0,4,8,12,16,20").lighting;
        assert_eq!(lighting.brightness, Some(vec![0, 4, 8, 12, 16, 20]));
        assert!(model("LightUI=9,9").lighting.brightness.is_none());
    }

    #[test]
    fn sleep_is_on_unless_the_file_turns_it_off() {
        assert!(model("").lighting.sleep);
        assert!(model("ShowSleepTime=1").lighting.sleep);
        assert!(!model("ShowSleepTime=0").lighting.sleep);
    }

    #[test]
    fn led_opt_columns() {
        let effects = model(LED).lighting.effects;
        assert_eq!(effects.len(), 2);
        let first = &effects[0];
        assert_eq!((first.id, first.name.as_str()), (1, "Solid"));
        assert_eq!(
            (first.speed, first.brightness, first.rainbow, first.color),
            (false, true, true, true)
        );
        assert_eq!(first.hw, 0);
        let second = &effects[1];
        assert_eq!((second.id, second.name.as_str()), (2, "Breathing"));
        assert_eq!(
            (
                second.speed,
                second.brightness,
                second.rainbow,
                second.color
            ),
            (true, true, false, false)
        );
    }

    #[test]
    fn each_column_maps_to_its_own_field() {
        let effect = |opt: &str| {
            let mut effects = model(&format!("LedOpt1={opt}")).lighting.effects;
            effects.remove(0)
        };
        assert!(effect("0,1,0,0,0,0").speed);
        assert!(effect("0,0,1,0,0,0").brightness);
        assert!(effect("0,0,0,0,1,0").rainbow);
        assert!(effect("0,0,0,0,0,1").color);
        // Direction has no effect on any stored field.
        let direction = effect("0,0,0,1,0,0");
        assert!(
            !direction.speed && !direction.brightness && !direction.rainbow && !direction.color
        );
        assert_eq!(effect("7,0,0,0,0,0").hw, 7);
    }

    #[test]
    fn led_opt_stops_at_the_first_missing_key() {
        let opt = "LedOpt1=0,0,1,0,1,1\nLedOpt2=0,0,1,0,1,1\nLedOpt4=0,0,1,0,1,1\n";
        let ids: Vec<u32> = model(opt).lighting.effects.iter().map(|e| e.id).collect();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn led_opt_tolerates_spaces_and_inline_comments() {
        let effects = model("LedOpt1=  0,1,1,0,1,1 ;note").lighting.effects;
        assert_eq!(effects.len(), 1);
        assert!(effects[0].color);
    }

    #[test]
    fn short_led_opt_is_an_error() {
        let ini = Ini::parse("[OPT]\nLedOpt1=0,1,1\n[KEY]\n");
        assert!(parse_model(1, "x".to_string(), &ini).is_err());
    }

    #[test]
    fn effect_names_are_ours_and_fall_back_to_the_number() {
        let opt: String = (1..=19)
            .map(|n| format!("LedOpt{n}=0,0,0,0,0,0\n"))
            .collect();
        let effects = model(&opt).lighting.effects;
        assert_eq!(effects[9].name, "Snake");
        assert_eq!(effects[16].name, "Blossom");
        assert_eq!(effects[17].name, "Effect 18");
        assert_eq!(effects[18].name, "Effect 19");
    }

    #[test]
    fn led_mask_is_kept_as_it_is() {
        assert_eq!(model("LedMask=0x2100").lighting.led_mask, Some(0x2100));
        assert_eq!(model("").lighting.led_mask, None);
    }

    #[test]
    fn unknown_opt_keys_are_collected() {
        let parsed = parse("Fw=1\nBrandNewKey=3\nOtherNew2=1\nLedOpt1=0,0,0,0,0,0\nPackageTime=12");
        assert_eq!(parsed.unknown, ["BrandNewKey", "OtherNewN"]);
    }

    #[test]
    fn key_line_with_type_code_extra_and_matrix() {
        let text = "[OPT]\n[KEY]\nK87=0,0,0,0,0x09,0x01,0x0700000D,84\n";
        let parsed = parse_model(1, "x".to_string(), &Ini::parse(text)).unwrap();
        let key = &parsed.model.keys[0];
        assert_eq!(key.id, 87);
        assert_eq!(key.rect, [0, 0, 0, 0]);
        assert_eq!((key.kind, key.code, key.extra), (9, 1, 0x0700_000D));
        assert_eq!(key.matrix, 84);
        assert!(key.matrix_extra.is_empty());
    }

    #[test]
    fn key_line_with_spaces_and_label() {
        let key = &model("").keys[0];
        assert_eq!(key.id, 1);
        assert_eq!(key.label.as_deref(), Some("Esc"));
        assert_eq!(key.rect, [1, 2, 3, 4]);
        assert_eq!((key.kind, key.code, key.extra, key.matrix), (2, 0x1B, 0, 0));
    }

    #[test]
    fn key_line_with_more_matrix_columns() {
        let text = "[OPT]\n[KEY]\nK59=145,187,167,209, 0x02,0x5A,0x00,10,10,16\n";
        let parsed = parse_model(1, "x".to_string(), &Ini::parse(text)).unwrap();
        let key = &parsed.model.keys[0];
        assert_eq!(key.matrix, 10);
        assert_eq!(key.matrix_extra, [10, 16]);
    }

    #[test]
    fn short_key_line_is_an_error() {
        let ini = Ini::parse("[OPT]\n[KEY]\nK1=1,2,3\n");
        let error = parse_model(1, "x".to_string(), &ini).unwrap_err();
        assert!(error.contains("K1"), "{error}");
    }

    #[test]
    fn keys_are_sorted_by_number() {
        let text = "[OPT]\n[KEY]\nK10=0,0,0,0,2,1,0,0\nK2=0,0,0,0,2,1,0,0\n";
        let parsed = parse_model(1, "x".to_string(), &Ini::parse(text)).unwrap();
        let ids: Vec<u32> = parsed.model.keys.iter().map(|k| k.id).collect();
        assert_eq!(ids, [2, 10]);
    }

    #[test]
    fn fn_layers_attach_to_their_key() {
        let text = "[OPT]\n[KEY]\nK1=0,0,0,0,2,0x1B,0,0\nK2=0,0,0,0,2,0x70,0,1\n\
                    [FN1]\n;F1\nK2=0x09,0x01,0x02000194\n[FN2]\nK1=0x02,0x91,0x00,\n";
        let parsed = parse_model(1, "x".to_string(), &Ini::parse(text)).unwrap();
        let keys = &parsed.model.keys;
        assert_eq!(keys[0].fn1, None);
        assert_eq!(
            keys[1].fn1,
            Some(Layer {
                kind: 9,
                code: 1,
                extra: 0x0200_0194
            })
        );
        assert_eq!(
            keys[0].fn2,
            Some(Layer {
                kind: 2,
                code: 0x91,
                extra: 0
            })
        );
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn fn_entry_without_a_key_is_a_warning() {
        let text = "[OPT]\n[KEY]\n[FN1]\nK9=2,1,0\n";
        let parsed = parse_model(1, "x".to_string(), &Ini::parse(text)).unwrap();
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].contains("K9"));
    }

    #[test]
    fn models_with_a_layout_store_no_keys() {
        assert!(model("KbImgUse=0x01fa").keys.is_empty());
        assert_eq!(model("").keys.len(), 1);
    }

    #[test]
    fn names_are_read_from_cfg_with_any_pid_case() {
        let cfg = Ini::parse(
            "[OPT]\nTitle=x\nDevName1=005d,RK857 Keyboard\nDevName2=01FA,RK S85 ISO\n\
             DevName3=01fa,Second\n",
        );
        let names = parse_names(&cfg);
        assert_eq!(names[&0x005D], "RK857 Keyboard");
        assert_eq!(names[&0x01FA], "RK S85 ISO");
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn only_a_trailing_keyboard_is_dropped_from_names() {
        let names = BTreeMap::from([
            (1, "RK S85 ES Keyboard".to_string()),
            (2, "RK68 N ISO Return Keyboard Bluetooth".to_string()),
            (3, "RK868RGB Wired keyboard".to_string()),
            (4, "RK-S98RGB-New".to_string()),
        ]);
        assert_eq!(model_name(1, &names), "RK S85 ES");
        assert_eq!(
            model_name(2, &names),
            "RK68 N ISO Return Keyboard Bluetooth"
        );
        assert_eq!(model_name(3, &names), "RK868RGB Wired keyboard");
        assert_eq!(model_name(4, &names), "RK-S98RGB-New");
    }

    #[test]
    fn model_without_a_name_uses_its_pid() {
        assert_eq!(model_name(0x01FA, &BTreeMap::new()), "RK 01FA");
    }

    fn stub(pid: u16, fw: u32, layout: Option<u16>) -> Model {
        let mut model = model(&format!("Fw={fw}"));
        model.pid = pid;
        model.layout = layout;
        model
    }

    #[test]
    fn valid_set_has_no_errors() {
        // v2 without a screen is fine: only some v2 models have one.
        let models = [stub(1, 1, None), stub(2, 0, Some(1)), stub(3, 2, Some(1))];
        assert!(validate(&models).is_empty());
    }

    #[test]
    fn duplicate_pid_is_an_error() {
        let errors = validate(&[stub(0x0265, 1, None), stub(0x0265, 1, None)]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("0265"), "{}", errors[0]);
    }

    #[test]
    fn layout_to_a_missing_model_is_an_error() {
        let errors = validate(&[stub(1, 1, Some(0x01FA))]);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("01FA"), "{}", errors[0]);
    }

    #[test]
    fn layout_to_a_model_without_keys_is_an_error() {
        let errors = validate(&[stub(1, 1, Some(2)), stub(2, 1, Some(3)), stub(3, 1, None)]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("0001"), "{}", errors[0]);
    }

    #[test]
    fn screen_outside_fw_1_is_an_error() {
        let opt = "Fw=2\nShowScreen=1\nsiScreen=1,1\nMaxScreenFrame=1";
        let errors = validate(&[model(opt)]);
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn toml_for_a_model_with_a_layout() {
        let opt = "Fw=1\nKbImgUse=0x01fa\nShowScreen=1\nsiScreen=320,172\nMaxScreenFrame=60\n\
                   DrawFrameTime=180\nLightHW=0,4\nSpeedParam=0x010500\nLedOpt1=0,0,1,0,1,1\n\
                   VID_Wireless=0x258a\nPID_Wireless=0x0150\n";
        let mut model = model(opt);
        model.name = "RK S85 ES".to_string();
        let expected = r#"pid = 0x0265
name = "RK S85 ES"
protocol = "v2"
fw = 1
status = "experimental"
layout = "01FA"

[screen]
width = 320
height = 172
max_frames = 60
kind = 1
crc = false
draw_frame_time = 180

[lighting]
brightness = [0, 4]
speed = { min = 0, max = 5, default = 1 }
sleep = true
effects = [
  { id = 1, name = "Solid", speed = false, brightness = true, rainbow = true, color = true },
]

[wireless]
vid = 0x258A
pid = 0x0150
"#;
        assert_eq!(to_toml(&model), expected);
    }

    #[test]
    fn toml_for_a_model_with_keys() {
        let text = "[OPT]\nLedMask=0x2100\nLedOpt1=3,1,0,0,0,0\n\
                    [KEY]\n;\\\nK2=1,2,3,4,0x02,0xDC,0x00,7\n;\"\nK3=0,0,0,0,0x09,0x01,0x0700000D,8,8,9\n\
                    K4=0,0,0,0,0x02,0x20,0x00,1\n[FN1]\nK2=0x09,0x01,0x02000194\n[FN2]\nK2=0x02,0x91,0x00\n";
        let parsed = parse_model(0x00AB, "RK \"Q\"".to_string(), &Ini::parse(text)).unwrap();
        let expected = r#"pid = 0x00AB
name = "RK \"Q\""
protocol = "v1"
fw = 0
status = "experimental"

[lighting]
sleep = true
led_mask = 0x2100
effects = [
  { id = 1, name = "Solid", speed = true, brightness = false, rainbow = false, color = false, hw = 3 },
]

[[keys]]
id = 2
label = "\\"
rect = [1, 2, 3, 4]
kind = 0x02
code = 0xDC
extra = 0x00000000
matrix = 7
fn1 = { kind = 0x09, code = 0x01, extra = 0x02000194 }
fn2 = { kind = 0x02, code = 0x91, extra = 0x00000000 }

[[keys]]
id = 3
label = "\""
rect = [0, 0, 0, 0]
kind = 0x09
code = 0x01
extra = 0x0700000D
matrix = 8
matrix_extra = [8, 9]

[[keys]]
id = 4
rect = [0, 0, 0, 0]
kind = 0x02
code = 0x20
extra = 0x00000000
matrix = 1
"#;
        assert_eq!(to_toml(&parsed.model), expected);
    }

    #[test]
    fn toml_escapes_control_characters() {
        assert_eq!(quote("a\tb\u{1}"), "\"a\\tb\\u0001\"");
    }
}
