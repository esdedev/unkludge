//! Lighting settings on top of the config block, and the effect palettes.

use std::time::Duration;

use crate::config::{Config, SleepTime};
use crate::model::Lighting;
use crate::packet::cmd;
use crate::transport::Transport;
use crate::{Device, Error};

/// Size of the palette block: 20 entries, one per effect number, of 7 colours of 3 bytes.
pub const PALETTES_LEN: usize = ENTRIES * ENTRY_LEN;
/// How many colours each effect's palette holds.
pub const PALETTE_SLOTS: u8 = 7;

const ENTRIES: usize = 20;
const ENTRY_LEN: usize = PALETTE_SLOTS as usize * 3;

/// The slot a custom colour goes into.
const CUSTOM_COLOR_SLOT: u8 = 0;
/// Wait between writing the config and the palettes, as the official app does.
const PALETTES_DELAY: Duration = Duration::from_millis(100);

/// A colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }
}

/// The colours of the effects.
///
/// The keyboard stores these but can't report them, and they can only be written all together. So
/// the caller keeps the copy, and whatever is in it replaces what's on the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palettes {
    bytes: [u8; PALETTES_LEN],
}

impl Default for Palettes {
    /// Every effect gets red, green, blue, yellow, magenta, cyan and white.
    fn default() -> Self {
        const COLORS: [u8; ENTRY_LEN] = [
            0xFF, 0x00, 0x00, // red
            0x00, 0xFF, 0x00, // green
            0x00, 0x00, 0xFF, // blue
            0xFF, 0xFF, 0x00, // yellow
            0xFF, 0x00, 0xFF, // magenta
            0x00, 0xFF, 0xFF, // cyan
            0xFF, 0xFF, 0xFF, // white
        ];
        let mut bytes = [0; PALETTES_LEN];
        for entry in bytes.chunks_mut(ENTRY_LEN).skip(1) {
            entry.copy_from_slice(&COLORS);
        }
        Palettes { bytes }
    }
}

impl Palettes {
    /// Checks the length, and that entry 0 is all zero as the keyboard expects.
    pub fn from_bytes(bytes: &[u8]) -> Result<Palettes, Error> {
        let bytes = <[u8; PALETTES_LEN]>::try_from(bytes).map_err(|_| {
            Error::Invalid(format!(
                "palettes have {PALETTES_LEN} bytes, not {}",
                bytes.len()
            ))
        })?;
        if bytes[..ENTRY_LEN].iter().any(|&b| b != 0) {
            return Err(Error::Invalid(
                "the first palette entry must be all zero".to_string(),
            ));
        }
        Ok(Palettes { bytes })
    }

    /// The block written to the keyboard: `R G B` for each slot of each entry.
    pub fn as_bytes(&self) -> &[u8; PALETTES_LEN] {
        &self.bytes
    }

    /// A colour of an effect's palette, or `None` if there is no such slot.
    pub fn get(&self, effect: u8, slot: u8) -> Option<Rgb> {
        let at = slot_offset(effect, slot)?;
        Some(Rgb::new(
            self.bytes[at],
            self.bytes[at + 1],
            self.bytes[at + 2],
        ))
    }

    /// Changes a colour of an effect's palette. Effects start at 1, and slots run from 0 to 6.
    pub fn set(&mut self, effect: u8, slot: u8, color: Rgb) -> Result<(), Error> {
        let at = slot_offset(effect, slot)
            .ok_or_else(|| Error::Invalid(format!("no palette slot {slot} for effect {effect}")))?;
        self.bytes[at..at + 3].copy_from_slice(&[color.r, color.g, color.b]);
        Ok(())
    }
}

/// Where a colour is in the palette block. Entry 0 belongs to no effect.
fn slot_offset(effect: u8, slot: u8) -> Option<usize> {
    let entry = usize::from(effect);
    ((1..ENTRIES).contains(&entry) && slot < PALETTE_SLOTS)
        .then(|| entry * ENTRY_LEN + usize::from(slot) * 3)
}

/// What the lights are doing, as read from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightMode {
    Off,
    /// An effect from the model's table.
    Effect(u8),
    /// The custom mode, set with another tool. This version can't change it, only leave it.
    Custom,
    /// An effect byte that isn't in the model's table.
    Unknown(u8),
}

/// The colour an effect uses, as read from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSetting {
    Rainbow,
    /// A slot of the effect's palette, 0 to 6.
    Slot(u8),
    /// A colour value this crate doesn't know the meaning of.
    Unknown(u8),
}

/// The sleep time, as read from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepSetting {
    Known(SleepTime),
    /// A byte that isn't one of the options. Changing other settings keeps it as it is.
    Unknown(u8),
}

/// The lighting settings of a keyboard. `brightness`, `speed` and `color` belong to the current
/// effect, and are `None` when there isn't one or it doesn't have that control. The values are the
/// keyboard's own, so they may not be ones the model allows setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentLighting {
    pub mode: LightMode,
    pub brightness: Option<u8>,
    pub speed: Option<u8>,
    pub color: Option<ColorSetting>,
    pub sleep: SleepSetting,
}

impl CurrentLighting {
    /// Reads the settings out of a config block.
    pub fn from_config(config: &Config, lighting: &Lighting) -> CurrentLighting {
        let sleep = SleepTime::from_byte(config.sleep())
            .map_or(SleepSetting::Unknown(config.sleep()), SleepSetting::Known);
        let mut current = CurrentLighting {
            mode: LightMode::Unknown(config.effect()),
            brightness: None,
            speed: None,
            color: None,
            sleep,
        };
        if config.is_custom() {
            current.mode = LightMode::Custom;
        } else if config.effect() == 0 {
            current.mode = LightMode::Off;
        } else if let Some(effect) = lighting.effects.iter().find(|e| e.id == config.effect()) {
            current.mode = LightMode::Effect(effect.id);
            current.brightness = config.brightness(effect.id).filter(|_| effect.brightness);
            current.speed = config.speed(effect.id).filter(|_| effect.speed);
            current.color = config
                .color(effect.id)
                .filter(|_| effect.color || effect.rainbow)
                .map(color_setting);
        }
        current
    }
}

fn color_setting(value: u8) -> ColorSetting {
    match value {
        7 => ColorSetting::Rainbow,
        slot if slot < PALETTE_SLOTS => ColorSetting::Slot(slot),
        other => ColorSetting::Unknown(other),
    }
}

/// An effect to select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeChoice {
    Off,
    /// An effect from the model's table, by number.
    Effect(u8),
}

/// A colour to give an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    Rainbow,
    /// A slot of the effect's palette, 0 to 6.
    Slot(u8),
    /// A colour of your own. It goes into slot 0 of the effect's palette, and the effect uses it.
    Custom(Rgb),
}

/// Settings to change, leaving anything that is `None` as it is. `brightness`, `speed` and `color`
/// apply to the effect in `mode`, or to the current effect if `mode` is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightingChange {
    pub mode: Option<ModeChoice>,
    /// One of the bytes in the model's [`Lighting::brightness`].
    pub brightness: Option<u8>,
    pub speed: Option<u8>,
    pub color: Option<ColorChoice>,
    pub sleep: Option<SleepTime>,
}

/// Reads the current settings.
pub fn read<T: Transport>(device: &mut Device<T>) -> Result<CurrentLighting, Error> {
    let lighting = &device.model().lighting;
    Ok(CurrentLighting::from_config(
        &Config::read(device)?,
        lighting,
    ))
}

/// Changes lighting settings and returns the settings that result.
///
/// The config is read first and only the fields given change, so nothing is written if the read
/// fails or the model doesn't allow the change ([`Error::Invalid`]). If nothing would change in it,
/// the config isn't written.
///
/// A [`ColorChoice::Custom`] also writes `palettes`, always, as the keyboard can't report what it
/// holds. `palettes` is updated once that write has worked.
pub fn apply<T: Transport>(
    device: &mut Device<T>,
    change: &LightingChange,
    palettes: &mut Palettes,
) -> Result<CurrentLighting, Error> {
    let lighting = &device.model().lighting;
    let before = Config::read(device)?;
    let (after, new_palettes) = edit(&before, lighting, change, palettes)?;

    if after != before {
        after.write(device)?;
    }
    if let Some(new_palettes) = new_palettes {
        device.sleep(PALETTES_DELAY);
        device.send_generic(cmd::PALETTES, 0, new_palettes.as_bytes())?;
        *palettes = new_palettes;
    }
    Ok(CurrentLighting::from_config(&after, lighting))
}

/// The config and, for a custom colour, the palettes that a change results in.
fn edit(
    config: &Config,
    lighting: &Lighting,
    change: &LightingChange,
    palettes: &Palettes,
) -> Result<(Config, Option<Palettes>), Error> {
    let mut config = config.clone();
    let mut new_palettes = None;

    match change.mode {
        Some(ModeChoice::Off) => config.set_effect(lighting, 0)?,
        // Number 0 would mean off to the config block, but isn't what was asked for.
        Some(ModeChoice::Effect(0)) => {
            return Err(Error::Invalid("effect numbers start at 1".to_string()));
        }
        Some(ModeChoice::Effect(id)) => config.set_effect(lighting, id)?,
        None => {}
    }
    if change.brightness.is_some() || change.speed.is_some() || change.color.is_some() {
        let effect = adjustable_effect(&config)?;
        if let Some(value) = change.brightness {
            config.set_brightness(lighting, effect, value)?;
        }
        if let Some(value) = change.speed {
            config.set_speed(lighting, effect, value)?;
        }
        match change.color {
            Some(ColorChoice::Rainbow) => config.set_rainbow(lighting, effect)?,
            Some(ColorChoice::Slot(slot)) => config.set_color(lighting, effect, slot)?,
            Some(ColorChoice::Custom(rgb)) => {
                config.set_color(lighting, effect, CUSTOM_COLOR_SLOT)?;
                let mut changed = palettes.clone();
                changed.set(effect, CUSTOM_COLOR_SLOT, rgb)?;
                new_palettes = Some(changed);
            }
            None => {}
        }
    }
    if let Some(sleep) = change.sleep {
        config.set_sleep(lighting, sleep)?;
    }
    Ok((config, new_palettes))
}

/// The effect whose settings can be changed, if the lights are showing one.
fn adjustable_effect(config: &Config) -> Result<u8, Error> {
    if config.is_custom() {
        return Err(Error::Invalid(
            "the keyboard is in the custom mode: choose an effect first".to_string(),
        ));
    }
    match config.effect() {
        0 => Err(Error::Invalid(
            "the lights are off: choose an effect first".to_string(),
        )),
        effect => Ok(effect),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CONFIG_LEN, config_reply};
    use crate::model::{self, Model};
    use crate::transport::fake::FakeTransport;
    use std::time::Duration;

    const RED: Rgb = Rgb::new(255, 0, 0);
    const PINK: Rgb = Rgb::new(0xFF, 0x10, 0x80);

    fn s85() -> &'static Model {
        model::find(0x0265).unwrap()
    }

    fn device(fake: &FakeTransport) -> Device<FakeTransport> {
        Device::new(fake.clone(), None, s85()).unwrap()
    }

    /// A block with something in every byte, showing effect 5 at brightness 8, speed 2 and slot 3.
    fn block() -> [u8; CONFIG_LEN] {
        let mut bytes = [0x77; CONFIG_LEN];
        bytes[0x09] = 0;
        bytes[0x0A] = 5;
        bytes[0x18] = 0x14;
        bytes[0x38 + 10] = 8;
        bytes[0x39 + 10] = 0x23;
        bytes[0x7E..].copy_from_slice(&[0x5A, 0xA5]);
        bytes
    }

    fn fake_with(block: &[u8; CONFIG_LEN]) -> FakeTransport {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&config_reply(block));
        fake
    }

    /// The command byte of each report sent.
    fn commands(fake: &FakeTransport) -> Vec<u8> {
        fake.sent().iter().map(|r| r[1]).collect()
    }

    /// The config block in the report that wrote it.
    fn written_config(fake: &FakeTransport) -> Vec<u8> {
        let sent = fake.sent();
        let report = sent.iter().find(|r| r[1] == 0x04).expect("a config write");
        report[8..8 + CONFIG_LEN].to_vec()
    }

    fn apply_to(
        block: &[u8; CONFIG_LEN],
        change: &LightingChange,
    ) -> (FakeTransport, Result<CurrentLighting, Error>) {
        let fake = fake_with(block);
        let result = apply(&mut device(&fake), change, &mut Palettes::default());
        (fake, result)
    }

    /// Checks that a change is refused with nothing written.
    #[track_caller]
    fn assert_refused(block: &[u8; CONFIG_LEN], change: &LightingChange) {
        let (fake, result) = apply_to(block, change);
        assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
        assert_eq!(commands(&fake), [0x84]);
    }

    fn effect(id: u8) -> LightingChange {
        LightingChange {
            mode: Some(ModeChoice::Effect(id)),
            ..LightingChange::default()
        }
    }

    // Palettes.

    #[test]
    fn default_palettes_have_20_entries_of_7_colours() {
        let palettes = Palettes::default();
        let bytes = palettes.as_bytes();
        assert_eq!(bytes.len(), 420);
        let entry: [u8; 21] = [
            255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255,
        ];
        assert_eq!(&bytes[..21], &[0; 21], "entry 0 stays all zero");
        for chunk in bytes[21..].chunks(21) {
            assert_eq!(chunk, entry);
        }
    }

    #[test]
    fn set_writes_red_green_blue_at_the_effects_entry() {
        let mut palettes = Palettes::default();
        let before = palettes.clone();
        palettes.set(4, 2, Rgb::new(1, 2, 3)).unwrap();
        let at = 4 * 21 + 2 * 3;
        assert_eq!(&palettes.as_bytes()[at..at + 3], &[1, 2, 3]);
        assert_eq!(palettes.get(4, 2), Some(Rgb::new(1, 2, 3)));

        let changed = (0..420)
            .filter(|&i| palettes.as_bytes()[i] != before.as_bytes()[i])
            .count();
        assert_eq!(changed, 3);
    }

    #[test]
    fn set_refuses_entry_0_and_anything_out_of_range() {
        let mut palettes = Palettes::default();
        for (effect, slot) in [(0, 0), (20, 0), (1, 7)] {
            assert!(matches!(
                palettes.set(effect, slot, RED),
                Err(Error::Invalid(_))
            ));
            assert_eq!(palettes.get(effect, slot), None);
        }
        assert_eq!(palettes, Palettes::default());
    }

    #[test]
    fn palettes_round_trip_through_bytes() {
        let mut palettes = Palettes::default();
        palettes.set(19, 6, PINK).unwrap();
        assert_eq!(Palettes::from_bytes(palettes.as_bytes()), Ok(palettes));
    }

    #[test]
    fn from_bytes_refuses_a_wrong_length_or_a_used_entry_0() {
        assert!(matches!(
            Palettes::from_bytes(&[0; 419]),
            Err(Error::Invalid(_))
        ));
        let mut bytes = [0; 420];
        bytes[20] = 1;
        assert!(matches!(
            Palettes::from_bytes(&bytes),
            Err(Error::Invalid(_))
        ));
    }

    // Reading.

    #[test]
    fn read_gives_the_settings_of_the_current_effect() {
        let fake = fake_with(&block());
        assert_eq!(
            read(&mut device(&fake)),
            Ok(CurrentLighting {
                mode: LightMode::Effect(5),
                brightness: Some(8),
                speed: Some(2),
                color: Some(ColorSetting::Slot(3)),
                sleep: SleepSetting::Known(SleepTime::Min10),
            })
        );
        assert_eq!(commands(&fake), [0x84]);
    }

    fn current(block: &[u8; CONFIG_LEN]) -> CurrentLighting {
        let config = Config::from_bytes(block).unwrap();
        CurrentLighting::from_config(&config, &s85().lighting)
    }

    #[test]
    fn a_rainbow_colour_and_an_unknown_one_are_told_apart() {
        let mut bytes = block();
        bytes[0x39 + 10] = 0x27;
        assert_eq!(current(&bytes).color, Some(ColorSetting::Rainbow));
        bytes[0x39 + 10] = 0x2F;
        assert_eq!(current(&bytes).color, Some(ColorSetting::Unknown(15)));
    }

    #[test]
    fn controls_the_effect_doesnt_have_read_as_none() {
        let mut bytes = block();
        // Effect 1 has no speed, and effect 3 no colour.
        bytes[0x0A] = 1;
        let solid = current(&bytes);
        assert_eq!(solid.mode, LightMode::Effect(1));
        assert_eq!(solid.speed, None);
        assert!(solid.color.is_some());
        bytes[0x0A] = 3;
        let rainbow = current(&bytes);
        assert!(rainbow.speed.is_some());
        assert_eq!(rainbow.color, None);
    }

    #[test]
    fn effect_byte_0_reads_as_off_with_no_settings() {
        let mut bytes = block();
        bytes[0x0A] = 0;
        let off = current(&bytes);
        assert_eq!(off.mode, LightMode::Off);
        assert_eq!((off.brightness, off.speed, off.color), (None, None, None));
    }

    #[test]
    fn the_custom_mode_reads_as_custom_from_either_byte() {
        let mut bytes = block();
        bytes[0x09] = 1;
        assert_eq!(current(&bytes).mode, LightMode::Custom);
        assert_eq!(current(&bytes).brightness, None);

        let mut bytes = block();
        bytes[0x0A] = 18;
        assert_eq!(current(&bytes).mode, LightMode::Custom);

        let mut bytes = block();
        bytes[0x09] = 1;
        bytes[0x0A] = 0;
        assert_eq!(current(&bytes).mode, LightMode::Custom);
    }

    #[test]
    fn an_effect_the_model_doesnt_have_reads_as_unknown() {
        for byte in [19, 20, 0x7F, 0xFF] {
            let mut bytes = block();
            bytes[0x0A] = byte;
            let read = current(&bytes);
            assert_eq!(read.mode, LightMode::Unknown(byte));
            assert_eq!(read.brightness, None);
        }
    }

    #[test]
    fn a_sleep_byte_outside_the_options_reads_raw() {
        let mut bytes = block();
        bytes[0x18] = 0x1E;
        assert_eq!(current(&bytes).sleep, SleepSetting::Unknown(0x1E));
        for sleep in SleepTime::ALL {
            bytes[0x18] = sleep.byte();
            assert_eq!(current(&bytes).sleep, SleepSetting::Known(sleep));
        }
    }

    // Applying.

    #[test]
    fn apply_reads_once_then_writes_once_and_changes_only_whats_given() {
        let change = LightingChange {
            brightness: Some(16),
            ..LightingChange::default()
        };
        let (fake, result) = apply_to(&block(), &change);
        assert_eq!(commands(&fake), [0x84, 0x04]);
        assert_eq!(result.unwrap().brightness, Some(16));

        let mut expected = block();
        expected[0x38 + 10] = 16;
        assert_eq!(written_config(&fake), expected);
    }

    #[test]
    fn apply_writes_every_field_at_its_offset_and_nothing_else() {
        let change = LightingChange {
            mode: Some(ModeChoice::Effect(7)),
            brightness: Some(20),
            speed: Some(5),
            color: Some(ColorChoice::Slot(6)),
            sleep: Some(SleepTime::NoSleep),
        };
        let (fake, result) = apply_to(&block(), &change);
        let mut expected = block();
        expected[0x0A] = 7;
        expected[0x18] = 0x3C;
        expected[0x38 + 14] = 20;
        expected[0x39 + 14] = 0x56;
        assert_eq!(written_config(&fake), expected);
        assert_eq!(
            result,
            Ok(CurrentLighting {
                mode: LightMode::Effect(7),
                brightness: Some(20),
                speed: Some(5),
                color: Some(ColorSetting::Slot(6)),
                sleep: SleepSetting::Known(SleepTime::NoSleep),
            })
        );
    }

    #[test]
    fn a_new_effect_keeps_the_settings_it_already_had() {
        let mut bytes = block();
        bytes[0x38 + 14] = 4;
        bytes[0x39 + 14] = 0x51;
        let (fake, result) = apply_to(&bytes, &effect(7));
        let mut expected = bytes;
        expected[0x0A] = 7;
        assert_eq!(written_config(&fake), expected);
        let result = result.unwrap();
        assert_eq!(result.brightness, Some(4));
        assert_eq!(result.speed, Some(5));
    }

    #[test]
    fn rainbow_sets_the_colour_value_7() {
        let change = LightingChange {
            color: Some(ColorChoice::Rainbow),
            ..LightingChange::default()
        };
        let (fake, result) = apply_to(&block(), &change);
        assert_eq!(written_config(&fake)[0x39 + 10], 0x27);
        assert_eq!(result.unwrap().color, Some(ColorSetting::Rainbow));
    }

    #[test]
    fn off_writes_effect_0() {
        let change = LightingChange {
            mode: Some(ModeChoice::Off),
            ..LightingChange::default()
        };
        let (fake, result) = apply_to(&block(), &change);
        let mut expected = block();
        expected[0x0A] = 0;
        assert_eq!(written_config(&fake), expected);
        assert_eq!(result.unwrap().mode, LightMode::Off);
    }

    #[test]
    fn a_sleep_byte_outside_the_options_survives_other_changes() {
        let mut bytes = block();
        bytes[0x18] = 0x1E;
        let change = LightingChange {
            brightness: Some(4),
            ..LightingChange::default()
        };
        let (fake, result) = apply_to(&bytes, &change);
        assert_eq!(written_config(&fake)[0x18], 0x1E);
        assert_eq!(result.unwrap().sleep, SleepSetting::Unknown(0x1E));
    }

    #[test]
    fn nothing_to_change_writes_nothing() {
        let (fake, result) = apply_to(&block(), &LightingChange::default());
        assert_eq!(commands(&fake), [0x84]);
        assert_eq!(result.unwrap().mode, LightMode::Effect(5));

        // The same values as the keyboard already has.
        let change = LightingChange {
            brightness: Some(8),
            ..LightingChange::default()
        };
        let (fake, _) = apply_to(&block(), &change);
        assert_eq!(commands(&fake), [0x84]);
    }

    // Custom colours and palettes.

    #[test]
    fn a_custom_colour_goes_to_slot_0_of_the_effects_entry() {
        let mut palettes = Palettes::default();
        palettes.set(9, 4, RED).unwrap();
        let change = LightingChange {
            color: Some(ColorChoice::Custom(PINK)),
            ..LightingChange::default()
        };
        let fake = fake_with(&block());
        let result = apply(&mut device(&fake), &change, &mut palettes);

        assert_eq!(result.unwrap().color, Some(ColorSetting::Slot(0)));
        assert_eq!(commands(&fake), [0x84, 0x04, 0x0A]);
        assert_eq!(written_config(&fake)[0x39 + 10], 0x20);

        let sent = fake.sent();
        let report = &sent[2];
        assert_eq!(&report[..8], &[0x09, 0x0A, 0, 0, 1, 0, 0xA4, 0x01]);
        assert_eq!(&report[8..428], palettes.as_bytes());
        assert!(report[428..].iter().all(|&b| b == 0));

        // The change is in the palette the caller keeps, and what it already held went out too.
        assert_eq!(palettes.get(5, 0), Some(PINK));
        assert_eq!(palettes.get(9, 4), Some(RED));
    }

    #[test]
    fn palettes_go_out_100_ms_after_the_config() {
        let change = LightingChange {
            color: Some(ColorChoice::Custom(PINK)),
            ..LightingChange::default()
        };
        let (fake, _) = apply_to(&block(), &change);
        assert_eq!(
            fake.sleeps(),
            [25, 25, 25, 100, 25].map(Duration::from_millis)
        );
    }

    #[test]
    fn a_custom_colour_on_a_new_effect_goes_to_that_effect() {
        let mut palettes = Palettes::default();
        let change = LightingChange {
            mode: Some(ModeChoice::Effect(7)),
            color: Some(ColorChoice::Custom(PINK)),
            ..LightingChange::default()
        };
        let fake = fake_with(&block());
        apply(&mut device(&fake), &change, &mut palettes).unwrap();
        assert_eq!(palettes.get(7, 0), Some(PINK));
        assert_ne!(palettes.get(5, 0), Some(PINK));
    }

    #[test]
    fn the_same_custom_colour_is_still_written() {
        // The keyboard can't report palettes, so it may not hold what the caller's copy says.
        let mut bytes = block();
        bytes[0x39 + 10] = 0x20;
        let mut palettes = Palettes::default();
        palettes.set(5, 0, PINK).unwrap();
        let change = LightingChange {
            color: Some(ColorChoice::Custom(PINK)),
            ..LightingChange::default()
        };
        let fake = fake_with(&bytes);
        apply(&mut device(&fake), &change, &mut palettes).unwrap();
        assert_eq!(commands(&fake), [0x84, 0x0A]);
    }

    #[test]
    fn a_failed_config_write_leaves_the_palettes_alone() {
        let mut palettes = Palettes::default();
        let change = LightingChange {
            color: Some(ColorChoice::Custom(PINK)),
            ..LightingChange::default()
        };
        let fake = fake_with(&block());
        // The read's send is number 0.
        (1..4).for_each(|n| fake.fail_send(n));
        let result = apply(&mut device(&fake), &change, &mut palettes);
        assert!(matches!(result, Err(Error::Hid(_))));
        assert_eq!(commands(&fake), [0x84]);
        assert_eq!(palettes, Palettes::default());
    }

    #[test]
    fn a_failed_palette_write_leaves_the_callers_copy_alone() {
        let mut palettes = Palettes::default();
        let change = LightingChange {
            color: Some(ColorChoice::Custom(PINK)),
            ..LightingChange::default()
        };
        let fake = fake_with(&block());
        (2..5).for_each(|n| fake.fail_send(n));
        let result = apply(&mut device(&fake), &change, &mut palettes);
        assert!(matches!(result, Err(Error::Hid(_))));
        assert_eq!(palettes, Palettes::default());
    }

    // Nothing is written when the read or the checks fail.

    #[test]
    fn a_failed_read_writes_nothing() {
        let mut no_marker = block();
        no_marker[0x7F] = 0;
        let fake = FakeTransport::new();
        (0..5).for_each(|_| fake.queue_feature_reply(&config_reply(&no_marker)));
        let change = effect(7);
        let result = apply(&mut device(&fake), &change, &mut Palettes::default());
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert_eq!(commands(&fake), [0x84; 5]);

        let fake = FakeTransport::new();
        fake.disconnect();
        let result = apply(&mut device(&fake), &change, &mut Palettes::default());
        assert!(matches!(result, Err(Error::Hid(_))));
        assert!(fake.sent().is_empty());
    }

    #[test]
    fn values_the_model_doesnt_allow_are_refused_with_nothing_written() {
        let base = LightingChange::default;
        let changes = [
            // An effect that doesn't exist, or the custom mode, which isn't offered.
            effect(18),
            effect(19),
            effect(0),
            LightingChange {
                brightness: Some(5),
                ..base()
            },
            LightingChange {
                speed: Some(6),
                ..base()
            },
            LightingChange {
                color: Some(ColorChoice::Slot(7)),
                ..base()
            },
            // Effect 1 has no speed.
            LightingChange {
                mode: Some(ModeChoice::Effect(1)),
                speed: Some(1),
                ..base()
            },
            // Effect 3 has no colour, custom or not, and no rainbow option.
            LightingChange {
                mode: Some(ModeChoice::Effect(3)),
                color: Some(ColorChoice::Slot(1)),
                ..base()
            },
            LightingChange {
                mode: Some(ModeChoice::Effect(3)),
                color: Some(ColorChoice::Custom(RED)),
                ..base()
            },
            LightingChange {
                mode: Some(ModeChoice::Effect(3)),
                color: Some(ColorChoice::Rainbow),
                ..base()
            },
            // Off has no controls.
            LightingChange {
                mode: Some(ModeChoice::Off),
                brightness: Some(4),
                ..base()
            },
            // One bad value among good ones.
            LightingChange {
                mode: Some(ModeChoice::Effect(7)),
                brightness: Some(4),
                speed: Some(1),
                color: Some(ColorChoice::Slot(9)),
                sleep: Some(SleepTime::Min5),
            },
        ];
        for change in &changes {
            assert_refused(&block(), change);
        }
    }

    #[test]
    fn a_refused_change_leaves_the_callers_palette_alone() {
        let mut palettes = Palettes::default();
        let change = LightingChange {
            mode: Some(ModeChoice::Effect(3)),
            color: Some(ColorChoice::Custom(RED)),
            ..LightingChange::default()
        };
        let fake = fake_with(&block());
        assert!(apply(&mut device(&fake), &change, &mut palettes).is_err());
        assert_eq!(palettes, Palettes::default());
    }

    #[test]
    fn effect_settings_need_a_current_effect() {
        let settings = [
            LightingChange {
                brightness: Some(4),
                ..LightingChange::default()
            },
            LightingChange {
                speed: Some(1),
                ..LightingChange::default()
            },
            LightingChange {
                color: Some(ColorChoice::Slot(1)),
                ..LightingChange::default()
            },
        ];
        let mut off = block();
        off[0x0A] = 0;
        let mut custom = block();
        custom[0x09] = 1;
        let mut effect_18 = block();
        effect_18[0x0A] = 18;
        let mut unknown = block();
        unknown[0x0A] = 200;
        for bytes in [off, custom, effect_18, unknown] {
            for change in &settings {
                assert_refused(&bytes, change);
            }
        }
    }

    // On a keyboard in the custom mode.

    #[test]
    fn the_custom_mode_stays_unless_the_effect_changes() {
        let mut bytes = block();
        bytes[0x09] = 1;
        bytes[0x0A] = 18;
        let change = LightingChange {
            sleep: Some(SleepTime::Min5),
            ..LightingChange::default()
        };
        let (fake, result) = apply_to(&bytes, &change);
        let mut expected = bytes;
        expected[0x18] = 0x0A;
        assert_eq!(written_config(&fake), expected);
        assert_eq!(result.unwrap().mode, LightMode::Custom);
    }

    #[test]
    fn choosing_an_effect_leaves_the_custom_mode() {
        let mut bytes = block();
        bytes[0x09] = 1;
        bytes[0x0A] = 18;
        let (fake, result) = apply_to(&bytes, &effect(7));
        let written = written_config(&fake);
        assert_eq!((written[0x09], written[0x0A]), (0, 7));
        assert_eq!(result.unwrap().mode, LightMode::Effect(7));

        let change = LightingChange {
            mode: Some(ModeChoice::Off),
            ..LightingChange::default()
        };
        let (fake, result) = apply_to(&bytes, &change);
        let written = written_config(&fake);
        assert_eq!((written[0x09], written[0x0A]), (0, 0));
        assert_eq!(result.unwrap().mode, LightMode::Off);
    }

    // Every supported model takes the same flow.

    #[test]
    fn apply_works_on_every_supported_model() {
        for model in model::supported() {
            let main = fake_with(&block());
            let screen = model.has_second_collection().then(FakeTransport::new);
            let mut device = Device::new(main.clone(), screen.clone(), model).unwrap();
            apply(&mut device, &effect(7), &mut Palettes::default()).unwrap();
            assert_eq!(commands(&main), [0x84, 0x04], "{:04X}", model.pid);
            if let Some(screen) = screen {
                assert!(
                    screen.sent().is_empty(),
                    "config stays on the main collection"
                );
            }
        }
    }
}
