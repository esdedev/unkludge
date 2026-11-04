//! Noticing that the computer has been asleep. The kernel's boot clock counts time asleep and its
//! monotonic clock doesn't, so the difference between them only ever grows by that. Neither is touched
//! by the time being set, so unlike comparing with the wall clock this can't be fooled by that. Asking
//! logind instead would take listening on the system bus, for something this does in a few lines.

use std::time::Duration;

use libc::{CLOCK_BOOTTIME, CLOCK_MONOTONIC, clock_gettime, timespec};

/// How often to look. The clock being set this long after waking up is soon enough.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Less than this is the clocks not being read at quite the same moment.
const MIN_SLEEP: Duration = Duration::from_secs(1);

fn clock(id: libc::clockid_t) -> Option<Duration> {
    let mut time = timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is valid for the call, which only writes to it.
    let result = unsafe { clock_gettime(id, &raw mut time) };
    (result == 0).then(|| Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}

/// How long the computer has been asleep since it started.
pub fn time_asleep() -> Option<Duration> {
    Some(clock(CLOCK_BOOTTIME)?.saturating_sub(clock(CLOCK_MONOTONIC)?))
}

/// Watches [`time_asleep`] for it growing.
#[derive(Default)]
pub struct SleepWatch {
    seen: Option<Duration>,
}

impl SleepWatch {
    /// Takes the time asleep so far, and says whether that means it has just woken up.
    pub fn woke_up(&mut self, asleep: Option<Duration>) -> bool {
        let Some(asleep) = asleep else {
            return false;
        };
        let Some(seen) = self.seen else {
            self.seen = Some(asleep);
            return false;
        };
        if asleep.saturating_sub(seen) < MIN_SLEEP {
            return false;
        }
        self.seen = Some(asleep);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(value: f32) -> Option<Duration> {
        Some(Duration::from_secs_f32(value))
    }

    #[test]
    fn the_first_look_is_only_where_it_starts() {
        assert!(!SleepWatch::default().woke_up(secs(5000.0)));
    }

    #[test]
    fn time_asleep_growing_means_it_woke_up_once() {
        let mut watch = SleepWatch::default();
        assert!(!watch.woke_up(secs(10.0)));
        assert!(!watch.woke_up(secs(10.0)));
        assert!(watch.woke_up(secs(70.0)));
        assert!(!watch.woke_up(secs(70.0)));
    }

    #[test]
    fn the_clocks_being_read_a_moment_apart_isnt_sleeping() {
        let mut watch = SleepWatch::default();
        assert!(!watch.woke_up(secs(10.0)));
        assert!(!watch.woke_up(secs(10.01)));
        assert!(!watch.woke_up(secs(9.99)));
    }

    #[test]
    fn small_differences_dont_add_up_to_a_sleep_by_themselves_unless_they_get_large() {
        let mut watch = SleepWatch::default();
        assert!(!watch.woke_up(secs(10.0)));
        assert!(!watch.woke_up(secs(10.5)));
        assert!(watch.woke_up(secs(11.5)));
    }

    #[test]
    fn a_clock_that_cant_be_read_is_no_news() {
        let mut watch = SleepWatch::default();
        assert!(!watch.woke_up(None));
        assert!(!watch.woke_up(secs(10.0)));
        assert!(!watch.woke_up(None));
        assert!(watch.woke_up(secs(20.0)));
    }

    #[test]
    fn these_clocks_can_be_read_here() {
        assert!(time_asleep().is_some());
    }
}
