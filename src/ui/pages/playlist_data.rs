//! Loading a playlist for its page: details, then items 100 at a time
//! (several requests in flight), cached in the response cache.
//!
//! The details are cached under `playlists/{uuid}` and each page of items
//! under a key that includes the playlist's `lastUpdated`, so a cached copy
//! is revalidated with one small request: if TIDAL's `lastUpdated` (and
//! counts) still match, the cached items are current. Editing a playlist
//! forgets everything under `playlists/{uuid}` (see
//! [`crate::api::Api::invalidate`]).
//!
//! Playlists can hold videos, which Swell does not show. [`Slots`] keeps
//! the place of every item so the indices sent back to TIDAL (remove,
//! move) count the hidden videos too.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures_util::{StreamExt, stream};
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::api::Api;
use crate::model::TrackInfo;
use crate::tidal::models::{Playlist, Track};
use crate::tidal::{Base, Call};

/// Items per request (TIDAL's maximum).
pub const PAGE: usize = 100;
/// Requests in flight while loading the rest of a playlist.
const CONCURRENCY: usize = 4;

/// Some items of a playlist, in order.
#[derive(Debug, Default)]
pub struct Chunk {
    /// The tracks among them.
    pub tracks: Vec<TrackInfo>,
    /// One entry per item: `true` for a track, `false` for something
    /// hidden (a video).
    pub slots: Vec<bool>,
}

impl Chunk {
    fn extend(&mut self, other: Chunk) {
        self.tracks.extend(other.tracks);
        self.slots.extend(other.slots);
    }
}

/// What the loader reports, in this order: details (possibly twice: cached,
/// then fresh), items, then `Done` or `Failed`.
pub enum Update {
    Details(Box<Playlist>),
    /// Items; `reset` replaces what is shown, otherwise they follow it.
    /// `total` counts every item of the playlist (hidden ones too).
    Items {
        chunk: Chunk,
        total: usize,
        reset: bool,
    },
    /// Everything is loaded and current (or the cached copy is all there
    /// is, offline).
    Done,
    Failed(anyhow::Error),
}

/// Loads a playlist: the cached copy first, then, unless it is younger
/// than `fresh_for`, whatever changed.
pub fn load(api: &Arc<Api>, uuid: String, fresh_for: Duration) -> UnboundedReceiver<Update> {
    let (tx, rx) = unbounded_channel();
    let api = api.clone();
    crate::runtime::runtime().spawn(async move {
        if let Err(err) = run(&api, &uuid, fresh_for, &tx).await {
            let _ = tx.send(Update::Failed(err));
        }
    });
    rx
}

/// Loads items from `offset` to the end from TIDAL (resuming after a
/// failure, or picking up tracks just added), without caching them.
pub fn load_from(api: &Arc<Api>, uuid: String, offset: usize) -> UnboundedReceiver<Update> {
    let (tx, rx) = unbounded_channel();
    let api = api.clone();
    crate::runtime::runtime().spawn(async move {
        let result = async {
            let (first, total) = fetch_page(&api, &uuid, offset).await?;
            fetch_rest(&api, &uuid, None, offset, first, total, true, &tx).await
        }
        .await;
        match result {
            Ok(()) => {
                let _ = tx.send(Update::Done);
            }
            Err(err) => {
                let _ = tx.send(Update::Failed(err));
            }
        }
    });
    rx
}

/// The playlist's details from TIDAL.
pub async fn details(api: &Api, uuid: &str) -> Result<Playlist> {
    Ok(fetch_details(api, uuid).await?.0)
}

/// The track at `index` (counting hidden items), to check what an append
/// added.
pub async fn track_at(api: &Api, uuid: &str, index: usize) -> Result<Option<u64>> {
    let call = Call::get(Base::V1, format!("playlists/{uuid}/items")).page(index, 1);
    let value: Value = api.send(&call).await?.json()?;
    let (chunk, _) = parse_items(&value);
    Ok(chunk
        .slots
        .first()
        .copied()
        .unwrap_or(false)
        .then(|| chunk.tracks.first().map(|t| t.id))
        .flatten())
}

/// Puts a removed track back at item index `to` (undo): appends it (even
/// if the playlist has it elsewhere) and moves it there from `appended_at`.
/// `false` if the append did not land where expected (the playlist was
/// edited elsewhere meanwhile).
pub async fn restore(
    api: &Api,
    uuid: &str,
    track_id: u64,
    appended_at: usize,
    to: usize,
) -> Result<bool> {
    let (_, etag) = api.playlist(uuid).await?;
    let mut call = Call::post(Base::V1, format!("playlists/{uuid}/items"))
        .form("trackIds", track_id)
        .form("onDupes", "ADD");
    if let Some(etag) = etag {
        call = call.header("If-None-Match", etag);
    }
    api.write(call).await?;
    if track_at(api, uuid, appended_at).await? != Some(track_id) {
        return Ok(false);
    }
    if appended_at != to {
        api.move_in_playlist(uuid, appended_at, to).await?;
    }
    Ok(true)
}

/// Forgets the cached copy (after an edit).
pub async fn forget(api: &Api, uuid: &str) {
    api.invalidate(Base::V1, &format!("playlists/{uuid}")).await;
}

/// What identifies a version of a playlist's contents.
fn version(playlist: &Playlist) -> (Option<&str>, u32, u32) {
    (
        playlist.last_updated.as_deref(),
        playlist.number_of_tracks,
        playlist.duration,
    )
}

fn details_key(uuid: &str) -> String {
    format!("{}playlists/{uuid}", Base::V1.prefix())
}

fn items_prefix(uuid: &str) -> String {
    format!("{}playlists/{uuid}/items", Base::V1.prefix())
}

fn items_key(uuid: &str, playlist: &Playlist, offset: usize) -> String {
    format!(
        "{}#{}@{offset}",
        items_prefix(uuid),
        playlist.last_updated.as_deref().unwrap_or("")
    )
}

async fn run(
    api: &Api,
    uuid: &str,
    fresh_for: Duration,
    tx: &UnboundedSender<Update>,
) -> Result<()> {
    let user = api.user_id();
    let cache = api.cache().await;

    // 1. The cached copy, as far as it goes.
    let mut cached: Option<Playlist> = None;
    let mut complete = false;
    let mut items_shown = false;
    let mut age = Duration::MAX;
    if let Some(cache) = cache
        && let Ok(Some((data, cached_age))) = cache.response(user, details_key(uuid)).await
        && let Ok(playlist) = serde_json::from_slice::<Playlist>(&data)
    {
        age = cached_age;
        let _ = tx.send(Update::Details(Box::new(playlist.clone())));
        let mut chunk = Chunk::default();
        let mut total = None;
        loop {
            let key = items_key(uuid, &playlist, chunk.slots.len());
            let Ok(Some((data, _))) = cache.response(user, key).await else {
                break;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&data) else {
                break;
            };
            let (page, page_total) = parse_items(&value);
            let empty = page.slots.is_empty();
            chunk.extend(page);
            total = Some(page_total);
            if empty || chunk.slots.len() >= page_total {
                complete = true;
                break;
            }
        }
        if let Some(total) = total {
            items_shown = true;
            let _ = tx.send(Update::Items {
                chunk,
                total,
                reset: true,
            });
        }
        cached = Some(playlist);
    }
    if complete && age < fresh_for {
        let _ = tx.send(Update::Done);
        return Ok(());
    }

    // 2. Revalidate: unchanged details mean unchanged items.
    let fresh = match &cached {
        Some(cached) => match fetch_details(api, uuid).await {
            Ok((fresh, raw)) => {
                store(api, details_key(uuid), raw).await;
                if version(&fresh) != version(cached)
                    || fresh.title != cached.title
                    || fresh.description != cached.description
                    || fresh.public_playlist != cached.public_playlist
                    || fresh.square_image != cached.square_image
                {
                    let _ = tx.send(Update::Details(Box::new(fresh.clone())));
                }
                if complete && version(&fresh) == version(cached) {
                    let _ = tx.send(Update::Done);
                    return Ok(());
                }
                fresh
            }
            // Offline: the cached copy is all there is.
            Err(err) if complete => {
                log::debug!("revalidating playlist {uuid} failed: {err:#}");
                let _ = tx.send(Update::Done);
                return Ok(());
            }
            Err(err) => return Err(err),
        },
        None => {
            // Nothing cached: details and the first page together.
            let (details, first) = tokio::join!(fetch_details(api, uuid), fetch_raw(api, uuid, 0));
            let (fresh, raw) = details?;
            store(api, details_key(uuid), raw).await;
            let _ = tx.send(Update::Details(Box::new(fresh.clone())));
            let (value, raw) = first?;
            let (chunk, total) = parse_items(&value);
            forget_items(api, uuid).await;
            store(api, items_key(uuid, &fresh, 0), raw).await;
            return fetch_rest(api, uuid, Some(&fresh), 0, chunk, total, true, tx)
                .await
                .map(|()| {
                    let _ = tx.send(Update::Done);
                });
        }
    };

    // 3. The items changed (or were not all cached): fetch them all. When
    //    a stale copy is on screen, it is replaced at once at the end.
    forget_items(api, uuid).await;
    let (value, raw) = fetch_raw(api, uuid, 0).await?;
    store(api, items_key(uuid, &fresh, 0), raw).await;
    let (chunk, total) = parse_items(&value);
    fetch_rest(api, uuid, Some(&fresh), 0, chunk, total, !items_shown, tx).await?;
    let _ = tx.send(Update::Done);
    Ok(())
}

/// Sends `first` (the page at `offset`) and fetches the pages after it,
/// `CONCURRENCY` at a time, in order. `progressive` sends each page as it
/// comes; otherwise everything is sent at the end, replacing what is shown.
/// With `cache_as`, pages are stored under that version.
#[allow(clippy::too_many_arguments)]
async fn fetch_rest(
    api: &Api,
    uuid: &str,
    cache_as: Option<&Playlist>,
    offset: usize,
    first: Chunk,
    total: usize,
    progressive: bool,
    tx: &UnboundedSender<Update>,
) -> Result<()> {
    let first_len = first.slots.len();
    let mut all = Chunk::default();
    if progressive {
        let _ = tx.send(Update::Items {
            chunk: first,
            total,
            reset: offset == 0,
        });
    } else {
        all.extend(first);
    }
    if first_len == 0 {
        if !progressive {
            let _ = tx.send(Update::Items {
                chunk: all,
                total,
                reset: true,
            });
        }
        return Ok(());
    }
    let offsets = ((offset + first_len)..total).step_by(PAGE);
    let mut pages = stream::iter(offsets)
        .map(|at| async move { (at, fetch_raw(api, uuid, at).await) })
        .buffered(CONCURRENCY);
    while let Some((at, page)) = pages.next().await {
        let (value, raw) = page?;
        if let Some(playlist) = cache_as {
            store(api, items_key(uuid, playlist, at), raw).await;
        }
        let (chunk, _) = parse_items(&value);
        if progressive {
            let _ = tx.send(Update::Items {
                chunk,
                total,
                reset: false,
            });
        } else {
            all.extend(chunk);
        }
    }
    if !progressive {
        let _ = tx.send(Update::Items {
            chunk: all,
            total,
            reset: true,
        });
    }
    Ok(())
}

async fn fetch_details(api: &Api, uuid: &str) -> Result<(Playlist, Vec<u8>)> {
    let response = api
        .send(&Call::get(Base::V1, format!("playlists/{uuid}")))
        .await?;
    Ok((response.json()?, response.body.to_vec()))
}

async fn fetch_raw(api: &Api, uuid: &str, offset: usize) -> Result<(Value, Vec<u8>)> {
    let call = Call::get(Base::V1, format!("playlists/{uuid}/items")).page(offset, PAGE);
    let response = api.send(&call).await?;
    let value = serde_json::from_slice(&response.body)
        .map_err(|err| anyhow!("TIDAL sent unreadable playlist items: {err}"))?;
    Ok((value, response.body.to_vec()))
}

async fn fetch_page(api: &Api, uuid: &str, offset: usize) -> Result<(Chunk, usize)> {
    let (value, _) = fetch_raw(api, uuid, offset).await?;
    Ok(parse_items(&value))
}

async fn store(api: &Api, key: String, data: Vec<u8>) {
    if api.is_signed_out() {
        return;
    }
    if let Some(cache) = api.cache().await
        && let Err(err) = cache.store_response(api.user_id(), key, data).await
    {
        log::debug!("could not cache a playlist: {err:#}");
    }
}

/// Drops cached pages of older versions.
async fn forget_items(api: &Api, uuid: &str) {
    if let Some(cache) = api.cache().await {
        let _ = cache
            .forget_responses(api.user_id(), items_prefix(uuid))
            .await;
    }
}

/// A page of `playlists/{uuid}/items` and the playlist's item count.
fn parse_items(value: &Value) -> (Chunk, usize) {
    let mut chunk = Chunk::default();
    let items = value.get("items").and_then(Value::as_array);
    for wrapped in items.into_iter().flatten() {
        let is_track = wrapped
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind.eq_ignore_ascii_case("track"));
        let track = is_track
            .then(|| wrapped.get("item"))
            .flatten()
            .and_then(|item| serde_json::from_value::<Track>(item.clone()).ok());
        match track {
            Some(mut track) => {
                if let Some(created) = wrapped.get("created").and_then(Value::as_str) {
                    track.date_added = Some(created.to_owned());
                }
                chunk.tracks.push(TrackInfo::from(track));
                chunk.slots.push(true);
            }
            None => chunk.slots.push(false),
        }
    }
    let total = value
        .get("totalNumberOfItems")
        .and_then(Value::as_u64)
        .map_or(chunk.slots.len(), |t| t as usize);
    (chunk, total)
}

/// Where each shown track sits among all the playlist's items.
#[derive(Debug, Default)]
pub struct Slots(Vec<bool>);

impl Slots {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn extend(&mut self, slots: &[bool]) {
        self.0.extend_from_slice(slots);
    }

    /// Items loaded (hidden ones too).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Item index of the track at list index `rank`.
    pub fn index_of(&self, rank: usize) -> Option<usize> {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, track)| **track)
            .nth(rank)
            .map(|(index, _)| index)
    }

    /// Forgets the track at `rank`; returns its item index.
    pub fn remove(&mut self, rank: usize) -> Option<usize> {
        let index = self.index_of(rank)?;
        self.0.remove(index);
        Some(index)
    }

    /// Makes room for a track at `rank` (before the track now there, or at
    /// the end); returns its item index.
    pub fn insert(&mut self, rank: usize) -> usize {
        let index = self.index_of(rank).unwrap_or(self.0.len());
        self.0.insert(index, true);
        index
    }

    /// Puts a track back at item index `index` (undoing [`Slots::remove`]).
    pub fn insert_at(&mut self, index: usize) {
        self.0.insert(index.min(self.0.len()), true);
    }

    /// Forgets the item at `index` (undoing [`Slots::insert`]).
    pub fn remove_at(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }

    /// Moves the track at `from` to `to` (list indices, as
    /// [`crate::ui::widgets::track_list::TrackList::move_track`] does);
    /// returns the item indices for TIDAL.
    pub fn move_track(&mut self, from: usize, to: usize) -> Option<(usize, usize)> {
        let source = self.remove(from)?;
        let target = self.insert(to);
        Some((source, target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(pattern: &str) -> Slots {
        Slots(pattern.chars().map(|c| c == 't').collect())
    }

    fn pattern(slots: &Slots) -> String {
        slots.0.iter().map(|t| if *t { 't' } else { 'v' }).collect()
    }

    #[test]
    fn indices_skip_hidden_items() {
        let s = slots("vtvtt");
        assert_eq!(s.index_of(0), Some(1));
        assert_eq!(s.index_of(1), Some(3));
        assert_eq!(s.index_of(2), Some(4));
        assert_eq!(s.index_of(3), None);
    }

    #[test]
    fn remove_and_insert_round_trip() {
        let mut s = slots("vtvtt");
        assert_eq!(s.remove(1), Some(3));
        assert_eq!(pattern(&s), "vtvt");
        assert_eq!(s.insert(1), 3);
        assert_eq!(pattern(&s), "vtvtt");
        assert_eq!(s.insert(3), 5);
        assert_eq!(pattern(&s), "vtvttt");
    }

    #[test]
    fn moves_match_tidal_semantics() {
        // Tracks only: TIDAL moves item `from` so it ends up at `to`.
        let mut s = slots("tttt");
        assert_eq!(s.move_track(0, 2), Some((0, 2)));
        assert_eq!(s.move_track(3, 0), Some((3, 0)));
        // With a video between: the track goes before the track now at
        // `to`, the video keeps its place relative to the others.
        let mut s = slots("tvtt");
        assert_eq!(s.move_track(2, 0), Some((3, 0)));
        assert_eq!(pattern(&s), "ttvt");
        let mut s = slots("tvtt");
        assert_eq!(s.move_track(0, 2), Some((0, 3)));
        assert_eq!(pattern(&s), "vttt");
    }

    #[test]
    fn parses_tracks_and_hides_videos() {
        let value = serde_json::json!({
            "totalNumberOfItems": 3,
            "items": [
                {"type": "track", "item": {"id": 1, "title": "A"}, "created": "2020-01-01"},
                {"type": "video", "item": {"id": 2, "title": "B"}},
                {"type": "track", "item": {"id": 3, "title": "C"}},
            ]
        });
        let (chunk, total) = parse_items(&value);
        assert_eq!(total, 3);
        assert_eq!(chunk.slots, vec![true, false, true]);
        assert_eq!(
            chunk.tracks.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(chunk.tracks[0].added.as_deref(), Some("2020-01-01"));
    }
}
