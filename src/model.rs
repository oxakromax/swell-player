//! The compact track record the UI keeps for every list row.
//!
//! `tidalrs::Track` carries every field the API returns; a library of a few
//! thousand tracks only needs these.

use tidalrs::Track;

#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub id: u64,
    pub title: String,
    pub artists: String,
    pub album: String,
    /// Seconds.
    pub duration: u32,
    /// Image id of the album cover, if it has one.
    pub cover: Option<String>,
    pub hires: bool,
}

impl From<Track> for TrackInfo {
    fn from(track: Track) -> Self {
        let title = match track.version.as_deref().filter(|v| !v.is_empty()) {
            Some(version) => format!("{} ({version})", track.title),
            None => track.title,
        };
        let artists = track
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let hires = track
            .media_metadata
            .is_some_and(|meta| meta.tags.iter().any(|tag| tag == "HIRES_LOSSLESS"));
        Self {
            id: track.id,
            title,
            artists,
            album: track.album.title,
            duration: track.duration,
            cover: track.album.cover,
            hires,
        }
    }
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn format_duration(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3725), "1:02:05");
    }
}
