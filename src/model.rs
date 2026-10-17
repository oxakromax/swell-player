//! The compact track record lists, the queue and the cache share.
//!
//! TIDAL's track object carries every field the API returns; a library of a
//! few thousand tracks only needs these.

use serde::{Deserialize, Serialize};

use crate::tidal::models::Track;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistLink {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackInfo {
    pub id: u64,
    /// Title with the version appended, e.g. "Song (Live)".
    pub title: String,
    pub artists: Vec<ArtistLink>,
    pub album_id: u64,
    pub album: String,
    /// Image id of the album cover, if it has one.
    pub cover: Option<String>,
    /// Seconds.
    pub duration: u32,
    pub explicit: bool,
    /// Available as Hi-Res Lossless (24-bit).
    pub hires: bool,
    /// Available in Dolby Atmos.
    pub atmos: bool,
    pub track_number: u32,
    pub volume_number: u32,
    /// Streamable in the user's country.
    pub available: bool,
    /// When it was added to the list it came from (favourites, playlists),
    /// as TIDAL's ISO 8601 string.
    pub added: Option<String>,
}

impl TrackInfo {
    /// "Artist A, Artist B".
    pub fn artist_names(&self) -> String {
        self.artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn main_artist(&self) -> Option<&ArtistLink> {
        self.artists.first()
    }

    /// TIDAL's public web link, for "Copy link".
    pub fn share_url(&self) -> String {
        format!("https://tidal.com/browse/track/{}", self.id)
    }
}

impl From<Track> for TrackInfo {
    fn from(track: Track) -> Self {
        let title = match track.version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(version) if !track.title.contains(version) => {
                format!("{} ({version})", track.title)
            }
            _ => track.title,
        };
        let tags = &track.media_metadata.tags;
        Self {
            id: track.id,
            title,
            artists: track
                .artists
                .into_iter()
                .map(|artist| ArtistLink {
                    id: artist.id,
                    name: artist.name,
                })
                .collect(),
            album_id: track.album.id,
            album: track.album.title,
            cover: track.album.cover,
            duration: track.duration,
            explicit: track.explicit,
            hires: tags.iter().any(|tag| tag == "HIRES_LOSSLESS"),
            atmos: tags.iter().any(|tag| tag == "DOLBY_ATMOS"),
            track_number: track.track_number,
            volume_number: track.volume_number.max(1),
            available: track.stream_ready && track.allow_streaming,
            added: track.date_added,
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
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    #[test]
    fn version_not_repeated() {
        let track: Track = serde_json::from_str(
            r#"{"id":1,"title":"Song (Live)","version":"Live","artists":[{"id":2,"name":"A"}]}"#,
        )
        .unwrap();
        let info = TrackInfo::from(track);
        assert_eq!(info.title, "Song (Live)");
        assert_eq!(info.artist_names(), "A");
        assert!(info.available);
    }
}
