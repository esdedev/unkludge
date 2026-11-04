//! The keyboard models, compiled in from `data/models/*.toml`.

/// USB vendor ID shared by all the models.
pub const VID: u16 = 0x258A;

/// Which wire protocol and config layout a model uses, decided by its firmware family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// 65-byte reports. Not supported.
    V1,
    /// 520-byte reports and the config layout this crate knows.
    V2,
    /// 520-byte reports with a different config layout. Not supported.
    V2Alt,
}

/// How far a model has been tried on real hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Tested,
    Experimental,
}

/// A model's screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenSpec {
    pub width: u16,
    pub height: u16,
    pub max_frames: u16,
    /// 1: one HID collection carries everything. 2: screen data uses a second collection.
    pub kind: u8,
    /// Chunks carry a checksum, which the keyboard's ack repeats.
    pub crc: bool,
    /// A time per frame in milliseconds, from the model data. Changes the frame count and start
    /// delay sent. What it means to the firmware is unconfirmed.
    pub draw_frame_time: u16,
}

impl ScreenSpec {
    /// Whether screen data goes to a second HID collection.
    pub fn has_second_collection(&self) -> bool {
        self.kind == 2
    }
}

/// A lighting effect, and which controls it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effect {
    /// Effect number, written to the config block.
    pub id: u8,
    pub name: &'static str,
    pub color: bool,
    pub speed: bool,
    pub brightness: bool,
    pub rainbow: bool,
}

/// A model's lighting controls. `brightness` is empty and the speeds are zero on models whose
/// data doesn't list them, all of which are unsupported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lighting {
    /// The brightness bytes that can be written, dimmest first.
    pub brightness: &'static [u8],
    pub speed_min: u8,
    pub speed_max: u8,
    pub speed_default: u8,
    pub effects: &'static [Effect],
    pub sleep: bool,
}

/// One keyboard model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub pid: u16,
    pub name: &'static str,
    pub protocol: Protocol,
    pub status: Status,
    pub screen: Option<ScreenSpec>,
    pub lighting: Lighting,
    /// PID of the model whose key layout this one shares.
    pub layout: Option<u16>,
}

include!(concat!(env!("OUT_DIR"), "/models.rs"));

/// The model with this USB product ID, supported or not.
pub fn find(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// The models this version can configure: the `V2` ones with a screen.
pub fn supported() -> impl Iterator<Item = &'static Model> {
    MODELS.iter().filter(|m| m.is_supported())
}

impl Model {
    /// Whether this version can configure the model.
    pub fn is_supported(&self) -> bool {
        self.protocol == Protocol::V2 && self.screen.is_some()
    }

    /// Whether screen data goes to a second HID collection.
    pub fn has_second_collection(&self) -> bool {
        self.screen.is_some_and(|s| s.has_second_collection())
    }
}

#[cfg(test)]
mod tests {
    /// The udev rule grants access to these models and no others, so it has to agree with the table.
    #[test]
    fn the_udev_rule_lists_the_supported_models_and_no_others() {
        let rule = include_str!("../../../packaging/linux/70-unkludge.rules");
        let mut in_rule: Vec<String> = rule
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| {
                line.strip_prefix(
                    r#"KERNEL=="hidraw*", ATTRS{idVendor}=="258a", ATTRS{idProduct}==""#,
                )
                .and_then(|rest| rest.strip_suffix(r#"", TAG+="uaccess""#))
                .unwrap_or_else(|| panic!("a line of another shape: {line}"))
                .to_string()
            })
            .collect();
        in_rule.sort();
        let mut in_table: Vec<String> = super::supported()
            .map(|model| format!("{:04x}", model.pid))
            .collect();
        in_table.sort();
        assert_eq!(in_rule, in_table);
    }

    use super::*;

    #[test]
    fn all_218_models_are_in_the_table() {
        assert_eq!(MODELS.len(), 218);
    }

    #[test]
    fn pids_are_unique() {
        let mut pids: Vec<u16> = MODELS.iter().map(|m| m.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        assert_eq!(pids.len(), MODELS.len());
    }

    #[test]
    fn layouts_point_to_models_that_exist() {
        for model in MODELS {
            if let Some(layout) = model.layout {
                assert!(find(layout).is_some(), "{:04X} -> {layout:04X}", model.pid);
            }
        }
    }

    #[test]
    fn there_are_29_supported_models_all_v2_with_a_screen() {
        let s: Vec<_> = supported().collect();
        assert_eq!(s.len(), 29);
        assert!(
            s.iter()
                .all(|m| m.protocol == Protocol::V2 && m.screen.is_some())
        );
    }

    #[test]
    fn s85_es_values() {
        let m = find(0x0265).unwrap();
        assert_eq!(m.name, "RK S85 ES");
        assert_eq!(m.protocol, Protocol::V2);
        assert_eq!(m.status, Status::Experimental);
        assert_eq!(m.layout, Some(0x01FA));
        let s = m.screen.unwrap();
        assert_eq!((s.width, s.height, s.max_frames), (320, 172, 60));
        assert_eq!((s.kind, s.crc, s.draw_frame_time), (1, false, 180));
    }

    #[test]
    fn s85_es_lighting() {
        let l = find(0x0265).unwrap().lighting;
        assert_eq!(l.brightness, &[0, 4, 8, 12, 16, 20]);
        assert_eq!((l.speed_min, l.speed_max, l.speed_default), (0, 5, 1));
        assert!(l.sleep);
        assert_eq!(l.effects.len(), 17);
        let solid = l.effects[0];
        assert_eq!((solid.id, solid.name), (1, "Solid"));
        assert_eq!(
            (solid.speed, solid.brightness, solid.rainbow, solid.color),
            (false, true, true, true)
        );
        let blossom = l.effects[16];
        assert_eq!((blossom.id, blossom.name), (17, "Blossom"));
        assert_eq!(
            (
                blossom.speed,
                blossom.brightness,
                blossom.rainbow,
                blossom.color
            ),
            (true, true, false, false)
        );
    }

    #[test]
    fn m87_uses_the_chunk_checksum() {
        let s = find(0x01A2).unwrap().screen.unwrap();
        assert_eq!((s.width, s.height, s.max_frames), (240, 135, 100));
        assert_eq!((s.kind, s.crc, s.draw_frame_time), (1, true, 25));
    }

    #[test]
    fn n99_family_has_a_second_collection() {
        let kind_2: Vec<u16> = supported()
            .filter(|m| m.has_second_collection())
            .map(|m| m.pid)
            .collect();
        assert_eq!(kind_2, [0x01B8, 0x025E, 0x025F]);
        assert_eq!(find(0x01B8).unwrap().screen.unwrap().draw_frame_time, 0);
    }

    #[test]
    fn checksum_models_are_the_m87_family_only() {
        let crc: Vec<u16> = supported()
            .filter(|m| m.screen.unwrap().crc)
            .map(|m| m.pid)
            .collect();
        assert_eq!(
            crc,
            [
                0x01A2, 0x01D6, 0x01F5, 0x0272, 0x0273, 0x0274, 0x0275, 0x0276
            ]
        );
    }

    #[test]
    fn unsupported_models_stay_in_the_table() {
        assert_eq!(
            MODELS.iter().filter(|m| m.protocol == Protocol::V1).count(),
            127
        );
        assert_eq!(
            MODELS
                .iter()
                .filter(|m| m.protocol == Protocol::V2Alt)
                .count(),
            12
        );
        let no_screen = MODELS
            .iter()
            .filter(|m| m.protocol == Protocol::V2 && m.screen.is_none())
            .count();
        assert_eq!(no_screen, 50);
        let v1 = MODELS.iter().find(|m| m.protocol == Protocol::V1).unwrap();
        assert!(!v1.is_supported());
        assert!(v1.lighting.brightness.is_empty());
        assert_eq!(v1.lighting.speed_max, 0);
    }

    #[test]
    fn find_returns_none_for_an_unknown_pid() {
        assert!(find(0xFFFF).is_none());
    }
}
