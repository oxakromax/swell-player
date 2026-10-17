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
//!
//! Edits name items by index, so they must apply to the version of the
//! playlist the indices were computed from. TIDAL versions playlists with
//! an ETag (sent back in `If-None-Match`; a stale one gets 412): the loader
//! reports the ETag of the items it shows ([`Update::Version`]), every edit
//! sends it and gets the next one back, and a refusal comes back as
//! [`Changed`], after which the page reloads. TIDAL's ETags lag behind its
//! writes, though, so before giving up [`send`] checks whether the
//! playlist still holds exactly the items the edit was computed on.
//! [`EditQueue`] keeps the edits waiting to be sent, the version each
//! belongs to and the items of that version.

use std::collections::VecDeque;
use std::fmt;
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
    /// The ETag of the version of the playlist that the items shown since
    /// the last reset (and the ones still to come) belong to. Sent after
    /// the items that replace what is shown: a reset makes the version
    /// unknown until the next `Version`.
    Version(Option<String>),
    /// The playlist changed while it was loading: load it again.
    Changed,
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

/// TIDAL refused an edit because the playlist is no longer the version its
/// items were loaded from (or it changed while loading).
#[derive(Debug)]
pub struct Changed;

impl fmt::Display for Changed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the playlist changed elsewhere")
    }
}

impl std::error::Error for Changed {}

/// Whether `err` is [`Changed`].
pub fn is_changed(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Changed>().is_some()
}

/// [`Changed`] for TIDAL's "wrong ETag" (412), `err` otherwise.
fn conflict(err: anyhow::Error) -> anyhow::Error {
    match err.downcast_ref::<crate::tidal::Error>() {
        Some(crate::tidal::Error::Api { status: 412, .. }) => Changed.into(),
        _ => err,
    }
}

fn finish(tx: &UnboundedSender<Update>, result: Result<()>) {
    let _ = tx.send(match result {
        Ok(()) => Update::Done,
        Err(err) if is_changed(&err) => Update::Changed,
        Err(err) => Update::Failed(err),
    });
}

/// Loads a playlist: the cached copy first, then, unless it is younger
/// than `fresh_for`, whatever changed.
pub fn load(api: &Arc<Api>, uuid: String, fresh_for: Duration) -> UnboundedReceiver<Update> {
    let (tx, rx) = unbounded_channel();
    let api = api.clone();
    crate::runtime::runtime().spawn(async move {
        let result = run(&api, &uuid, fresh_for, &tx).await;
        finish(&tx, result);
    });
    rx
}

/// Loads items from `offset` to the end from TIDAL (resuming after a
/// failure, or picking up tracks just added), without caching them. They
/// must belong to version `etag` of the playlist (the one shown), or
/// [`Update::Changed`] is sent.
pub fn load_from(
    api: &Arc<Api>,
    uuid: String,
    offset: usize,
    etag: Option<String>,
) -> UnboundedReceiver<Update> {
    let (tx, rx) = unbounded_channel();
    let api = api.clone();
    crate::runtime::runtime().spawn(async move {
        let result = async {
            let (value, _, version) = fetch_raw(&api, &uuid, offset).await?;
            if etag.is_some() && version != etag {
                return Err(Changed.into());
            }
            let (first, total) = parse_items(&value);
            fetch_rest(&api, &uuid, None, version, offset, first, total, true, &tx).await
        }
        .await;
        finish(&tx, result);
    });
    rx
}

/// The playlist's details from TIDAL.
pub async fn details(api: &Api, uuid: &str) -> Result<Playlist> {
    Ok(fetch_details(api, uuid).await?.0)
}

/// The items of a version of a playlist, in order: a track's id, or `None`
/// for something hidden (a video).
pub type Items = Vec<Option<u64>>;

/// The items of `chunk`.
pub fn items_of(chunk: &Chunk) -> Items {
    let mut tracks = chunk.tracks.iter();
    chunk
        .slots
        .iter()
        .map(|track| {
            if *track {
                tracks.next().map(|t| t.id)
            } else {
                None
            }
        })
        .collect()
}

/// The track at `index` (counting hidden items), to check what an append
/// added.
async fn track_at(api: &Api, uuid: &str, index: usize) -> Result<Option<u64>> {
    let call = Call::get(Base::V1, format!("playlists/{uuid}/items")).page(index, 1);
    let value: Value = api.send(&call).await?.json()?;
    let (chunk, _) = parse_items(&value);
    Ok(items_of(&chunk).first().copied().flatten())
}

/// All the items of the playlist as TIDAL has them now, and the ETag it
/// reports with them.
async fn snapshot(api: &Api, uuid: &str) -> Result<(Items, Option<String>)> {
    let (value, _, etag) = fetch_raw(api, uuid, 0).await?;
    let (first, total) = parse_items(&value);
    let mut items = items_of(&first);
    if !first.slots.is_empty() {
        let mut pages = stream::iter((items.len()..total).step_by(PAGE))
            .map(|at| async move { fetch_raw(api, uuid, at).await })
            .buffered(CONCURRENCY);
        while let Some(page) = pages.next().await {
            items.extend(items_of(&parse_items(&page?.0).0));
        }
    }
    Ok((items, etag))
}

/// An edit as sent to TIDAL: item indices count hidden items too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Remove {
        item: usize,
    },
    /// Moves item `from` so it ends up at `to`.
    Move {
        from: usize,
        to: usize,
    },
    /// Appends tracks not in the playlist yet.
    Add(Vec<u64>),
    /// Puts a removed track back at item index `to` (undo): appends it
    /// (even if the playlist has it elsewhere), checks that it landed at
    /// `appended_at` and moves it into place.
    Restore {
        track: u64,
        appended_at: usize,
        to: usize,
    },
}

impl Request {
    /// What `items` become with this edit; `false` (and `items` are not
    /// to be trusted) if it does not fit them.
    pub fn apply(&self, items: &mut Items) -> bool {
        match self {
            Request::Remove { item } => {
                if *item >= items.len() {
                    return false;
                }
                items.remove(*item);
            }
            Request::Move { from, to } => {
                if *from >= items.len() || *to >= items.len() {
                    return false;
                }
                let moved = items.remove(*from);
                items.insert(*to, moved);
            }
            Request::Add(ids) => {
                for id in ids {
                    if !items.contains(&Some(*id)) {
                        items.push(Some(*id));
                    }
                }
            }
            Request::Restore {
                track,
                appended_at,
                to,
            } => {
                if *appended_at != items.len() || *to > items.len() {
                    return false;
                }
                items.push(Some(*track));
                let moved = items.remove(*appended_at);
                items.insert(*to, moved);
            }
        }
        true
    }
}

/// Waits before each new try after TIDAL refused an ETag (see [`send`]).
const RETRIES: [Duration; 6] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(750),
    Duration::from_millis(1000),
    Duration::from_millis(1500),
    Duration::from_millis(2000),
];

/// Sends `request` to version `etag` of the playlist, whose items are
/// `base` (if known). Returns the version after it, `None` if unknown (the
/// edit was made, but no other can be sent safely); [`Changed`] if the
/// playlist is no longer the one the edit was computed on (nothing was
/// edited).
///
/// TIDAL's ETags lag behind its writes: right after an edit it may refuse
/// the ETag it has just answered with (and the one it reports), and accept
/// an older one, for seconds. So a refusal is not taken as a change at
/// once: the playlist is read again, and if it holds exactly `base` (the
/// items the edit's positions were computed on) the edit is sent again
/// with the ETag TIDAL reports with them. Without `base`, a refusal is a
/// change.
pub async fn send(
    api: &Api,
    uuid: &str,
    request: &Request,
    etag: &str,
    base: Option<&Items>,
) -> Result<Option<String>> {
    let next = match request {
        Request::Remove { item } => {
            retrying(api, uuid, base, etag, |etag| async move {
                api.remove_from_playlist(uuid, &[*item], &etag).await
            })
            .await?
        }
        Request::Move { from, to } => {
            retrying(api, uuid, base, etag, |etag| async move {
                api.move_in_playlist(uuid, *from, *to, &etag).await
            })
            .await?
        }
        Request::Add(ids) => {
            retrying(api, uuid, base, etag, |etag| async move {
                append(api, uuid, ids, "SKIP", &etag).await
            })
            .await?
        }
        Request::Restore {
            track,
            appended_at,
            to,
        } => {
            let appended = retrying(api, uuid, base, etag, |etag| async move {
                append(api, uuid, &[*track], "ADD", &etag).await
            })
            .await?;
            let Some(appended) = next_version(api, uuid, appended).await else {
                return Err(Changed.into());
            };
            // It lands last; check that only if the items it was appended
            // to are not known.
            let with = base.filter(|base| base.len() == *appended_at).map(|base| {
                let mut with = base.clone();
                with.push(Some(*track));
                with
            });
            if with.is_none() && track_at(api, uuid, *appended_at).await? != Some(*track) {
                return Err(Changed.into());
            }
            if appended_at == to {
                Some(appended)
            } else {
                retrying(api, uuid, with.as_ref(), &appended, |etag| async move {
                    api.move_in_playlist(uuid, *appended_at, *to, &etag).await
                })
                .await?
            }
        }
    };
    Ok(next_version(api, uuid, next).await)
}

/// Runs `write` with `etag`, and again with the ETag TIDAL reports while
/// the playlist holds exactly `base`, as long as it refuses the ETag (see
/// [`send`]).
async fn retrying<F, Fut>(
    api: &Api,
    uuid: &str,
    base: Option<&Items>,
    etag: &str,
    write: F,
) -> Result<Option<String>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Option<String>>>,
{
    let refused = match write(etag.to_owned()).await.map_err(conflict) {
        Err(err) if is_changed(&err) => err,
        other => return other,
    };
    let Some(base) = base else {
        return Err(refused);
    };
    let mut tried = etag.to_owned();
    for wait in RETRIES {
        tokio::time::sleep(wait).await;
        let (items, now) = snapshot(api, uuid).await?;
        if items != *base {
            log::debug!("playlist {uuid}: not the edited version (yet)");
            continue;
        }
        // The same items: the edit's positions are right for them, under
        // whatever ETag TIDAL wants now (or, again, the one refused).
        let now = now.unwrap_or_else(|| tried.clone());
        log::debug!("playlist {uuid}: ETag {tried} refused, trying {now}");
        match write(now.clone()).await.map_err(conflict) {
            Err(err) if is_changed(&err) => tried = now,
            other => return other,
        }
    }
    Err(refused)
}

/// The version after a write: `etag` if TIDAL answered with one, otherwise
/// the playlist's ETag asked for right away.
async fn next_version(api: &Api, uuid: &str, etag: Option<String>) -> Option<String> {
    if etag.is_some() {
        return etag;
    }
    match api.playlist(uuid).await {
        Ok((_, etag)) => etag,
        Err(err) => {
            log::warn!("playlist {uuid}: no version after an edit: {err:#}");
            None
        }
    }
}

/// Appends tracks to version `etag` (`dupes`: "ADD" or "SKIP"); returns
/// the next ETag, if TIDAL names it.
async fn append(
    api: &Api,
    uuid: &str,
    track_ids: &[u64],
    dupes: &str,
    etag: &str,
) -> Result<Option<String>> {
    let ids = track_ids
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let call = Call::post(Base::V1, format!("playlists/{uuid}/items"))
        .form("trackIds", ids)
        .form("onDupes", dupes.to_owned())
        .header("If-None-Match", etag);
    Ok(api.write(call).await.map_err(conflict)?.etag())
}

/// An edit the queue holds.
pub trait Queued {
    /// What is sent to TIDAL for it.
    fn request(&self) -> Request;
}

impl Queued for Request {
    fn request(&self) -> Request {
        self.clone()
    }
}

/// What to do next with the queued edits ([`EditQueue::next`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Next<E> {
    /// Nothing to send.
    Idle,
    /// An edit is on its way: wait for its answer.
    Busy,
    /// Send `edit` to version `etag` of the playlist, whose items are
    /// `base` (if all are known; see [`send`]).
    Send {
        edit: E,
        etag: String,
        base: Option<Items>,
    },
    /// The version the queued edits were made on is not known (an answer
    /// named none) or not the list's any more: they were dropped, and come
    /// back newest first to be rolled back. Reload the list.
    Lost(Vec<E>),
}

/// The edits of a playlist waiting to be sent, and the version of the
/// playlist each applies to (no I/O: the page sends them).
///
/// Edits name items by position, so each must reach TIDAL with the ETag of
/// the version its positions were computed on: the version the list showed
/// when it was made. That is known once the loader reports it
/// ([`Update::Version`]); afterwards the answer to each edit names the
/// next one. Edits are only accepted while the version is known, and each
/// is stamped with the list's generation, which [`EditQueue::invalidate`]
/// bumps whenever the list is reloaded and replaced: the edits not sent by
/// then are dropped, and the answer to one on its way no longer names the
/// version shown.
///
/// It also follows the items of that version (as loaded, with the edits
/// TIDAL made), which [`send`] compares with what TIDAL has when it
/// refuses an ETag.
#[derive(Debug)]
pub struct EditQueue<E> {
    /// Bumped whenever the list shown is replaced.
    generation: u64,
    /// The version of the list shown is known (directly, or as the answer
    /// to the edit on its way).
    known: bool,
    /// The ETag to send the next edit with: the version shown, with the
    /// edits sent so far. `None` while unknown or while an edit is on its
    /// way.
    etag: Option<String>,
    /// The items of that version as far as loaded; `None` once they are
    /// not to be trusted.
    items: Option<Items>,
    /// Edits not sent yet, with the generation they were made on.
    pending: VecDeque<(u64, E)>,
    /// The edit on its way: its generation, the ETag it was sent with and
    /// what was sent.
    in_flight: Option<(u64, String, Request)>,
}

impl<E> Default for EditQueue<E> {
    fn default() -> Self {
        Self {
            generation: 0,
            known: false,
            etag: None,
            items: Some(Items::new()),
            pending: VecDeque::new(),
            in_flight: None,
        }
    }
}

impl<E: Queued> EditQueue<E> {
    /// Whether edits can be made now (the version of the list is known).
    pub fn can_edit(&self) -> bool {
        self.known
    }

    /// Nothing waiting and nothing on its way.
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_none() && self.pending.is_empty()
    }

    /// The ETag of the version shown, unless it is about to change.
    pub fn version(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// How many items TIDAL has (with the edits it made), if known.
    pub fn known_len(&self) -> Option<usize> {
        self.items.as_ref().map(Vec::len)
    }

    /// The list shows version `etag` (`None`: TIDAL did not say).
    pub fn set_version(&mut self, etag: Option<String>) {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|(g, ..)| *g == self.generation)
        {
            // The answer to the edit on its way names it.
            return;
        }
        self.known = etag.is_some();
        self.etag = etag;
    }

    /// More items of the version shown were loaded (after the ones known).
    pub fn extend(&mut self, chunk: &Chunk) {
        if let Some(items) = &mut self.items {
            items.extend(items_of(chunk));
        }
    }

    /// The list is about to be replaced: its version is unknown until
    /// [`Self::set_version`], its items arrive with [`Self::extend`].
    /// Returns the edits not sent yet (their positions are into the old
    /// list), newest first, to roll back.
    pub fn invalidate(&mut self) -> Vec<E> {
        self.generation += 1;
        self.known = false;
        self.etag = None;
        self.items = Some(Items::new());
        self.pending.drain(..).rev().map(|(_, edit)| edit).collect()
    }

    /// Queues an edit made on the list as shown. Refused (`Err` gives it
    /// back) while the list's version is not known.
    pub fn push(&mut self, edit: E) -> Result<(), E> {
        if !self.known {
            return Err(edit);
        }
        self.pending.push_back((self.generation, edit));
        Ok(())
    }

    pub fn next(&mut self) -> Next<E> {
        if self.in_flight.is_some() {
            return Next::Busy;
        }
        let Some((generation, _)) = self.pending.front() else {
            return Next::Idle;
        };
        match self.etag.take() {
            Some(etag) if *generation == self.generation && self.known => {
                let (generation, edit) = self.pending.pop_front().expect("checked above");
                self.in_flight = Some((generation, etag.clone(), edit.request()));
                Next::Send {
                    edit,
                    etag,
                    base: self.items.clone(),
                }
            }
            _ => {
                // Never send an edit with the ETag of another version.
                self.known = false;
                Next::Lost(self.pending.drain(..).rev().map(|(_, edit)| edit).collect())
            }
        }
    }

    /// TIDAL made the edit on its way; `etag` names the version after it.
    /// Returns whether it was made on the list shown (if not, that list
    /// was loaded meanwhile and may lack it: reload).
    pub fn applied(&mut self, etag: Option<String>) -> bool {
        let Some((generation, _, request)) = self.in_flight.take() else {
            return false;
        };
        if generation != self.generation {
            return false;
        }
        self.known = etag.is_some();
        self.etag = etag;
        self.items = self
            .items
            .take()
            .and_then(|mut items| request.apply(&mut items).then_some(items));
        true
    }

    /// TIDAL refused the edit on its way: back to the version it was sent
    /// to. Returns the edits queued after it, newest first, to roll back
    /// before it (they assumed it); `None` if the list was replaced
    /// meanwhile (nothing on screen to roll back).
    pub fn failed(&mut self) -> Option<Vec<E>> {
        let (generation, etag, _) = self.in_flight.take()?;
        if generation != self.generation {
            return None;
        }
        self.etag = Some(etag);
        self.known = true;
        Some(self.pending.drain(..).rev().map(|(_, edit)| edit).collect())
    }
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
        return Ok(());
    }

    // 2. Revalidate: unchanged details mean unchanged items.
    let fresh = match &cached {
        Some(cached) => match fetch_details(api, uuid).await {
            Ok((fresh, raw, etag)) => {
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
                    // The cached items are this version's.
                    let _ = tx.send(Update::Version(etag));
                    return Ok(());
                }
                fresh
            }
            // Offline: the cached copy is all there is.
            Err(err) if complete => {
                log::debug!("revalidating playlist {uuid} failed: {err:#}");
                return Ok(());
            }
            Err(err) => return Err(err),
        },
        None => {
            // Nothing cached: details and the first page together.
            let (details, first) = tokio::join!(fetch_details(api, uuid), fetch_raw(api, uuid, 0));
            let (fresh, raw, _) = details?;
            store(api, details_key(uuid), raw).await;
            let _ = tx.send(Update::Details(Box::new(fresh.clone())));
            let (value, raw, etag) = first?;
            let (chunk, total) = parse_items(&value);
            forget_items(api, uuid).await;
            store(api, items_key(uuid, &fresh, 0), raw).await;
            return fetch_rest(api, uuid, Some(&fresh), etag, 0, chunk, total, true, tx).await;
        }
    };

    // 3. The items changed (or were not all cached): fetch them all. When
    //    a stale copy is on screen, it is replaced at once at the end.
    forget_items(api, uuid).await;
    let (value, raw, etag) = fetch_raw(api, uuid, 0).await?;
    store(api, items_key(uuid, &fresh, 0), raw).await;
    let (chunk, total) = parse_items(&value);
    fetch_rest(
        api,
        uuid,
        Some(&fresh),
        etag,
        0,
        chunk,
        total,
        !items_shown,
        tx,
    )
    .await
}

/// Sends `first` (the page at `offset`, of version `etag`) and fetches the
/// pages after it, `CONCURRENCY` at a time, in order; a page of another
/// version fails with [`Changed`]. `progressive` sends each page as it
/// comes; otherwise everything is sent at the end, replacing what is shown.
/// With `cache_as`, pages are stored under that version.
#[allow(clippy::too_many_arguments)]
async fn fetch_rest(
    api: &Api,
    uuid: &str,
    cache_as: Option<&Playlist>,
    etag: Option<String>,
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
        if offset == 0 {
            let _ = tx.send(Update::Version(etag.clone()));
        }
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
            let _ = tx.send(Update::Version(etag));
        }
        return Ok(());
    }
    let offsets = ((offset + first_len)..total).step_by(PAGE);
    let mut pages = stream::iter(offsets)
        .map(|at| async move { (at, fetch_raw(api, uuid, at).await) })
        .buffered(CONCURRENCY);
    while let Some((at, page)) = pages.next().await {
        let (value, raw, version) = page?;
        if etag.is_some() && version != etag {
            log::debug!("playlist {uuid} changed while loading");
            return Err(Changed.into());
        }
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
        let _ = tx.send(Update::Version(etag));
    }
    Ok(())
}

/// The details, as sent (for the cache), and the ETag.
async fn fetch_details(api: &Api, uuid: &str) -> Result<(Playlist, Vec<u8>, Option<String>)> {
    let response = api
        .send(&Call::get(Base::V1, format!("playlists/{uuid}")))
        .await?;
    Ok((response.json()?, response.body.to_vec(), response.etag()))
}

/// A page of items, as sent (for the cache), and the playlist's ETag.
async fn fetch_raw(
    api: &Api,
    uuid: &str,
    offset: usize,
) -> Result<(Value, Vec<u8>, Option<String>)> {
    let call = Call::get(Base::V1, format!("playlists/{uuid}/items")).page(offset, PAGE);
    let response = api.send(&call).await?;
    let value = serde_json::from_slice(&response.body)
        .map_err(|err| anyhow!("TIDAL sent unreadable playlist items: {err}"))?;
    Ok((value, response.body.to_vec(), response.etag()))
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

    fn rm(item: usize) -> Request {
        Request::Remove { item }
    }

    fn send(queue: &mut EditQueue<Request>) -> (Request, String) {
        match queue.next() {
            Next::Send { edit, etag, .. } => (edit, etag),
            other => panic!("expected an edit to send, got {other:?}"),
        }
    }

    fn base(queue: &mut EditQueue<Request>) -> Option<Items> {
        match queue.next() {
            Next::Send { base, .. } => base,
            other => panic!("expected an edit to send, got {other:?}"),
        }
    }

    fn chunk(items: &[Option<u64>]) -> Chunk {
        Chunk {
            tracks: items
                .iter()
                .flatten()
                .map(|id| TrackInfo {
                    id: *id,
                    ..Default::default()
                })
                .collect(),
            slots: items.iter().map(Option::is_some).collect(),
        }
    }

    #[test]
    fn edits_wait_for_a_known_version() {
        let mut queue = EditQueue::default();
        assert!(!queue.can_edit());
        assert_eq!(queue.push(rm(3)), Err(rm(3)));
        assert_eq!(queue.next(), Next::Idle);
        queue.set_version(None);
        assert_eq!(queue.push(rm(3)), Err(rm(3)));
        queue.set_version(Some("v1".into()));
        assert!(queue.can_edit());
        assert_eq!(queue.push(rm(3)), Ok(()));
    }

    #[test]
    fn edits_are_sent_in_order_each_with_the_version_before_it() {
        let mut queue = EditQueue::default();
        queue.set_version(Some("v1".into()));
        queue.push(rm(1)).unwrap();
        queue.push(rm(2)).unwrap();
        assert_eq!(send(&mut queue), (rm(1), "v1".into()));
        // One at a time; edits made meanwhile are still accepted.
        assert_eq!(queue.next(), Next::Busy);
        queue.push(rm(3)).unwrap();
        assert_eq!(queue.version(), None);
        assert!(queue.applied(Some("v2".into())));
        assert_eq!(send(&mut queue), (rm(2), "v2".into()));
        assert!(queue.applied(Some("v3".into())));
        assert_eq!(send(&mut queue), (rm(3), "v3".into()));
        assert!(queue.applied(Some("v4".into())));
        assert_eq!(queue.next(), Next::Idle);
        assert!(queue.is_idle());
        assert_eq!(queue.version(), Some("v4"));
    }

    #[test]
    fn a_reload_drops_unsent_edits_and_the_old_versions() {
        let mut queue = EditQueue::default();
        queue.set_version(Some("v1".into()));
        queue.push(rm(1)).unwrap();
        queue.push(rm(2)).unwrap();
        queue.push(rm(3)).unwrap();
        assert_eq!(send(&mut queue), (rm(1), "v1".into()));
        // Reloaded while the first is on its way: the others were made on
        // the old list. Newest first, to roll back.
        assert_eq!(queue.invalidate(), vec![rm(3), rm(2)]);
        assert!(!queue.can_edit());
        assert_eq!(queue.push(rm(4)), Err(rm(4)));
        // The reloaded list's version arrives while the first is still on
        // its way.
        queue.set_version(Some("w1".into()));
        queue.push(rm(4)).unwrap();
        assert_eq!(queue.next(), Next::Busy);
        // The first was made, but not on the list shown now (which may
        // lack it), and its version is not the shown list's.
        assert!(!queue.applied(Some("v2".into())));
        assert_eq!(send(&mut queue), (rm(4), "w1".into()));
    }

    #[test]
    fn a_reload_before_the_version_is_known_again_blocks_sending() {
        let mut queue = EditQueue::default();
        queue.set_version(Some("v1".into()));
        queue.push(rm(1)).unwrap();
        assert_eq!(queue.invalidate(), vec![rm(1)]);
        assert_eq!(queue.next(), Next::Idle);
        queue.set_version(Some("v2".into()));
        assert_eq!(queue.next(), Next::Idle);
    }

    #[test]
    fn a_refusal_rolls_back_the_edits_that_assumed_it() {
        let mut queue = EditQueue::default();
        queue.set_version(Some("v1".into()));
        queue.push(rm(1)).unwrap();
        assert_eq!(send(&mut queue), (rm(1), "v1".into()));
        queue.push(rm(2)).unwrap();
        queue.push(rm(3)).unwrap();
        assert_eq!(queue.failed(), Some(vec![rm(3), rm(2)]));
        // Back at the version the first was sent to.
        assert!(queue.can_edit());
        assert_eq!(queue.version(), Some("v1"));
        queue.push(rm(4)).unwrap();
        assert_eq!(send(&mut queue), (rm(4), "v1".into()));
        // A refusal for a list since replaced rolls nothing back.
        queue.invalidate();
        assert_eq!(queue.failed(), None);
        assert!(!queue.can_edit());
    }

    #[test]
    fn an_answer_without_a_version_stops_the_queue() {
        let mut queue = EditQueue::default();
        queue.set_version(Some("v1".into()));
        queue.push(rm(1)).unwrap();
        queue.push(rm(2)).unwrap();
        assert_eq!(send(&mut queue), (rm(1), "v1".into()));
        assert!(queue.applied(None));
        assert!(!queue.can_edit());
        // Never sent with a guessed version.
        assert_eq!(queue.next(), Next::Lost(vec![rm(2)]));
        assert_eq!(queue.next(), Next::Idle);
        assert_eq!(queue.push(rm(3)), Err(rm(3)));
    }

    #[test]
    fn a_version_report_does_not_override_the_edit_on_its_way() {
        let mut queue = EditQueue::default();
        queue.set_version(Some("v1".into()));
        queue.push(rm(1)).unwrap();
        assert_eq!(send(&mut queue), (rm(1), "v1".into()));
        queue.set_version(Some("v1".into()));
        assert_eq!(queue.version(), None);
        assert!(queue.applied(Some("v2".into())));
        assert_eq!(queue.version(), Some("v2"));
    }

    #[test]
    fn each_edit_carries_the_items_it_was_computed_on() {
        let mut queue = EditQueue::default();
        queue.invalidate();
        queue.extend(&chunk(&[Some(1), None, Some(2)]));
        queue.set_version(Some("v1".into()));
        queue.extend(&chunk(&[Some(3), Some(4)]));
        queue.push(rm(0)).unwrap();
        queue.push(Request::Move { from: 3, to: 0 }).unwrap();
        queue.push(rm(9)).unwrap();
        assert_eq!(
            base(&mut queue),
            Some(vec![Some(1), None, Some(2), Some(3), Some(4)])
        );
        assert!(queue.applied(Some("v2".into())));
        assert_eq!(
            base(&mut queue),
            Some(vec![None, Some(2), Some(3), Some(4)])
        );
        // A refused edit leaves them as they were.
        assert_eq!(queue.failed(), Some(vec![rm(9)]));
        queue.push(Request::Move { from: 3, to: 0 }).unwrap();
        assert_eq!(
            base(&mut queue),
            Some(vec![None, Some(2), Some(3), Some(4)])
        );
        assert!(queue.applied(Some("v3".into())));
        assert_eq!(queue.known_len(), Some(4));
        // An edit that does not fit them makes them unknown.
        queue.push(rm(9)).unwrap();
        assert!(base(&mut queue).is_some());
        assert!(queue.applied(Some("v4".into())));
        queue.push(rm(0)).unwrap();
        assert_eq!(base(&mut queue), None);
        assert!(queue.applied(Some("v5".into())));
        // A reload starts them over.
        queue.invalidate();
        queue.extend(&chunk(&[Some(7)]));
        queue.set_version(Some("w1".into()));
        queue.push(rm(0)).unwrap();
        assert_eq!(base(&mut queue), Some(vec![Some(7)]));
    }

    #[test]
    fn requests_apply_like_tidal() {
        let mut items = vec![Some(1), None, Some(2), Some(3)];
        assert!(Request::Move { from: 0, to: 2 }.apply(&mut items));
        assert_eq!(items, vec![None, Some(2), Some(1), Some(3)]);
        assert!(Request::Move { from: 3, to: 0 }.apply(&mut items));
        assert_eq!(items, vec![Some(3), None, Some(2), Some(1)]);
        assert!(Request::Remove { item: 1 }.apply(&mut items));
        assert_eq!(items, vec![Some(3), Some(2), Some(1)]);
        // Duplicates skipped.
        assert!(Request::Add(vec![2, 5, 5, 6]).apply(&mut items));
        assert_eq!(items, vec![Some(3), Some(2), Some(1), Some(5), Some(6)]);
        // Undo: appended (even if there), then moved into place.
        let restore = Request::Restore {
            track: 2,
            appended_at: 5,
            to: 0,
        };
        assert!(restore.apply(&mut items));
        assert_eq!(
            items,
            vec![Some(2), Some(3), Some(2), Some(1), Some(5), Some(6)]
        );
        assert!(!Request::Remove { item: 6 }.apply(&mut items));
        assert!(!Request::Move { from: 0, to: 6 }.apply(&mut items));
        assert!(
            !Request::Restore {
                track: 1,
                appended_at: 2,
                to: 0
            }
            .apply(&mut items)
        );
    }

    #[test]
    fn items_of_a_chunk_keep_hidden_ones() {
        let items = vec![None, Some(1), None, Some(2)];
        assert_eq!(items_of(&chunk(&items)), items);
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
