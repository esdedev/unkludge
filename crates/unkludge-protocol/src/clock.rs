//! The clock on the keyboard's screen. Callers supply the time, so this crate has no calendar
//! dependency.

use crate::packet::cmd;
use crate::transport::Transport;
use crate::{Device, Error};

/// A local date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// Must not be 0.
    pub year: u16,
    /// 1 to 12.
    pub month: u8,
    /// 1 to the length of the month.
    pub day: u8,
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
    /// 0 to 59.
    pub second: u8,
    /// Day of the week, 0 for Sunday to 6 for Saturday.
    pub weekday: u8,
}

impl DateTime {
    /// Checks each field. The weekday isn't compared to the date.
    pub fn validate(&self) -> Result<(), Error> {
        let valid = self.year != 0
            && (1..=12).contains(&self.month)
            && (1..=month_length(self.year, self.month)).contains(&self.day)
            && self.hour <= 23
            && self.minute <= 59
            && self.second <= 59
            && self.weekday <= 6;
        if valid {
            Ok(())
        } else {
            Err(Error::Invalid(format!("not a valid time: {self:?}")))
        }
    }
}

fn month_length(year: u16, month: u8) -> u8 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `[0, 0, 0] <year LE16> <month> <day> <hour> <minute> <second> <weekday> [0, 0, 0]`. The first
/// three bytes carry volume, CPU and memory load on the official app's timer. This crate sends none
/// of those, the same as that app's packet on connecting.
fn payload(time: &DateTime) -> [u8; 14] {
    let [year_lo, year_hi] = time.year.to_le_bytes();
    [
        0,
        0,
        0,
        year_lo,
        year_hi,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second,
        time.weekday,
        0,
        0,
        0,
    ]
}

/// Sets the keyboard's clock. The keyboard doesn't acknowledge this, so `Ok` only means the report
/// went out. An invalid time sends nothing.
pub fn set_clock<T: Transport>(device: &mut Device<T>, time: &DateTime) -> Result<(), Error> {
    time.validate()?;
    device.send_generic(cmd::CLOCK, 0, &payload(time))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;
    use crate::transport::fake::{FakeEvent, FakeTransport};
    use std::time::Duration;

    /// 2026-10-29 01:24:04, a Thursday.
    fn thursday() -> DateTime {
        DateTime {
            year: 2026,
            month: 10,
            day: 29,
            hour: 1,
            minute: 24,
            second: 4,
            weekday: 4,
        }
    }

    fn device(fake: &FakeTransport) -> Device<FakeTransport> {
        Device::new(fake.clone(), None, model::find(0x0265).unwrap()).unwrap()
    }

    #[track_caller]
    fn assert_invalid(time: DateTime) {
        assert!(
            matches!(time.validate(), Err(Error::Invalid(_))),
            "{time:?}"
        );
    }

    #[test]
    fn payload_has_the_time_and_zero_elsewhere() {
        assert_eq!(
            payload(&thursday()),
            [0, 0, 0, 0xEA, 0x07, 10, 29, 1, 24, 4, 4, 0, 0, 0]
        );
    }

    #[test]
    fn set_clock_sends_one_unacked_report() {
        let fake = FakeTransport::new();
        set_clock(&mut device(&fake), &thursday()).unwrap();
        let events = fake.events();
        assert_eq!(events.len(), 2, "a wait, a send, and no read for an ack");
        assert_eq!(events[0], FakeEvent::Sleep(Duration::from_millis(25)));

        let sent = fake.sent();
        assert_eq!(&sent[0][..8], &[0x09, 0x0B, 0, 0, 1, 0, 14, 0]);
        assert_eq!(&sent[0][8..22], &payload(&thursday()));
        assert!(sent[0][22..].iter().all(|&b| b == 0));
    }

    #[test]
    fn set_clock_uses_the_main_collection() {
        let main = FakeTransport::new();
        let screen = FakeTransport::new();
        let model = model::find(0x01B8).unwrap();
        let mut device = Device::new(main.clone(), Some(screen.clone()), model).unwrap();
        set_clock(&mut device, &thursday()).unwrap();
        assert_eq!(main.sent().len(), 1);
        assert!(screen.sent().is_empty());
    }

    #[test]
    fn an_invalid_time_sends_nothing() {
        let fake = FakeTransport::new();
        let time = DateTime {
            month: 13,
            ..thursday()
        };
        let result = set_clock(&mut device(&fake), &time);
        assert!(matches!(result, Err(Error::Invalid(_))));
        assert!(fake.events().is_empty());
    }

    #[test]
    fn set_clock_passes_on_transport_errors() {
        let fake = FakeTransport::new();
        (0..3).for_each(|n| fake.fail_send(n));
        let result = set_clock(&mut device(&fake), &thursday());
        assert!(matches!(result, Err(Error::Hid(_))));
    }

    #[test]
    fn each_field_is_checked() {
        let t = thursday;
        assert!(t().validate().is_ok());
        assert_invalid(DateTime { year: 0, ..t() });
        assert_invalid(DateTime { month: 0, ..t() });
        assert_invalid(DateTime { month: 13, ..t() });
        assert_invalid(DateTime { day: 0, ..t() });
        assert_invalid(DateTime { day: 32, ..t() });
        assert_invalid(DateTime { hour: 24, ..t() });
        assert_invalid(DateTime { minute: 60, ..t() });
        assert_invalid(DateTime { second: 60, ..t() });
        assert_invalid(DateTime { weekday: 7, ..t() });
    }

    #[test]
    fn the_limits_themselves_are_fine() {
        let late = DateTime {
            year: u16::MAX,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            weekday: 6,
        };
        assert!(late.validate().is_ok());
        let early = DateTime {
            year: 1,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            weekday: 0,
        };
        assert!(early.validate().is_ok());
    }

    #[test]
    fn month_lengths_count_leap_years() {
        let date = |year, month, day| DateTime {
            year,
            month,
            day,
            ..thursday()
        };
        assert!(date(2026, 4, 30).validate().is_ok());
        assert_invalid(date(2026, 4, 31));
        assert!(date(2026, 2, 28).validate().is_ok());
        assert_invalid(date(2026, 2, 29));
        assert!(date(2028, 2, 29).validate().is_ok());
        assert_invalid(date(2028, 2, 30));
        // Centuries are leap years only when divisible by 400.
        assert_invalid(date(2100, 2, 29));
        assert!(date(2000, 2, 29).validate().is_ok());
    }
}
