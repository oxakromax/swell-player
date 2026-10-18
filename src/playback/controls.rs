//! The desktop's media controls: media keys, the shell's or lock screen's
//! player widget. [`MediaControls`] is MPRIS on Linux and the BSDs
//! (`mpris.rs`) and the system media controls on Windows (SMTC) and macOS
//! (Now Playing) (`os_controls.rs`). Both read a [`Snapshot`] the main
//! thread keeps current and send what the user asks for back as
//! [`Command`]s; the `Playback` facade does the rest.

// The OS controls (Windows, macOS) have no shuffle, repeat or volume, and
// say nothing about what is possible.
#![cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]

use std::time::Instant;

use crate::settings::Repeat;

#[cfg(all(unix, not(target_os = "macos")))]
pub use super::mpris::Mpris as MediaControls;
#[cfg(any(windows, target_os = "macos"))]
pub use super::os_controls::OsControls as MediaControls;

/// A request from a media controller, for the main thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative, in seconds.
    Seek(f64),
    /// Absolute, in seconds, if `track_id` is still current.
    SetPosition {
        track_id: u64,
        position: f64,
    },
    Volume(f64),
    Shuffle(bool),
    Repeat(Repeat),
    /// A `tidal://` or TIDAL web link to open in the app.
    OpenUri(String),
    Raise,
    Quit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub track_id: u64,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    /// Seconds.
    pub length: f64,
    pub art_url: Option<String>,
    pub url: String,
    pub track_number: u32,
    pub disc_number: u32,
}

/// Everything the media controls can show.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: Status,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Slider position 0–1.
    pub volume: f64,
    pub track: Option<Metadata>,
    /// Seconds, as of `position_at` (advancing from then while playing).
    pub position: f64,
    pub position_at: Option<Instant>,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_seek: bool,
    pub can_raise: bool,
    pub can_quit: bool,
}

impl Snapshot {
    /// The position now, in seconds: advanced since `position_at` while
    /// playing, within the track.
    pub fn position_now(&self) -> f64 {
        let mut position = self.position;
        if self.status == Status::Playing
            && let Some(at) = self.position_at
        {
            position += at.elapsed().as_secs_f64();
        }
        if let Some(track) = &self.track
            && track.length > 0.0
        {
            position = position.min(track.length);
        }
        position.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_extrapolates_while_playing() {
        let snapshot = Snapshot {
            status: Status::Paused,
            position: 10.0,
            position_at: Some(Instant::now() - std::time::Duration::from_secs(5)),
            ..Snapshot::default()
        };
        assert_eq!(snapshot.position_now(), 10.0);
        let playing = Snapshot {
            status: Status::Playing,
            ..snapshot
        };
        assert!(playing.position_now() >= 15.0);
        let ended = Snapshot {
            track: Some(Metadata {
                length: 12.0,
                ..Metadata::default()
            }),
            ..playing
        };
        assert_eq!(ended.position_now(), 12.0);
    }
}
