//! The computer's local time, as the keyboard's clock takes it.

use std::ops::Add;
use std::time::Duration;

use chrono::{Datelike, Local, TimeDelta, Timelike};
use unkludge_protocol::clock::{DateTime, set_clock_after};
use unkludge_protocol::transport::Transport;
use unkludge_protocol::{Device, Error};

/// The time now.
pub fn device_now() -> Result<DateTime, Error> {
    device_time(&Local::now())
}

/// The next whole second, and how long until then. A time that's already a whole second is its own.
pub fn next_whole_second<T: Timelike + Add<TimeDelta, Output = T>>(now: T) -> (Duration, T) {
    const SECOND: u32 = 1_000_000_000;

    // During a leap second, this runs past a whole second.
    let past = now.nanosecond() % SECOND;
    if past == 0 {
        return (Duration::ZERO, now);
    }
    let left = SECOND - past;
    (
        Duration::from_nanos(left.into()),
        now + TimeDelta::nanoseconds(left.into()),
    )
}

/// Sets the keyboard's clock as the next second starts, and says to what. The clock only takes whole
/// seconds and counts on from when it's told, so sending the second that's under way would leave it
/// behind by however much of that second has gone, up to all of it. This takes up to a second.
pub fn set_clock_to_next_second<T: Transport>(
    device: &mut Device<T>,
) -> Result<chrono::DateTime<Local>, Error> {
    let (wait, time) = next_whole_second(Local::now());
    set_clock_after(device, wait, &device_time(&time)?)?;
    Ok(time)
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
    use chrono::{NaiveDate, NaiveDateTime};
    use unkludge_protocol::model;
    use unkludge_protocol::transport::fake::FakeTransport;

    fn at(day: u32, hour: u32, minute: u32, second: u32, nanos: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, day)
            .unwrap()
            .and_hms_nano_opt(hour, minute, second, nanos)
            .unwrap()
    }

    #[test]
    fn the_next_second_comes_after_what_is_left_of_this_one() {
        assert_eq!(
            next_whole_second(at(29, 1, 24, 7, 250_000_000)),
            (Duration::from_millis(750), at(29, 1, 24, 8, 0))
        );
    }

    #[test]
    fn the_next_second_can_be_in_the_next_minute() {
        assert_eq!(
            next_whole_second(at(29, 1, 24, 59, 999_000_000)),
            (Duration::from_millis(1), at(29, 1, 25, 0, 0))
        );
    }

    #[test]
    fn the_next_second_can_be_in_the_next_day_with_its_weekday() {
        // 2026-10-31 was a Saturday, and the next day a Sunday.
        let (wait, time) = next_whole_second(at(31, 23, 59, 59, 500_000_000));
        assert_eq!(wait, Duration::from_millis(500));
        let sent = device_time(&time).unwrap();
        assert_eq!((sent.month, sent.day, sent.hour), (11, 1, 0));
        assert_eq!(sent.weekday, 0);
    }

    #[test]
    fn a_time_that_is_already_a_whole_second_doesnt_wait() {
        let now = at(29, 1, 24, 7, 0);
        assert_eq!(next_whole_second(now), (Duration::ZERO, now));
    }

    #[test]
    fn setting_the_clock_waits_and_sends_the_whole_second_it_says() {
        let fake = FakeTransport::new();
        let mut device = Device::new(fake.clone(), None, model::find(0x0265).unwrap()).unwrap();
        let before = Local::now();
        let time = set_clock_to_next_second(&mut device).unwrap();

        assert_eq!(time.nanosecond(), 0);
        // Two, as a second may have started between the two looks at the time.
        assert!(time >= before && time - before < TimeDelta::seconds(2));
        assert!(fake.sleeps()[0] <= Duration::from_secs(1));
        assert_eq!(fake.sent()[0][17], time.second() as u8);
    }

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
