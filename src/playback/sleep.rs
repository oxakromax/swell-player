//! The sleep timer: pause after a while, or at the end of the track.

use std::time::{Duration, Instant};

#[allow(dead_code)] // Public API for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepTimer {
    /// Pause once this much time has passed (wall clock, from when it was
    /// set).
    After(Duration),
    /// Stop when the current track ends; the next one is then ready,
    /// paused at its start.
    EndOfTrack,
}

/// A running timer.
pub(super) struct Armed {
    pub timer: SleepTimer,
    /// When [`SleepTimer::After`] fires.
    pub deadline: Option<Instant>,
    /// Whole minutes left last time subscribers were told.
    pub announced: Option<u64>,
}

impl Armed {
    pub fn new(timer: SleepTimer, now: Instant) -> Self {
        let deadline = match timer {
            SleepTimer::After(duration) => Some(now + duration),
            SleepTimer::EndOfTrack => None,
        };
        Self {
            timer,
            deadline,
            announced: None,
        }
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.deadline.map(|d| d.saturating_duration_since(now))
    }
}

/// Minutes as shown ("23 min"): rounded up, so it reads 1 until the end.
pub fn minutes_left(remaining: Duration) -> u64 {
    remaining.as_secs().div_ceil(60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown() {
        let now = Instant::now();
        let armed = Armed::new(SleepTimer::After(Duration::from_secs(15 * 60)), now);
        assert_eq!(armed.remaining(now), Some(Duration::from_secs(900)));
        assert_eq!(
            armed.remaining(now + Duration::from_secs(2000)),
            Some(Duration::ZERO)
        );
        assert_eq!(minutes_left(Duration::from_secs(900)), 15);
        assert_eq!(minutes_left(Duration::from_secs(61)), 2);
        assert_eq!(minutes_left(Duration::from_secs(1)), 1);
        assert_eq!(minutes_left(Duration::ZERO), 0);
        assert_eq!(Armed::new(SleepTimer::EndOfTrack, now).remaining(now), None);
    }
}
