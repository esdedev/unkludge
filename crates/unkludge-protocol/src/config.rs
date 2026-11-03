//! The 128-byte config block, which holds the lighting settings. It is always read, edited and
//! written back whole: bytes this crate doesn't know about keep the value the keyboard gave them.
//!
//! Offsets, for models of the firmware family this crate supports:
//!
//! | Offset | Meaning |
//! |---|---|
//! | `0x09` | 1 when the custom mode is on |
//! | `0x0A` | Effect number, 0 for off |
//! | `0x18` | Sleep time |
//! | `0x38 + 2 * effect` | That effect's brightness byte |
//! | `0x39 + 2 * effect` | That effect's `speed << 4 \| colour`. Colour 7 means rainbow |
//! | `0x7E`, `0x7F` | `5A A5`, the end marker |

use std::time::Duration;

use crate::model::{Effect, Lighting};
use crate::packet::cmd;
use crate::transport::Transport;
use crate::{Device, Error};

/// Size of the config block.
pub const CONFIG_LEN: usize = 128;

const CUSTOM_FLAG: usize = 0x09;
const EFFECT: usize = 0x0A;
const SLEEP: usize = 0x18;
const EFFECT_SETTINGS: usize = 0x38;
const END_MARKER: usize = 0x7E;
const END_MARKER_BYTES: [u8; 2] = [0x5A, 0xA5];

/// The effect number of the custom mode, which this version doesn't offer but must recognize.
const CUSTOM_EFFECT: u8 = 18;
/// The colour value that means rainbow. Smaller values are palette slots.
const RAINBOW: u8 = 7;

/// How many times a read is tried before giving up on it.
const READ_TRIES: u32 = 5;
const READ_RETRY_GAP: Duration = Duration::from_millis(100);

/// The sleep times a keyboard can be set to, labelled as the official app labels them.
///
/// The bytes don't look like minutes: they may count in 30-second units, which fits the first three
/// labels. What `NoSleep` does on the keyboard hasn't been confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepTime {
    Min5,
    Min10,
    Min20,
    NoSleep,
}

impl SleepTime {
    /// Every option, shortest first.
    pub const ALL: [SleepTime; 4] = [
        SleepTime::Min5,
        SleepTime::Min10,
        SleepTime::Min20,
        SleepTime::NoSleep,
    ];

    /// The byte written to the config block.
    pub const fn byte(self) -> u8 {
        match self {
            SleepTime::Min5 => 0x0A,
            SleepTime::Min10 => 0x14,
            SleepTime::Min20 => 0x28,
            SleepTime::NoSleep => 0x3C,
        }
    }

    /// The option with this byte, if there is one.
    pub fn from_byte(byte: u8) -> Option<SleepTime> {
        SleepTime::ALL.into_iter().find(|s| s.byte() == byte)
    }

    pub const fn label(self) -> &'static str {
        match self {
            SleepTime::Min5 => "5 min",
            SleepTime::Min10 => "10 min",
            SleepTime::Min20 => "20 min",
            SleepTime::NoSleep => "No sleep",
        }
    }
}

/// A config block as read from a keyboard, whose end marker has been checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    bytes: [u8; CONFIG_LEN],
}

impl Config {
    /// Checks the length and the end marker.
    pub fn from_bytes(bytes: &[u8]) -> Result<Config, Error> {
        let bytes = <[u8; CONFIG_LEN]>::try_from(bytes).map_err(|_| {
            Error::Invalid(format!(
                "a config block has {CONFIG_LEN} bytes, not {}",
                bytes.len()
            ))
        })?;
        if bytes[END_MARKER..] != END_MARKER_BYTES {
            return Err(Error::Invalid(format!(
                "a config block ends in {END_MARKER_BYTES:02X?}, not {:02X?}",
                &bytes[END_MARKER..]
            )));
        }
        Ok(Config { bytes })
    }

    /// The block, as it is on the keyboard.
    pub fn as_bytes(&self) -> &[u8; CONFIG_LEN] {
        &self.bytes
    }

    /// Reads the block, trying again if the read fails or the block has no end marker. The last
    /// error is the one returned.
    pub fn read<T: Transport>(device: &mut Device<T>) -> Result<Config, Error> {
        let mut result = read_once(device);
        for _ in 1..READ_TRIES {
            if result.is_ok() {
                break;
            }
            device.sleep(READ_RETRY_GAP);
            result = read_once(device);
        }
        result
    }

    /// Writes the block back. These writes get no ack.
    pub(crate) fn write<T: Transport>(&self, device: &mut Device<T>) -> Result<(), Error> {
        device.send_generic(cmd::CONFIG, 0, &self.bytes)
    }

    /// The effect number, 0 for off. May be one the model doesn't have.
    pub fn effect(&self) -> u8 {
        self.bytes[EFFECT]
    }

    /// Whether the keyboard is in the custom mode, whatever the effect byte says.
    pub fn is_custom(&self) -> bool {
        self.bytes[CUSTOM_FLAG] == 1 || self.effect() == CUSTOM_EFFECT
    }

    /// The raw sleep byte, which may be one that [`SleepTime`] doesn't list.
    pub fn sleep(&self) -> u8 {
        self.bytes[SLEEP]
    }

    /// The raw brightness byte stored for an effect, or `None` if the block has no place for that
    /// effect's settings.
    pub fn brightness(&self, effect: u8) -> Option<u8> {
        Some(self.bytes[settings_offset(effect)?])
    }

    /// The speed stored for an effect.
    pub fn speed(&self, effect: u8) -> Option<u8> {
        Some(self.bytes[settings_offset(effect)? + 1] >> 4)
    }

    /// The colour value stored for an effect: a palette slot, or 7 for rainbow.
    pub fn color(&self, effect: u8) -> Option<u8> {
        Some(self.bytes[settings_offset(effect)? + 1] & 0x0F)
    }

    /// Selects an effect, or turns the lights off with 0, and leaves the custom mode.
    pub(crate) fn set_effect(&mut self, lighting: &Lighting, effect: u8) -> Result<(), Error> {
        if effect != 0 {
            find_effect(lighting, effect)?;
        }
        self.bytes[EFFECT] = effect;
        self.bytes[CUSTOM_FLAG] = 0;
        Ok(())
    }

    pub(crate) fn set_brightness(
        &mut self,
        lighting: &Lighting,
        effect: u8,
        value: u8,
    ) -> Result<(), Error> {
        let (entry, offset) = effect_settings(lighting, effect)?;
        if !entry.brightness {
            return Err(no_control(entry, "brightness"));
        }
        if !lighting.brightness.contains(&value) {
            return Err(Error::Invalid(format!(
                "brightness {value} isn't one of {:?}",
                lighting.brightness
            )));
        }
        self.bytes[offset] = value;
        Ok(())
    }

    pub(crate) fn set_speed(
        &mut self,
        lighting: &Lighting,
        effect: u8,
        value: u8,
    ) -> Result<(), Error> {
        let (entry, offset) = effect_settings(lighting, effect)?;
        if !entry.speed {
            return Err(no_control(entry, "speed"));
        }
        let (min, max) = (lighting.speed_min, lighting.speed_max);
        if !(min..=max).contains(&value) {
            return Err(Error::Invalid(format!(
                "speed {value} isn't within {min}..={max}"
            )));
        }
        let byte = &mut self.bytes[offset + 1];
        *byte = (value << 4) | (*byte & 0x0F);
        Ok(())
    }

    /// Makes an effect use a palette slot, 0 to 6.
    pub(crate) fn set_color(
        &mut self,
        lighting: &Lighting,
        effect: u8,
        slot: u8,
    ) -> Result<(), Error> {
        let (entry, offset) = effect_settings(lighting, effect)?;
        if !entry.color {
            return Err(no_control(entry, "colour"));
        }
        if slot >= RAINBOW {
            return Err(Error::Invalid(format!(
                "palette slot {slot} isn't within 0..={}",
                RAINBOW - 1
            )));
        }
        self.write_color(offset, slot);
        Ok(())
    }

    /// Makes an effect use the rainbow option.
    pub(crate) fn set_rainbow(&mut self, lighting: &Lighting, effect: u8) -> Result<(), Error> {
        let (entry, offset) = effect_settings(lighting, effect)?;
        if !entry.rainbow {
            return Err(no_control(entry, "rainbow"));
        }
        self.write_color(offset, RAINBOW);
        Ok(())
    }

    pub(crate) fn set_sleep(&mut self, lighting: &Lighting, sleep: SleepTime) -> Result<(), Error> {
        if !lighting.sleep {
            return Err(Error::Invalid(
                "this keyboard has no sleep setting".to_string(),
            ));
        }
        self.bytes[SLEEP] = sleep.byte();
        Ok(())
    }

    fn write_color(&mut self, offset: usize, color: u8) {
        let byte = &mut self.bytes[offset + 1];
        *byte = (*byte & 0xF0) | color;
    }
}

fn read_once<T: Transport>(device: &mut Device<T>) -> Result<Config, Error> {
    Config::from_bytes(&device.read_block(cmd::CONFIG_READ, 0, CONFIG_LEN)?)
}

/// Where an effect's two settings bytes are. Off has none. Past the custom mode the block holds
/// other things.
fn settings_offset(effect: u8) -> Option<usize> {
    (1..=CUSTOM_EFFECT)
        .contains(&effect)
        .then(|| EFFECT_SETTINGS + 2 * usize::from(effect))
}

fn find_effect(lighting: &Lighting, effect: u8) -> Result<&Effect, Error> {
    lighting
        .effects
        .iter()
        .find(|e| e.id == effect)
        .ok_or_else(|| Error::Invalid(format!("this keyboard has no effect {effect}")))
}

fn effect_settings(lighting: &Lighting, effect: u8) -> Result<(&Effect, usize), Error> {
    let entry = find_effect(lighting, effect)?;
    let offset = settings_offset(effect)
        .ok_or_else(|| Error::Invalid(format!("effect {effect} has no settings to change")))?;
    Ok((entry, offset))
}

fn no_control(effect: &Effect, control: &str) -> Error {
    Error::Invalid(format!(
        "effect {} ({}) has no {control} option",
        effect.id, effect.name
    ))
}

#[cfg(test)]
pub(crate) fn config_reply(block: &[u8; CONFIG_LEN]) -> Vec<u8> {
    let mut reply = vec![0x09, 0x84, 0, 0, 1, 0, 0x80, 0];
    reply.extend(block);
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{self, Lighting, Model};
    use crate::transport::fake::{FakeEvent, FakeTransport};
    use std::time::Duration;

    fn s85() -> &'static Model {
        model::find(0x0265).unwrap()
    }

    fn lighting() -> &'static Lighting {
        &s85().lighting
    }

    /// A block where each byte differs from its neighbours, so that a stray write shows up.
    fn block() -> [u8; CONFIG_LEN] {
        let mut bytes = [0; CONFIG_LEN];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = (i as u8).wrapping_mul(3) | 1;
        }
        bytes[0x7E..].copy_from_slice(&[0x5A, 0xA5]);
        bytes
    }

    fn config() -> Config {
        Config::from_bytes(&block()).unwrap()
    }

    /// The positions where two blocks differ.
    fn changed(before: &Config, after: &Config) -> Vec<usize> {
        (0..CONFIG_LEN)
            .filter(|&i| before.as_bytes()[i] != after.as_bytes()[i])
            .collect()
    }

    fn device(fake: &FakeTransport) -> crate::Device<FakeTransport> {
        crate::Device::new(fake.clone(), None, s85()).unwrap()
    }

    #[test]
    fn from_bytes_wants_the_end_marker() {
        assert!(Config::from_bytes(&block()).is_ok());
        let mut wrong = block();
        wrong[0x7E] = 0x5B;
        assert!(matches!(Config::from_bytes(&wrong), Err(Error::Invalid(_))));
        let mut wrong = block();
        wrong[0x7F] = 0xA4;
        assert!(matches!(Config::from_bytes(&wrong), Err(Error::Invalid(_))));
    }

    #[test]
    fn from_bytes_wants_the_full_length() {
        assert!(matches!(Config::from_bytes(&[]), Err(Error::Invalid(_))));
        let mut long = block().to_vec();
        long.push(0);
        assert!(matches!(Config::from_bytes(&long), Err(Error::Invalid(_))));
    }

    #[test]
    fn as_bytes_returns_the_block_unchanged() {
        assert_eq!(*config().as_bytes(), block());
    }

    // Reading.

    #[test]
    fn read_returns_the_block_from_the_keyboard() {
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&config_reply(&block()));
        let read = Config::read(&mut device(&fake)).unwrap();
        assert_eq!(*read.as_bytes(), block());
        assert_eq!(&fake.sent()[0][..8], &[0x09, 0x84, 0, 0, 1, 0, 0x80, 0]);
        assert_eq!(fake.sent().len(), 1);
    }

    #[test]
    fn read_retries_a_block_without_the_marker() {
        let mut bad = block();
        bad[0x7F] = 0;
        let fake = FakeTransport::new();
        fake.queue_feature_reply(&config_reply(&bad));
        fake.queue_feature_reply(&config_reply(&block()));
        let read = Config::read(&mut device(&fake)).unwrap();
        assert_eq!(*read.as_bytes(), block());
        assert_eq!(fake.sent().len(), 2);
        assert_eq!(
            fake.sleeps(),
            [25, 25, 100, 25, 25].map(Duration::from_millis)
        );
    }

    #[test]
    fn read_gives_up_after_5_tries_with_the_last_error() {
        let mut bad = block();
        bad[0x7F] = 0;
        let fake = FakeTransport::new();
        (0..6).for_each(|_| fake.queue_feature_reply(&config_reply(&bad)));
        let result = Config::read(&mut device(&fake));
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert_eq!(fake.sent().len(), 5);
        let gaps = fake
            .sleeps()
            .iter()
            .filter(|&&d| d == Duration::from_millis(100))
            .count();
        assert_eq!(gaps, 4);
    }

    #[test]
    fn read_retries_transport_errors_too() {
        // All 3 sends of the first read fail, and the second read works.
        let fake = FakeTransport::new();
        (0..3).for_each(|n| fake.fail_send(n));
        fake.queue_feature_reply(&config_reply(&block()));
        assert!(Config::read(&mut device(&fake)).is_ok());

        let fake = FakeTransport::new();
        fake.disconnect();
        let result = Config::read(&mut device(&fake));
        assert!(matches!(result, Err(Error::Hid(_))));
    }

    // Writing.

    #[test]
    fn write_sends_one_report_with_the_whole_block() {
        let fake = FakeTransport::new();
        config().write(&mut device(&fake)).unwrap();
        let sent = fake.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(&sent[0][..8], &[0x09, 0x04, 0, 0, 1, 0, 0x80, 0]);
        assert_eq!(&sent[0][8..136], &block());
        assert!(sent[0][136..].iter().all(|&b| b == 0));
        assert!(fake.events().iter().all(|e| *e != FakeEvent::GetFeature));
    }

    // Fields.

    #[test]
    fn getters_use_the_documented_offsets() {
        let mut bytes = block();
        bytes[0x0A] = 5;
        bytes[0x18] = 0x28;
        bytes[0x38 + 2 * 5] = 16;
        bytes[0x39 + 2 * 5] = 0x37;
        let config = Config::from_bytes(&bytes).unwrap();
        assert_eq!(config.effect(), 5);
        assert_eq!(config.sleep(), 0x28);
        assert_eq!(config.brightness(5), Some(16));
        assert_eq!(config.speed(5), Some(3));
        assert_eq!(config.color(5), Some(7));
    }

    #[test]
    fn effect_0_and_numbers_past_the_custom_one_have_no_settings() {
        let config = config();
        for effect in [0, 19, 200] {
            assert_eq!(config.brightness(effect), None);
            assert_eq!(config.speed(effect), None);
            assert_eq!(config.color(effect), None);
        }
    }

    #[test]
    fn the_custom_mode_shows_in_either_of_two_bytes() {
        let mut bytes = block();
        bytes[0x09] = 0;
        bytes[0x0A] = 5;
        assert!(!Config::from_bytes(&bytes).unwrap().is_custom());
        bytes[0x09] = 1;
        assert!(Config::from_bytes(&bytes).unwrap().is_custom());
        bytes[0x09] = 0;
        bytes[0x0A] = 18;
        assert!(Config::from_bytes(&bytes).unwrap().is_custom());
    }

    #[test]
    fn set_effect_changes_the_effect_and_clears_custom_only() {
        let before = config();
        let mut after = before.clone();
        after.set_effect(lighting(), 11).unwrap();
        assert_eq!(after.effect(), 11);
        assert_eq!(after.as_bytes()[0x09], 0);
        assert_eq!(changed(&before, &after), [0x09, 0x0A]);
    }

    #[test]
    fn set_effect_0_means_off() {
        let mut config = config();
        config.set_effect(lighting(), 0).unwrap();
        assert_eq!(config.effect(), 0);
    }

    #[test]
    fn set_effect_refuses_effects_the_model_doesnt_have() {
        let before = config();
        let mut after = before.clone();
        for effect in [18, 19, 255] {
            assert!(matches!(
                after.set_effect(lighting(), effect),
                Err(Error::Invalid(_))
            ));
        }
        assert_eq!(after, before);
    }

    #[test]
    fn set_brightness_changes_that_effects_byte_only() {
        let before = config();
        let mut after = before.clone();
        after.set_brightness(lighting(), 3, 12).unwrap();
        assert_eq!(after.brightness(3), Some(12));
        assert_eq!(changed(&before, &after), [0x38 + 6]);
    }

    #[test]
    fn set_brightness_takes_only_the_models_values() {
        let before = config();
        let mut after = before.clone();
        for value in [1, 21, 255] {
            assert!(matches!(
                after.set_brightness(lighting(), 3, value),
                Err(Error::Invalid(_))
            ));
        }
        assert_eq!(after, before);
    }

    #[test]
    fn speed_and_colour_share_a_byte_without_clobbering_each_other() {
        let mut bytes = block();
        bytes[0x39 + 2 * 2] = 0;
        let before = Config::from_bytes(&bytes).unwrap();
        let mut after = before.clone();

        after.set_speed(lighting(), 2, 5).unwrap();
        assert_eq!(after.as_bytes()[0x39 + 4], 0x50);
        after.set_color(lighting(), 2, 6).unwrap();
        assert_eq!(after.as_bytes()[0x39 + 4], 0x56);
        after.set_speed(lighting(), 2, 1).unwrap();
        assert_eq!(after.as_bytes()[0x39 + 4], 0x16);
        after.set_rainbow(lighting(), 2).unwrap();
        assert_eq!(after.as_bytes()[0x39 + 4], 0x17);
        assert_eq!(after.speed(2), Some(1));
        assert_eq!(after.color(2), Some(7));
        assert_eq!(changed(&before, &after), [0x39 + 4]);
    }

    #[test]
    fn set_speed_checks_the_range_and_the_effect() {
        let mut config = config();
        assert!(matches!(
            config.set_speed(lighting(), 2, 6),
            Err(Error::Invalid(_))
        ));
        // Effect 1, solid, has no speed.
        assert!(matches!(
            config.set_speed(lighting(), 1, 1),
            Err(Error::Invalid(_))
        ));
        config.set_speed(lighting(), 2, 0).unwrap();
        config.set_speed(lighting(), 2, 5).unwrap();
    }

    #[test]
    fn set_color_checks_the_slot_and_the_effect() {
        let mut config = config();
        assert!(matches!(
            config.set_color(lighting(), 2, 7),
            Err(Error::Invalid(_))
        ));
        // Effect 3 has neither a colour nor a rainbow option.
        assert!(matches!(
            config.set_color(lighting(), 3, 1),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            config.set_rainbow(lighting(), 3),
            Err(Error::Invalid(_))
        ));
        config.set_color(lighting(), 2, 0).unwrap();
        config.set_color(lighting(), 2, 6).unwrap();
    }

    #[test]
    fn field_setters_refuse_effects_the_model_doesnt_have() {
        let mut config = config();
        for effect in [0, 18, 19] {
            assert!(config.set_brightness(lighting(), effect, 4).is_err());
            assert!(config.set_speed(lighting(), effect, 1).is_err());
            assert!(config.set_color(lighting(), effect, 1).is_err());
            assert!(config.set_rainbow(lighting(), effect).is_err());
        }
    }

    #[test]
    fn sleep_changes_one_byte() {
        let before = config();
        let mut after = before.clone();
        after.set_sleep(lighting(), SleepTime::Min20).unwrap();
        assert_eq!(after.sleep(), 0x28);
        assert_eq!(changed(&before, &after), [0x18]);
    }

    #[test]
    fn sleep_times_have_the_bytes_and_labels_of_the_protocol() {
        let table: Vec<_> = SleepTime::ALL
            .iter()
            .map(|s| (s.byte(), s.label()))
            .collect();
        assert_eq!(
            table,
            [
                (0x0A, "5 min"),
                (0x14, "10 min"),
                (0x28, "20 min"),
                (0x3C, "No sleep")
            ]
        );
        for sleep in SleepTime::ALL {
            assert_eq!(SleepTime::from_byte(sleep.byte()), Some(sleep));
        }
        assert_eq!(SleepTime::from_byte(0), None);
        assert_eq!(SleepTime::from_byte(0x1E), None);
    }
}
