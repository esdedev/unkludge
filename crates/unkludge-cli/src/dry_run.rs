//! A keyboard that doesn't exist, for `--dry-run`. It's only reached through this file: nothing here
//! can open a device, so a dry run can't send anything anywhere.

use std::cell::Cell;
use std::time::Duration;

use unkludge_protocol::command_byte::{CONFIG_READ, SCREEN_CONTROL, SCREEN_DATA};
use unkludge_protocol::config::{CONFIG_LEN, END_MARKER_BYTES};
use unkludge_protocol::model::Model;
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error, HEADER_LEN, REPORT_ID};

const CONFIG_READ_HEADER: [u8; HEADER_LEN] =
    [REPORT_ID, CONFIG_READ, 0, 0, 1, 0, CONFIG_LEN as u8, 0];
const ACK_LEN: usize = 8;

/// Takes reports, answers as a keyboard would and doesn't wait.
#[derive(Default)]
pub struct DryRunTransport {
    ack: Cell<Option<[u8; ACK_LEN]>>,
    config_requested: Cell<bool>,
}

impl Transport for DryRunTransport {
    fn send_feature(&self, report: &[u8]) -> Result<(), Error> {
        match report[1] {
            SCREEN_DATA => self.ack.set(Some(ack(report[4], report[5]))),
            SCREEN_CONTROL => self.ack.set(Some(ack(0, 0))),
            CONFIG_READ => self.config_requested.set(true),
            _ => {}
        }
        Ok(())
    }

    /// Answers the config read with a block that's all zero but for its end marker, which reads as
    /// the lights being off. Nothing else is ever read in a dry run.
    fn get_feature(&self, buf: &mut [u8]) -> Result<usize, Error> {
        if !self.config_requested.take() {
            return Err(Error::Hid("a dry run has only the config to read".into()));
        }
        buf.fill(0);
        buf[..CONFIG_READ_HEADER.len()].copy_from_slice(&CONFIG_READ_HEADER);
        let end = CONFIG_READ_HEADER.len() + CONFIG_LEN;
        buf[end - 2..end].copy_from_slice(&END_MARKER_BYTES);
        Ok(buf.len())
    }

    fn read_timeout(&self, buf: &mut [u8], _ms: i32) -> Result<usize, Error> {
        match self.ack.take() {
            Some(ack) => {
                buf[..ACK_LEN].copy_from_slice(&ack);
                Ok(ACK_LEN)
            }
            None => Ok(0),
        }
    }

    fn sleep(&self, _duration: Duration) {}
}

/// An ack that accepts a report: with the chunk's index and check byte, which control reports don't
/// need.
fn ack(idx: u8, check: u8) -> [u8; ACK_LEN] {
    [REPORT_ID, 0x0A, 0x06, 1, idx, check, 0, 0]
}

/// A device of this model, with as many collections as the model has.
pub fn device(model: &'static Model) -> Result<Device<DryRunTransport>, Error> {
    let screen = model.has_second_collection().then(DryRunTransport::default);
    Device::new(DryRunTransport::default(), screen, model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unkludge_protocol::config::Config;
    use unkludge_protocol::lighting::{self, LightMode, SleepSetting};
    use unkludge_protocol::model;
    use unkludge_protocol::screen::{Animation, LoadOptions};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    fn solid_png(spec_width: u32, spec_height: u32) -> Vec<u8> {
        use image::{ImageFormat, Rgba, RgbaImage};
        let image = RgbaImage::from_pixel(spec_width, spec_height, Rgba([255, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn the_config_read_is_a_blank_block_that_reads_as_off() {
        let mut device = device(model::find(0x0265).unwrap()).unwrap();
        let config = Config::read(&mut device).unwrap();
        assert_eq!(&config.as_bytes()[CONFIG_LEN - 2..], &[0x5A, 0xA5]);
        assert!(config.as_bytes()[..CONFIG_LEN - 2].iter().all(|&b| b == 0));

        let current = lighting::read(&mut device).unwrap();
        assert_eq!(current.mode, LightMode::Off);
        assert_eq!(current.sleep, SleepSetting::Unknown(0));
    }

    #[test]
    fn nothing_can_be_read_without_asking_first() {
        let transport = DryRunTransport::default();
        let mut buf = [0; 520];
        assert!(transport.get_feature(&mut buf).is_err());
    }

    #[test]
    fn each_read_request_gets_one_answer() {
        let transport = DryRunTransport::default();
        let mut buf = [0xEE; 520];
        transport
            .send_feature(&[REPORT_ID, CONFIG_READ, 0, 0])
            .unwrap();
        assert_eq!(transport.get_feature(&mut buf), Ok(520));
        assert_eq!(&buf[..8], &CONFIG_READ_HEADER);
        assert_eq!(&buf[134..136], &[0x5A, 0xA5]);
        assert!(buf[136..].iter().all(|&b| b == 0), "the rest is cleared");
        assert!(transport.get_feature(&mut buf).is_err(), "only one answer");
    }

    #[test]
    fn each_acked_report_gets_one_ack() {
        let transport = DryRunTransport::default();
        let mut buf = [0; 8];
        transport
            .send_feature(&[REPORT_ID, SCREEN_DATA, 0, 3, 7, 0x55, 0, 2])
            .unwrap();
        assert_eq!(transport.read_timeout(&mut buf, 0), Ok(8));
        assert_eq!(buf, [REPORT_ID, 0x0A, 0x06, 1, 7, 0x55, 0, 0]);
        assert_eq!(transport.read_timeout(&mut buf, 0), Ok(0), "only one ack");

        transport
            .send_feature(&[REPORT_ID, SCREEN_CONTROL, 0, 0, 1, 0, 5, 0])
            .unwrap();
        assert_eq!(transport.read_timeout(&mut buf, 0), Ok(8));
        assert_eq!(buf, [REPORT_ID, 0x0A, 0x06, 1, 0, 0, 0, 0]);
        assert_eq!(transport.read_timeout(&mut buf, 0), Ok(0), "only one ack");
    }

    #[test]
    fn sleeping_takes_no_real_time() {
        let start = Instant::now();
        DryRunTransport::default().sleep(Duration::from_secs(30));
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn generic_commands_get_no_ack() {
        let transport = DryRunTransport::default();
        transport.send_feature(&[REPORT_ID, 0x0B, 0, 0]).unwrap();
        let mut buf = [0; 8];
        assert_eq!(transport.read_timeout(&mut buf, 0), Ok(0));
    }

    /// Anything left waiting for an ack would stop an upload for 8 s.
    #[test]
    fn a_whole_upload_takes_no_real_time() {
        let model = model::find(0x0265).unwrap();
        let mut device = device(model).unwrap();
        let spec = model.screen.unwrap();
        let animation =
            Animation::from_bytes(&solid_png(32, 32), &spec, LoadOptions::default()).unwrap();

        let start = Instant::now();
        let idle = AtomicBool::new(false);
        let outcome = animation.upload(&mut device, &idle, |_| {}).unwrap();
        assert!(outcome.finish_confirmed);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn checksum_models_get_acks_with_the_check_byte() {
        let model = model::find(0x01A2).unwrap();
        let mut device = device(model).unwrap();
        let spec = model.screen.unwrap();
        let animation =
            Animation::from_bytes(&solid_png(32, 32), &spec, LoadOptions::default()).unwrap();
        let idle = AtomicBool::new(false);
        let outcome = animation.upload(&mut device, &idle, |_| {}).unwrap();
        assert!(outcome.finish_confirmed);
    }

    #[test]
    fn models_with_two_collections_get_both() {
        use unkludge_protocol::device::{Collection, PacketEvent};

        let model = model::find(0x01B8).unwrap();
        let mut device = device(model).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hook = Arc::clone(&seen);
        device.on_packet = Some(Box::new(move |event| {
            if let PacketEvent::Sent { collection, report } = event {
                hook.lock().unwrap().push((collection, report[1]));
            }
        }));
        let spec = model.screen.unwrap();
        let animation =
            Animation::from_bytes(&solid_png(32, 32), &spec, LoadOptions::default()).unwrap();
        let idle = AtomicBool::new(false);
        animation.upload(&mut device, &idle, |_| {}).unwrap();

        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], (Collection::Main, SCREEN_CONTROL));
        assert_eq!(seen[1], (Collection::Screen, SCREEN_CONTROL));
        assert_eq!(seen[2], (Collection::Screen, SCREEN_DATA));
    }

    #[test]
    fn unsupported_models_are_refused() {
        let v1 = model::find(0x002A).unwrap();
        assert_eq!(device(v1).err(), Some(Error::Unsupported(0x002A)));
    }
}
