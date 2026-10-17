//! The queue saved between runs, in `$XDG_STATE_HOME/swell-player/`:
//! `queue.json` (the queue, source and position, written when the queue
//! changes and at quit) and `position.json` (just the position, written
//! every few seconds while playing, so a long queue is not rewritten that
//! often).

use std::path::{Path, PathBuf};

use anyhow::Result;
use gtk::glib;
use serde::{Deserialize, Serialize};

use super::Source;
use super::queue::QueueSnapshot;
use crate::tidal::pages::Target;

const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub version: u32,
    pub queue: QueueSnapshot,
    pub source: Option<SavedSource>,
    /// Seconds into the current track.
    pub position: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedPosition {
    pub track_id: u64,
    pub position: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSource {
    pub title: String,
    pub target: Option<SavedTarget>,
}

/// [`Target`], serialisable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum SavedTarget {
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
    Track(u64),
    Page(String),
    Collection(String),
    Url(String),
}

impl From<&Source> for SavedSource {
    fn from(source: &Source) -> Self {
        Self {
            title: source.title.clone(),
            target: source.target.as_ref().map(|target| match target {
                Target::Album(id) => SavedTarget::Album(*id),
                Target::Artist(id) => SavedTarget::Artist(*id),
                Target::Playlist(id) => SavedTarget::Playlist(id.clone()),
                Target::Mix(id) => SavedTarget::Mix(id.clone()),
                Target::Track(id) => SavedTarget::Track(*id),
                Target::Page(path) => SavedTarget::Page(path.clone()),
                Target::Collection(name) => SavedTarget::Collection(name.clone()),
                Target::Url(url) => SavedTarget::Url(url.clone()),
            }),
        }
    }
}

impl From<SavedSource> for Source {
    fn from(source: SavedSource) -> Self {
        Self {
            title: source.title,
            target: source.target.map(|target| match target {
                SavedTarget::Album(id) => Target::Album(id),
                SavedTarget::Artist(id) => Target::Artist(id),
                SavedTarget::Playlist(id) => Target::Playlist(id),
                SavedTarget::Mix(id) => Target::Mix(id),
                SavedTarget::Track(id) => Target::Track(id),
                SavedTarget::Page(path) => Target::Page(path),
                SavedTarget::Collection(name) => Target::Collection(name),
                SavedTarget::Url(url) => Target::Url(url),
            }),
        }
    }
}

impl Saved {
    pub fn new(queue: QueueSnapshot, source: Option<&Source>, position: f64) -> Self {
        Self {
            version: VERSION,
            queue,
            source: source.map(SavedSource::from),
            position,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// `None` for a file from an unknown (newer) version or without a
    /// current track. The newer of the two saved positions wins.
    pub fn decode(queue: &[u8], position: Option<&[u8]>) -> Option<Self> {
        let mut saved: Self = serde_json::from_slice(queue).ok()?;
        if saved.version != VERSION {
            return None;
        }
        let current = saved.queue.current.as_ref()?.id;
        if let Some(position) = position
            .and_then(|data| serde_json::from_slice::<SavedPosition>(data).ok())
            .filter(|p| p.track_id == current)
        {
            saved.position = position.position;
        }
        saved.position = saved.position.max(0.0);
        Some(saved)
    }
}

/// `$XDG_STATE_HOME/swell-player` (GLib's `g_get_user_state_dir` needs a
/// newer binding than ours).
fn dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local").join("state"))
        .join("swell-player")
}

fn queue_path() -> PathBuf {
    dir().join("queue.json")
}

fn position_path() -> PathBuf {
    dir().join("position.json")
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("json.part");
    std::fs::write(&partial, data)?;
    std::fs::rename(&partial, path)
}

/// Writes the whole session (blocking but small and fast: used at quit
/// and from the I/O runtime's blocking pool).
pub fn write(saved: &Saved) -> Result<()> {
    write_atomic(&queue_path(), &saved.encode()?)?;
    write_position(&SavedPosition {
        track_id: saved.queue.current.as_ref().map_or(0, |t| t.id),
        position: saved.position,
    })
}

pub fn write_position(position: &SavedPosition) -> Result<()> {
    Ok(write_atomic(
        &position_path(),
        &serde_json::to_vec(position)?,
    )?)
}

/// Reads the saved session, if any.
pub async fn read() -> Result<Option<Saved>> {
    let queue = match tokio::fs::read(queue_path()).await {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let position = tokio::fs::read(position_path()).await.ok();
    Ok(Saved::decode(&queue, position.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackInfo;
    use crate::settings::Repeat;

    fn saved() -> Saved {
        let track = |id| TrackInfo {
            id,
            title: format!("Track {id}"),
            available: true,
            ..TrackInfo::default()
        };
        let queue = QueueSnapshot {
            context: vec![track(1), track(2), track(3)],
            order: vec![2, 0, 1],
            next: 1,
            current: Some(track(3)),
            current_at: Some(0),
            up_next: vec![track(9)],
            shuffle: true,
            repeat: Repeat::All,
        };
        let source = Source {
            title: "Album".into(),
            target: Some(Target::Album(42)),
        };
        Saved::new(queue, Some(&source), 61.5)
    }

    #[test]
    fn round_trip() {
        let saved = saved();
        let data = saved.encode().unwrap();
        assert_eq!(Saved::decode(&data, None), Some(saved.clone()));
        let source: Source = saved.source.clone().unwrap().into();
        assert_eq!(source.target, Some(Target::Album(42)));
    }

    #[test]
    fn newer_position_wins_for_the_same_track() {
        let data = saved().encode().unwrap();
        let same = serde_json::to_vec(&SavedPosition {
            track_id: 3,
            position: 120.0,
        })
        .unwrap();
        assert_eq!(Saved::decode(&data, Some(&same)).unwrap().position, 120.0);
        let other = serde_json::to_vec(&SavedPosition {
            track_id: 7,
            position: 120.0,
        })
        .unwrap();
        assert_eq!(Saved::decode(&data, Some(&other)).unwrap().position, 61.5);
    }

    #[test]
    fn rejects_unknown_or_empty() {
        let mut future = saved();
        future.version = 99;
        assert_eq!(Saved::decode(&future.encode().unwrap(), None), None);
        let empty = Saved::new(QueueSnapshot::default(), None, 0.0);
        assert_eq!(Saved::decode(&empty.encode().unwrap(), None), None);
        assert_eq!(Saved::decode(b"not json", None), None);
    }
}
