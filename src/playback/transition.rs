//! How one track gives way to the next when it ends by itself: a
//! crossfade, or the gapless handover. Pure rules, unit-tested.
//!
//! A crossfade happens only when all of these hold:
//! - the Crossfade preference is on (1–12 s);
//! - the output is not bit-perfect: Volume Normalisation is on. Mixing two
//!   tracks changes the samples, which bit-perfect output promises not to;
//! - the next track does not continue the album: consecutive tracks of the
//!   same album (the next track number, or the first track of the next
//!   disc) play gaplessly, so live albums, DJ mixes and segued concept
//!   albums keep their seamless joins. When track numbers are unknown, any
//!   two tracks of the same album count as consecutive;
//! - both tracks are long enough: the crossfade is at most a third of
//!   either, and below one second it is not worth it.
//!
//! Skipping, going back and jumping never crossfade: they get the short
//! fade of [`super::fade::MANUAL_FADE`] instead.

use std::time::Duration;

use crate::model::TrackInfo;

/// The longest crossfade offered in Preferences, in seconds.
pub const MAX_CROSSFADE_SECS: u32 = 12;
/// Shorter crossfades are not worth it: the switch is gapless instead.
const MIN_CROSSFADE: f64 = 1.0;

/// What the preferences say about transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Crossfade length in seconds; 0 is off.
    pub crossfade_secs: u32,
    /// The output is bit-perfect (no normalisation): never crossfade.
    pub bit_perfect: bool,
}

impl Rules {
    /// Whether crossfading can happen at all with these preferences.
    pub fn crossfade_enabled(self) -> bool {
        self.crossfade_secs > 0 && !self.bit_perfect
    }
}

/// The crossfade from `current` (lasting `current_duration` seconds) into
/// `next`, or `None` for a gapless switch.
pub fn crossfade(
    rules: Rules,
    current: &TrackInfo,
    current_duration: f64,
    next: &TrackInfo,
) -> Option<Duration> {
    if !rules.crossfade_enabled() || continues_album(current, next) {
        return None;
    }
    let next_duration = f64::from(next.duration);
    let seconds = f64::from(rules.crossfade_secs.min(MAX_CROSSFADE_SECS))
        .min(current_duration / 3.0)
        .min(next_duration / 3.0);
    (seconds >= MIN_CROSSFADE).then(|| Duration::from_secs_f64(seconds))
}

/// `next` is the track that follows `current` on its album.
pub fn continues_album(current: &TrackInfo, next: &TrackInfo) -> bool {
    if current.album_id == 0 || current.album_id != next.album_id || current.id == next.id {
        return false;
    }
    if current.track_number == 0 || next.track_number == 0 {
        return true;
    }
    let (disc, number) = (current.volume_number.max(1), current.track_number);
    let (next_disc, next_number) = (next.volume_number.max(1), next.track_number);
    (next_disc == disc && next_number == number + 1) || (next_disc == disc + 1 && next_number == 1)
}

/// Seconds before the end of the current track at which the crossfade
/// starts; the next track is prepared (pre-rolled, paused) this much
/// earlier still.
pub fn crossfade_start(duration: f64, crossfade: Duration) -> f64 {
    (duration - crossfade.as_secs_f64()).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u64, album_id: u64, disc: u32, number: u32) -> TrackInfo {
        TrackInfo {
            id,
            album_id,
            volume_number: disc,
            track_number: number,
            duration: 240,
            available: true,
            ..TrackInfo::default()
        }
    }

    const ON: Rules = Rules {
        crossfade_secs: 6,
        bit_perfect: false,
    };

    #[test]
    fn off_and_bit_perfect_never_crossfade() {
        let (a, b) = (track(1, 10, 1, 1), track(2, 20, 1, 1));
        assert_eq!(crossfade(ON, &a, 240.0, &b), Some(Duration::from_secs(6)));
        let off = Rules {
            crossfade_secs: 0,
            ..ON
        };
        assert_eq!(crossfade(off, &a, 240.0, &b), None);
        assert!(!off.crossfade_enabled());
        let bit_perfect = Rules {
            bit_perfect: true,
            ..ON
        };
        assert_eq!(crossfade(bit_perfect, &a, 240.0, &b), None);
        assert!(!bit_perfect.crossfade_enabled());
        assert!(ON.crossfade_enabled());
    }

    #[test]
    fn consecutive_album_tracks_stay_gapless() {
        let a = track(1, 10, 1, 4);
        assert!(continues_album(&a, &track(2, 10, 1, 5)));
        assert_eq!(crossfade(ON, &a, 240.0, &track(2, 10, 1, 5)), None);
        // The first track of the next disc continues it too.
        assert!(continues_album(&track(1, 10, 1, 12), &track(2, 10, 2, 1)));
        // Same album, not in order (shuffle, a playlist): crossfade.
        assert!(!continues_album(&a, &track(3, 10, 1, 9)));
        assert!(!continues_album(&a, &track(3, 10, 1, 3)));
        assert!(crossfade(ON, &a, 240.0, &track(3, 10, 1, 9)).is_some());
        // Another album, even with the next number.
        assert!(!continues_album(&a, &track(4, 11, 1, 5)));
        // Unknown album: no rule applies.
        assert!(!continues_album(&track(5, 0, 1, 1), &track(6, 0, 1, 2)));
        // Unknown numbers on the same album: assume it continues.
        assert!(continues_album(&track(7, 10, 0, 0), &track(8, 10, 0, 0)));
        // Disc 0 (unknown) is disc 1.
        assert!(continues_album(&track(7, 10, 0, 3), &track(8, 10, 1, 4)));
        // Repeat-one: the same track again is not "the next one".
        assert!(!continues_album(&a, &a));
    }

    #[test]
    fn short_tracks_shorten_the_crossfade() {
        let rules = Rules {
            crossfade_secs: 12,
            ..ON
        };
        let a = track(1, 10, 1, 1);
        let mut b = track(2, 20, 1, 1);
        assert_eq!(
            crossfade(rules, &a, 240.0, &b),
            Some(Duration::from_secs(12))
        );
        assert_eq!(crossfade(rules, &a, 15.0, &b), Some(Duration::from_secs(5)));
        b.duration = 9;
        assert_eq!(
            crossfade(rules, &a, 240.0, &b),
            Some(Duration::from_secs(3))
        );
        b.duration = 2;
        assert_eq!(crossfade(rules, &a, 240.0, &b), None);
        // Out-of-range settings are capped.
        let huge = Rules {
            crossfade_secs: 60,
            ..ON
        };
        assert_eq!(
            crossfade(huge, &a, 600.0, &track(3, 30, 1, 1)),
            Some(Duration::from_secs(12))
        );
    }

    #[test]
    fn crossfade_starts_before_the_end() {
        assert_eq!(crossfade_start(240.0, Duration::from_secs(6)), 234.0);
        assert_eq!(crossfade_start(3.0, Duration::from_secs(6)), 0.0);
    }
}
