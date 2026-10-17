//! The Search page's data (docs/design.md §3.4): the filters, one answer
//! from TIDAL's `search` reduced to what the page shows, the answers of the
//! last few queries (in memory, so going back to a query is instant) and
//! the recent searches, kept in `$XDG_STATE_HOME/swell-player/searches.json`
//! and read and written on the I/O runtime.

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use gtk::glib;
use lru::LruCache;
use serde::{Deserialize, Serialize};

use crate::api::Api;
use crate::i18n::tr;
use crate::model::TrackInfo;
use crate::runtime;
use crate::tidal::SearchType;
use crate::tidal::models::{Album, Artist, Playlist, SearchResults, TopHit, Track};
use crate::tidal::pages::Item;

/// Recent searches kept (and shown when the field is empty).
pub const MAX_RECENT: usize = 10;
/// Queries whose answers stay in memory.
const CACHED_QUERIES: usize = 8;
/// Items per type for "All" (five tracks and carousels of up to 20).
const ALL_LIMIT: usize = 20;
/// Items per page of a single-type filter.
pub const PAGE: usize = 50;

/// Bumped by every save (and by [`forget_all`]): when writes overlap, only
/// the newest reaches the disk.
static SAVE_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Held while the file is written or deleted.
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What the results show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl Filter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Tracks,
        Self::Albums,
        Self::Artists,
        Self::Playlists,
    ];

    /// Name of its toggle (and of `SWELL_SEARCH_FILTER`).
    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::All => tr("All"),
            Self::Tracks => tr("Tracks"),
            Self::Albums => tr("Albums"),
            Self::Artists => tr("Artists"),
            Self::Playlists => tr("Playlists"),
        }
    }

    fn types(self) -> &'static [SearchType] {
        match self {
            Self::All => &[
                SearchType::Artists,
                SearchType::Albums,
                SearchType::Tracks,
                SearchType::Playlists,
            ],
            Self::Tracks => &[SearchType::Tracks],
            Self::Albums => &[SearchType::Albums],
            Self::Artists => &[SearchType::Artists],
            Self::Playlists => &[SearchType::Playlists],
        }
    }

    fn limit(self) -> usize {
        if self == Self::All { ALL_LIMIT } else { PAGE }
    }
}

/// One answer, ready for the page.
#[derive(Debug, Default)]
pub struct Results {
    pub top_hit: Option<Item>,
    pub tracks: Vec<TrackInfo>,
    pub albums: Vec<Item>,
    pub artists: Vec<Item>,
    pub playlists: Vec<Item>,
    pub total_tracks: usize,
    pub total_albums: usize,
    pub total_artists: usize,
    pub total_playlists: usize,
}

impl Results {
    fn from_search(results: SearchResults) -> Self {
        Self {
            top_hit: results.top_hit.and_then(top_hit),
            total_tracks: results.tracks.total,
            total_albums: results.albums.total,
            total_artists: results.artists.total,
            total_playlists: results.playlists.total,
            tracks: results
                .tracks
                .items
                .into_iter()
                .map(TrackInfo::from)
                .collect(),
            albums: results
                .albums
                .items
                .into_iter()
                .map(|a| Item::Album(Box::new(a)))
                .collect(),
            artists: results
                .artists
                .items
                .into_iter()
                .map(|a| Item::Artist(Box::new(a)))
                .collect(),
            playlists: results
                .playlists
                .items
                .into_iter()
                .map(|p| Item::Playlist(Box::new(p)))
                .collect(),
        }
    }

    /// Whether `filter` has nothing to show.
    pub fn is_empty(&self, filter: Filter) -> bool {
        match filter {
            Filter::All => {
                self.top_hit.is_none()
                    && self.tracks.is_empty()
                    && self.albums.is_empty()
                    && self.artists.is_empty()
                    && self.playlists.is_empty()
            }
            Filter::Tracks => self.tracks.is_empty(),
            other => self.cards(other).is_empty(),
        }
    }

    /// The cards of a card filter (albums, artists or playlists).
    pub fn cards(&self, filter: Filter) -> &[Item] {
        match filter {
            Filter::Albums => &self.albums,
            Filter::Artists => &self.artists,
            Filter::Playlists => &self.playlists,
            Filter::All | Filter::Tracks => &[],
        }
    }

    /// How many results of `filter`'s type TIDAL has in all.
    pub fn total(&self, filter: Filter) -> usize {
        match filter {
            Filter::All => 0,
            Filter::Tracks => self.total_tracks,
            Filter::Albums => self.total_albums,
            Filter::Artists => self.total_artists,
            Filter::Playlists => self.total_playlists,
        }
    }

    /// How many items of `filter`'s type this answer holds.
    pub fn len(&self, filter: Filter) -> usize {
        match filter {
            Filter::Tracks => self.tracks.len(),
            other => self.cards(other).len(),
        }
    }
}

/// The top hit as a page item. TIDAL names its type in the plural
/// ("ARTISTS"); the singular is accepted too.
fn top_hit(hit: TopHit) -> Option<Item> {
    let kind = hit.kind.to_ascii_uppercase();
    let value = hit.value;
    let item = match kind.trim_end_matches('S') {
        "ARTIST" => Item::Artist(Box::new(serde_json::from_value::<Artist>(value).ok()?)),
        "ALBUM" => Item::Album(Box::new(serde_json::from_value::<Album>(value).ok()?)),
        "TRACK" => Item::Track(Box::new(serde_json::from_value::<Track>(value).ok()?)),
        "PLAYLIST" => Item::Playlist(Box::new(serde_json::from_value::<Playlist>(value).ok()?)),
        other => {
            log::debug!("search: unknown top hit type {other}");
            return None;
        }
    };
    Some(item)
}

/// Searches TIDAL (call it from the I/O runtime).
pub async fn fetch(api: Arc<Api>, query: String, filter: Filter, offset: usize) -> Result<Results> {
    let results = api
        .search(&query, filter.types(), offset, filter.limit())
        .await?;
    Ok(Results::from_search(results))
}

/// Key of a query in the in-memory cache.
fn cache_key(query: &str, filter: Filter) -> (Filter, String) {
    (filter, query.trim().to_lowercase())
}

thread_local! {
    static ANSWERS: RefCell<LruCache<(Filter, String), Rc<Results>>> =
        RefCell::new(LruCache::new(NonZeroUsize::new(CACHED_QUERIES).expect("non-zero")));
    static RECENT: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// The answer to `query` with `filter` (first page), if it is in memory.
pub fn cached(query: &str, filter: Filter) -> Option<Rc<Results>> {
    ANSWERS.with_borrow_mut(|cache| cache.get(&cache_key(query, filter)).cloned())
}

pub fn remember_answer(query: &str, filter: Filter, results: Rc<Results>) {
    ANSWERS.with_borrow_mut(|cache| cache.put(cache_key(query, filter), results));
}

/// The recent searches, newest first (read from disk the first time).
pub async fn recent() -> Vec<String> {
    if let Some(list) = RECENT.with_borrow(Clone::clone) {
        return list;
    }
    let read = runtime::spawn(async move {
        let path = path();
        Ok(match tokio::fs::read(&path).await {
            Ok(data) => serde_json::from_slice::<SavedSearches>(&data)
                .map(|saved| saved.recent)
                .unwrap_or_else(|err| {
                    log::warn!("ignoring {}: {err}", path.display());
                    Vec::new()
                }),
            Err(_) => Vec::new(),
        })
    })
    .await
    .unwrap_or_default();
    // A search remembered while reading wins over the file.
    RECENT.with_borrow_mut(|recent| {
        recent
            .get_or_insert_with(|| {
                let mut read = read;
                read.truncate(MAX_RECENT);
                read
            })
            .clone()
    })
}

/// The recent searches if they have been read already.
pub fn recent_now() -> Option<Vec<String>> {
    RECENT.with_borrow(Clone::clone)
}

/// Puts `query` first in the recent searches.
pub fn remember(query: &str) {
    let query = query.trim().to_owned();
    if query.is_empty() {
        return;
    }
    edit_recent(move |list| push_recent(list, &query));
}

/// Removes `query` from the recent searches.
pub fn forget(query: &str) {
    let query = query.to_owned();
    edit_recent(move |list| list.retain(|q| *q != query));
}

/// Replaces the recent searches (to undo "Clear").
pub fn set_recent(list: Vec<String>) {
    edit_recent(move |recent| *recent = list);
}

pub fn clear_recent() {
    edit_recent(Vec::clear);
}

/// Forgets everything searched (signing out): the recent searches, in
/// memory and on disk, and the remembered answers. A save still pending is
/// dropped; the file is deleted off the main thread.
pub fn forget_all() {
    RECENT.with_borrow_mut(|recent| *recent = Some(Vec::new()));
    ANSWERS.with_borrow_mut(LruCache::clear);
    SAVE_GENERATION.fetch_add(1, Ordering::SeqCst);
    runtime::runtime().spawn_blocking(|| {
        let _guard = SAVING.lock().unwrap_or_else(|e| e.into_inner());
        match std::fs::remove_file(path()) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                log::warn!("could not delete recent searches: {err}");
            }
            _ => {}
        }
    });
}

/// Applies `edit` to the recent searches and saves them (once they have
/// been read, so the file's list is never lost).
fn edit_recent(edit: impl FnOnce(&mut Vec<String>) + 'static) {
    if RECENT.with_borrow(Option::is_none) {
        glib::spawn_future_local(async move {
            recent().await;
            edit_recent(edit);
        });
        return;
    }
    let list = RECENT.with_borrow_mut(|recent| {
        let list = recent.get_or_insert_with(Vec::new);
        let before = list.clone();
        edit(list);
        (before != *list).then(|| list.clone())
    });
    if let Some(list) = list {
        save(list);
    }
}

/// Moves (or adds) `query` to the front, ignoring case, and keeps
/// [`MAX_RECENT`].
fn push_recent(list: &mut Vec<String>, query: &str) {
    let lower = query.to_lowercase();
    list.retain(|q| q.to_lowercase() != lower);
    list.insert(0, query.to_owned());
    list.truncate(MAX_RECENT);
}

#[derive(Default, Serialize, Deserialize)]
struct SavedSearches {
    #[serde(default)]
    recent: Vec<String>,
}

/// Writes the list on the I/O runtime; when writes overlap, only the
/// newest reaches the disk.
fn save(recent: Vec<String>) {
    let generation = SAVE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let data = match serde_json::to_vec_pretty(&SavedSearches { recent }) {
        Ok(data) => data,
        Err(err) => return log::warn!("could not save recent searches: {err}"),
    };
    runtime::runtime().spawn_blocking(move || {
        let _guard = SAVING.lock().unwrap_or_else(|e| e.into_inner());
        if SAVE_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Err(err) = write_atomic(&path(), &data) {
            log::warn!("could not save recent searches: {err}");
        }
    });
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data)?;
    std::fs::rename(&temporary, path)
}

/// `$XDG_STATE_HOME/swell-player/searches.json`.
fn path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local").join("state"))
        .join("swell-player")
        .join("searches.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_searches_move_to_the_front_ignoring_case() {
        let mut list = vec!["eminem".to_owned(), "YOASOBI".to_owned()];
        push_recent(&mut list, "yoasobi");
        assert_eq!(list, ["yoasobi", "eminem"]);
        for i in 0..20 {
            push_recent(&mut list, &format!("q{i}"));
        }
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0], "q19");
    }

    #[test]
    fn top_hits_of_every_type() {
        let hit = |kind: &str, value: serde_json::Value| {
            top_hit(TopHit {
                kind: kind.into(),
                value,
            })
        };
        assert!(matches!(
            hit("ARTISTS", serde_json::json!({"id": 1, "name": "A"})),
            Some(Item::Artist(a)) if a.id == 1
        ));
        assert!(matches!(
            hit("ALBUMS", serde_json::json!({"id": 2, "title": "B"})),
            Some(Item::Album(a)) if a.id == 2
        ));
        assert!(matches!(
            hit("TRACK", serde_json::json!({"id": 3, "title": "C"})),
            Some(Item::Track(t)) if t.id == 3
        ));
        assert!(matches!(
            hit("PLAYLISTS", serde_json::json!({"uuid": "x", "title": "D"})),
            Some(Item::Playlist(p)) if p.uuid == "x"
        ));
        assert!(hit("VIDEOS", serde_json::json!({"id": 4})).is_none());
    }

    #[test]
    fn filters_round_trip_by_name() {
        for filter in Filter::ALL {
            assert_eq!(Filter::from_name(filter.name()), Some(filter));
        }
    }
}
