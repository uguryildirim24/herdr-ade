//! Wall timestamps remain evidence. Deadline arithmetic subtracts persisted
//! sleep intervals, measured against the host's sleep-stopping uptime clock.
use std::{cell::RefCell, path::Path};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct Clock {
    wall: i64,
    uptime: u64,
    sleeps: Vec<(i64, i64)>,
}

thread_local! {
    static CURRENT: RefCell<Clock> = RefCell::new(Clock::default());
    #[cfg(test)]
    static SAMPLE: std::cell::Cell<Option<(i64, u64)>> = const { std::cell::Cell::new(None) };
}

pub(crate) struct Scope(Clock);
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|clock| *clock.borrow_mut() = self.0.clone());
    }
}

impl Clock {
    /// Return whether sleep occurred since the last sample, including across
    /// a ticker restart. Uptime is host-wide, not process-relative.
    fn sample(&mut self, wall: i64, uptime: u64) -> bool {
        let mut slept = false;
        if self.wall != 0 && uptime >= self.uptime && wall >= self.wall {
            let awake = (uptime - self.uptime) as i64;
            let gap = wall - self.wall - awake;
            // Allow clock resolution / sampling jitter, not a polling window.
            if gap > 2 {
                self.sleeps.push((self.wall + awake, wall));
                slept = true;
            }
        }
        self.wall = wall;
        self.uptime = uptime;
        slept
    }

    fn elapsed(&self, then: i64, now: i64) -> i64 {
        now - then
            - self
                .sleeps
                .iter()
                .map(|&(start, end)| (end.min(now) - start.max(then)).max(0))
                .sum::<i64>()
    }
}

/// The ticker owns the file; command invocations only read it and sample for
/// their own deadline checks. No lane timestamps are rewritten on wake.
pub(crate) fn enter(root: &Path, persist: bool) -> Result<(Scope, bool)> {
    let path = root.join(".ticker-awake.json");
    let mut clock: Clock = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Clock::default(),
        Err(error) => return Err(error.into()),
    };
    let (wall, uptime) = sample();
    let slept = clock.sample(wall, uptime);
    if persist {
        crate::project::write_json(&path, &clock)?;
    }
    let old = CURRENT.with(|current| current.replace(clock));
    Ok((Scope(old), slept))
}

#[cfg(test)]
pub(crate) fn set_sample(sample: Option<(i64, u64)>) {
    SAMPLE.with(|value| value.set(sample));
}

fn sample() -> (i64, u64) {
    let sample = (jiff::Timestamp::now().as_second(), uptime());
    #[cfg(test)]
    let sample = SAMPLE.with(|value| value.get()).unwrap_or(sample);
    sample
}

pub(crate) fn elapsed(then: jiff::Timestamp, now: jiff::Timestamp) -> i64 {
    CURRENT.with(|clock| {
        let mut clock = clock.borrow_mut();
        if clock.wall != 0 {
            // A slow pass itself can span lid close. Sample at judgment too;
            // the next pass persists the gap from the durable prior sample.
            let (wall, uptime) = sample();
            clock.sample(wall, uptime);
        }
        clock.elapsed(then.as_second(), now.as_second())
    })
}

#[cfg(target_os = "macos")]
fn uptime() -> u64 {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase { numer: 0, denom: 0 };
    // SAFETY: mach_timebase_info writes a correctly sized initialized struct.
    unsafe {
        assert_eq!(mach_timebase_info(&mut info), 0);
        (u128::from(mach_absolute_time()) * u128::from(info.numer)
            / u128::from(info.denom)
            / 1_000_000_000) as u64
    }
}

#[cfg(not(target_os = "macos"))]
fn uptime() -> u64 {
    #[repr(C)]
    struct Timespec {
        seconds: std::os::raw::c_long,
        nanos: std::os::raw::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    let mut time = Timespec {
        seconds: 0,
        nanos: 0,
    };
    // SAFETY: CLOCK_MONOTONIC (Linux) writes this native timespec. Like the
    // Mac clock it excludes suspend and survives process restarts.
    unsafe {
        assert_eq!(clock_gettime(1, &mut time), 0);
    }
    time.seconds as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forty_minute_sleep_with_dark_wakes_survives_restart() {
        let mut clock = Clock::default();
        clock.sample(10_000, 100);
        for n in 1..=8 {
            assert!(clock.sample(10_000 + n * 300, 100 + n as u64 * 5));
            // Simulate launchd replacing the ticker during a dark wake.
            clock = serde_json::from_slice(&serde_json::to_vec(&clock).unwrap()).unwrap();
            assert_eq!(clock.elapsed(10_000, 10_000 + n * 300), n * 5);
        }
        assert!(!clock.sample(12_415, 155));
        assert_eq!(clock.elapsed(10_000, 12_415), 55);
        // A timestamp written by a box during sleep is clipped, not aged by
        // the entire sleep interval or moved into the future.
        assert_eq!(clock.elapsed(12_200, 12_415), 15);
    }
}
