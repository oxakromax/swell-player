//! Volume ramps for fades and crossfades, as pure arithmetic.
//!
//! A ramp moves a deck's fade factor (an amplitude multiplier, applied on
//! top of the user's volume) from one value to another over a duration:
//! - [`Shape::Smooth`], a raised cosine, for the short anti-click fades of
//!   pausing, seeking and skipping;
//! - [`Shape::EqualPower`] for crossfades: the outgoing track follows
//!   `cos(p·π/2)` and the incoming one `sin(p·π/2)`, so the summed power of
//!   two uncorrelated tracks stays constant and the middle does not dip.

use std::f64::consts::FRAC_PI_2;
use std::time::{Duration, Instant};

/// The short fade of manual changes (pause, resume, seek, skip): long
/// enough to avoid the click of cutting a waveform mid-cycle, short enough
/// not to be heard as a fade.
pub const MANUAL_FADE: Duration = Duration::from_millis(180);
/// The fade-out before a seek, kept shorter so seeking stays snappy.
pub const SEEK_FADE: Duration = Duration::from_millis(70);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Raised cosine: starts and ends without a corner.
    Smooth,
    /// Constant power when an `EqualPower` ramp up and one down overlap.
    EqualPower,
}

#[derive(Clone, Copy, Debug)]
pub struct Ramp {
    from: f64,
    to: f64,
    start: Instant,
    length: Duration,
    shape: Shape,
}

impl Ramp {
    /// From `from` to `to` (fade factors, 0–1), starting at `start`. The
    /// length is scaled by how far there is to go, so reversing a fade
    /// half-way (resuming while the pause fades out) takes half as long.
    pub fn new(from: f64, to: f64, start: Instant, full_length: Duration, shape: Shape) -> Self {
        let from = from.clamp(0.0, 1.0);
        let to = to.clamp(0.0, 1.0);
        let length = match shape {
            Shape::Smooth => full_length.mul_f64((to - from).abs()),
            // Crossfades always take the time asked for.
            Shape::EqualPower => full_length,
        };
        Self {
            from,
            to,
            start,
            length,
            shape,
        }
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    /// 0 at the start, 1 once done.
    pub fn progress(&self, now: Instant) -> f64 {
        if self.length.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.start);
        (elapsed.as_secs_f64() / self.length.as_secs_f64()).clamp(0.0, 1.0)
    }

    pub fn is_done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }

    /// The fade factor at `now`.
    pub fn value(&self, now: Instant) -> f64 {
        let p = self.progress(now);
        let (from, to) = (self.from, self.to);
        if p >= 1.0 {
            return to;
        }
        match self.shape {
            Shape::Smooth => from + (to - from) * smooth(p),
            Shape::EqualPower if to >= from => from + (to - from) * (p * FRAC_PI_2).sin(),
            Shape::EqualPower => to + (from - to) * (p * FRAC_PI_2).cos(),
        }
    }
}

/// Raised cosine from 0 to 1 over `p` in 0–1.
pub fn smooth(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    (1.0 - (p * std::f64::consts::PI).cos()) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn smooth_ramp_ends_where_asked() {
        let start = Instant::now();
        let ramp = Ramp::new(1.0, 0.0, start, Duration::from_millis(200), Shape::Smooth);
        assert_eq!(ramp.value(start), 1.0);
        assert!((ramp.value(at(start, 100)) - 0.5).abs() < 1e-9);
        assert_eq!(ramp.value(at(start, 200)), 0.0);
        assert_eq!(ramp.value(at(start, 5000)), 0.0);
        assert!(ramp.is_done(at(start, 200)));
        assert!(!ramp.is_done(at(start, 199)));
        // Monotonic, without a corner at either end.
        let values: Vec<f64> = (0..=200).map(|ms| ramp.value(at(start, ms))).collect();
        assert!(values.windows(2).all(|w| w[1] <= w[0]));
        assert!(1.0 - values[1] < 1e-3);
        assert!(values[199] < 1e-3);
    }

    #[test]
    fn reversing_half_way_takes_half_as_long() {
        let start = Instant::now();
        let ramp = Ramp::new(0.5, 1.0, start, Duration::from_millis(200), Shape::Smooth);
        assert!(ramp.is_done(at(start, 100)));
        assert!(!ramp.is_done(at(start, 99)));
        let none = Ramp::new(1.0, 1.0, start, Duration::from_millis(200), Shape::Smooth);
        assert!(none.is_done(start));
        assert_eq!(none.value(start), 1.0);
    }

    #[test]
    fn crossfade_keeps_constant_power() {
        let start = Instant::now();
        let length = Duration::from_secs(6);
        let out = Ramp::new(1.0, 0.0, start, length, Shape::EqualPower);
        let inc = Ramp::new(0.0, 1.0, start, length, Shape::EqualPower);
        for ms in (0..=6000).step_by(250) {
            let now = at(start, ms);
            let power = out.value(now).powi(2) + inc.value(now).powi(2);
            assert!((power - 1.0).abs() < 1e-9, "{ms} ms: {power}");
        }
        assert_eq!(out.value(at(start, 6000)), 0.0);
        assert!((inc.value(at(start, 6000)) - 1.0).abs() < 1e-12);
        // Half-way both are at −3 dB.
        let half = std::f64::consts::FRAC_1_SQRT_2;
        assert!((out.value(at(start, 3000)) - half).abs() < 1e-9);
        assert!((inc.value(at(start, 3000)) - half).abs() < 1e-9);
    }

    #[test]
    fn values_are_clamped() {
        let start = Instant::now();
        let ramp = Ramp::new(2.0, -1.0, start, Duration::from_millis(10), Shape::Smooth);
        assert_eq!(ramp.value(start), 1.0);
        assert_eq!(ramp.target(), 0.0);
        assert_eq!(smooth(-1.0), 0.0);
        assert_eq!(smooth(2.0), 1.0);
    }
}
