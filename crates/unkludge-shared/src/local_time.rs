//! The computer's local time, as the keyboard's clock takes it.

use chrono::{Datelike, Timelike};
use unkludge_protocol::Error;
use unkludge_protocol::clock::DateTime;

/// The time now.
pub fn device_now() -> Result<DateTime, Error> {
    device_time(&chrono::Local::now())
}

/// A local time, as the keyboard's clock takes it.
pub fn device_time<T: Datelike + Timelike>(time: &T) -> Result<DateTime, Error> {
    let year = u16::try_from(time.year())
        .map_err(|_| Error::Invalid(format!("the year {} doesn't fit", time.year())))?;
    // The rest are small enough for their fields, and `set_clock` checks them all.
    Ok(DateTime {
        year,
        month: time.month() as u8,
        day: time.day() as u8,
        hour: time.hour() as u8,
        minute: time.minute() as u8,
        second: time.second() as u8,
        weekday: time.weekday().num_days_from_sunday() as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn device_time_takes_the_fields_of_a_local_time() {
        // 2026-10-29 was a Thursday.
        let time = NaiveDate::from_ymd_opt(2026, 10, 29)
            .unwrap()
            .and_hms_opt(1, 24, 7)
            .unwrap();
        assert_eq!(
            device_time(&time).unwrap(),
            DateTime {
                year: 2026,
                month: 10,
                day: 29,
                hour: 1,
                minute: 24,
                second: 7,
                weekday: 4,
            }
        );
        let sunday = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(device_time(&sunday).unwrap().weekday, 0);
    }

    #[test]
    fn a_year_that_doesnt_fit_is_an_error() {
        let time = NaiveDate::from_ymd_opt(70000, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert!(device_time(&time).is_err());
    }
}
