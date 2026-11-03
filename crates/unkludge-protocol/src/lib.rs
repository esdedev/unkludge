//! Protocol library for Royal Kludge keyboards.

pub mod clock;
pub mod config;
pub mod device;
mod error;
pub mod lighting;
pub mod model;
pub(crate) mod packet;
pub mod screen;
pub mod transport;

pub use device::Device;
pub use error::Error;
pub use packet::{REPORT_ID, REPORT_LEN};

#[cfg(test)]
mod allow_list_tests {
    //! Runs every public flow on the fake, through the public API, and checks what went out.

    use std::sync::atomic::AtomicBool;

    use crate::clock::{DateTime, set_clock};
    use crate::config::{CONFIG_LEN, Config, SleepTime, config_reply};
    use crate::lighting::{self, ColorChoice, LightingChange, ModeChoice, Palettes, Rgb};
    use crate::model::{self, Model};
    use crate::screen::{Animation, LoadOptions};
    use crate::transport::fake::{AckFault, FakeTransport};
    use crate::{Device, Error};

    const NOW: DateTime = DateTime {
        year: 2026,
        month: 11,
        day: 3,
        hour: 22,
        minute: 15,
        second: 0,
        weekday: 2,
    };

    fn block() -> [u8; CONFIG_LEN] {
        let mut block = [0; CONFIG_LEN];
        block[0x0A] = 1;
        block[0x7E..].copy_from_slice(&[0x5A, 0xA5]);
        block
    }

    /// A small GIF of three frames.
    fn gif() -> Vec<u8> {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};

        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for value in [10, 120, 240] {
                let image = RgbaImage::from_pixel(8, 8, Rgba([value, 0, 0, 255]));
                let delay = Delay::from_numer_denom_ms(100, 1);
                encoder
                    .encode_frame(Frame::from_parts(image, 0, 0, delay))
                    .unwrap();
            }
        }
        bytes
    }

    fn run_every_flow(model: &'static Model, main: &FakeTransport, screen: Option<&FakeTransport>) {
        let new_device = || Device::new(main.clone(), screen.cloned(), model).unwrap();
        let mut device = new_device();
        let mut palettes = Palettes::default();

        for _ in 0..4 {
            main.queue_feature_reply(&config_reply(&block()));
        }
        Config::read(&mut device).unwrap();
        lighting::read(&mut device).unwrap();
        let change = LightingChange {
            mode: Some(ModeChoice::Effect(2)),
            brightness: Some(8),
            speed: Some(3),
            color: Some(ColorChoice::Custom(Rgb::new(1, 2, 3))),
            sleep: Some(SleepTime::Min10),
        };
        lighting::apply(&mut device, &change, &mut palettes).unwrap();
        let change = LightingChange {
            mode: Some(ModeChoice::Off),
            ..LightingChange::default()
        };
        lighting::apply(&mut device, &change, &mut palettes).unwrap();

        set_clock(&mut device, &NOW).unwrap();

        let spec = model.screen.unwrap();
        let animation = Animation::from_bytes(&gif(), &spec, LoadOptions::default()).unwrap();
        let idle = AtomicBool::new(false);
        animation.upload(&mut device, &idle, |_| {}).unwrap();

        // A cancelled upload, which sends the abort.
        let cancel = AtomicBool::new(false);
        let result = animation.upload(&mut device, &cancel, |progress| {
            if progress.done == 3 {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        assert_eq!(result, Err(Error::Cancelled));
    }

    #[test]
    fn a_model_with_one_collection_sends_only_allowed_commands() {
        let main = FakeTransport::with_auto_ack();
        run_every_flow(model::find(0x0265).unwrap(), &main, None);
        assert!(main.sent().iter().any(|r| r[1] == 0x0C));
        main.assert_allowed_commands_only();
    }

    #[test]
    fn a_checksum_model_sends_only_allowed_commands() {
        let main = FakeTransport::with_auto_ack();
        run_every_flow(model::find(0x01A2).unwrap(), &main, None);
        main.assert_allowed_commands_only();
    }

    #[test]
    fn a_model_with_two_collections_sends_only_allowed_commands() {
        let main = FakeTransport::with_auto_ack();
        let screen = FakeTransport::with_auto_ack();
        run_every_flow(model::find(0x01B8).unwrap(), &main, Some(&screen));
        assert!(screen.sent().iter().any(|r| r[1] == 0x0C));
        main.assert_allowed_commands_only();
        screen.assert_allowed_commands_only();
    }

    #[test]
    fn failing_flows_send_only_allowed_commands() {
        let model = model::find(0x0265).unwrap();
        let spec = model.screen.unwrap();
        let animation = Animation::from_bytes(&gif(), &spec, LoadOptions::default()).unwrap();
        let idle = AtomicBool::new(false);

        // A dropped chunk ack and the abort after it, then a start that's refused.
        let fake = FakeTransport::with_auto_ack();
        fake.ack_fault(2, AckFault::Drop);
        fake.ack_fault(4, AckFault::Reject);
        let mut device = Device::new(fake.clone(), None, model).unwrap();
        assert!(animation.upload(&mut device, &idle, |_| {}).is_err());
        assert!(animation.upload(&mut device, &idle, |_| {}).is_err());
        fake.assert_allowed_commands_only();

        // Failed sends count too. So does a read that never gets a good reply.
        let fake = FakeTransport::with_auto_ack();
        (0..4).for_each(|n| fake.fail_send(n));
        let mut device = Device::new(fake.clone(), None, model).unwrap();
        assert!(animation.upload(&mut device, &idle, |_| {}).is_err());
        assert!(lighting::read(&mut device).is_err());
        fake.assert_allowed_commands_only();
    }
}
