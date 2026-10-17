//! The user's library as the whole UI sees it: which items are favourites
//! (one `favorites/ids` request answers that for everything) and the
//! user's own playlists (for the sidebar and "Add to playlist").
//!
//! Main thread only. Changes are optimistic: the heart flips at once and
//! flips back with an error if TIDAL refuses. Each item has at most one
//! request in flight; toggles made meanwhile are coalesced into one request
//! for the latest wish (see [`Writes`]).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gtk::glib;

use crate::api::Api;
use crate::runtime;
use crate::tidal::Favorite;
use crate::tidal::models::{Mix, Playlist};

/// `playlistsAndFavoritePlaylists` serves at most 50 per page.
const PLAYLIST_PAGE: usize = 50;

/// Anything that can be a favourite.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Track(u64),
    Album(u64),
    Artist(u64),
    Playlist(String),
    Mix(String),
}

#[derive(Clone, Debug)]
pub enum Change {
    /// A favourite was added (true) or removed (false).
    Favorite(Item, bool),
    /// The set of favourites was (re)loaded.
    Loaded,
    /// The user's playlists changed.
    Playlists,
}

type Listener = Rc<dyn Fn(&Change)>;
type Done = Box<dyn FnOnce(Result<()>)>;

pub struct Library {
    api: Arc<Api>,
    favorites: RefCell<HashSet<Item>>,
    playlists: RefCell<Vec<Playlist>>,
    mixes: RefCell<Vec<Mix>>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_id: Cell<u64>,
    loaded: Cell<bool>,
    writes: RefCell<Writes<Done>>,
}

impl Library {
    pub fn new(api: Arc<Api>) -> Rc<Self> {
        Rc::new(Self {
            api,
            favorites: RefCell::default(),
            playlists: RefCell::default(),
            mixes: RefCell::default(),
            listeners: RefCell::default(),
            next_id: Cell::new(1),
            loaded: Cell::new(false),
            writes: RefCell::new(Writes::default()),
        })
    }

    pub fn connect_changed(&self, listener: impl Fn(&Change) + 'static) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(listener)));
        id
    }

    pub fn disconnect(&self, id: u64) {
        self.listeners
            .borrow_mut()
            .retain(|(other, _)| *other != id);
    }

    fn emit(&self, change: Change) {
        let listeners: Vec<Listener> = self
            .listeners
            .borrow()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(&change);
        }
    }

    /// Loads favourite ids, favourite mixes and the user's playlists.
    pub async fn load(self: Rc<Self>) -> Result<()> {
        let api = self.api.clone();
        let (ids, mixes, playlists) = runtime::spawn(async move {
            let (ids, mixes, playlists) =
                tokio::join!(api.favorite_ids(), all_mixes(&api), all_playlists(&api));
            Ok((ids?, mixes.unwrap_or_default(), playlists?))
        })
        .await?;
        let mut favorites = HashSet::new();
        favorites.extend(
            ids.track
                .iter()
                .filter_map(|id| id.parse().ok())
                .map(Item::Track),
        );
        favorites.extend(
            ids.album
                .iter()
                .filter_map(|id| id.parse().ok())
                .map(Item::Album),
        );
        favorites.extend(
            ids.artist
                .iter()
                .filter_map(|id| id.parse().ok())
                .map(Item::Artist),
        );
        favorites.extend(ids.playlist.iter().cloned().map(Item::Playlist));
        favorites.extend(mixes.iter().map(|mix| Item::Mix(mix.id.clone())));
        // Toggles still being written keep showing what the user asked for.
        for (item, desired) in self.writes.borrow().desired() {
            if desired {
                favorites.insert(item.clone());
            } else {
                favorites.remove(item);
            }
        }
        self.favorites.replace(favorites);
        self.mixes.replace(mixes);
        self.playlists.replace(playlists);
        self.loaded.set(true);
        self.emit(Change::Loaded);
        self.emit(Change::Playlists);
        Ok(())
    }

    /// Whether [`Library::load`] has finished once (favourites and
    /// playlists are known).
    pub fn is_loaded(&self) -> bool {
        self.loaded.get()
    }

    /// Number of favourites of a kind (for counters).
    pub fn favorite_count(&self, kind: fn(&Item) -> bool) -> usize {
        self.favorites
            .borrow()
            .iter()
            .filter(|item| kind(item))
            .count()
    }

    pub fn is_favorite(&self, item: &Item) -> bool {
        self.favorites.borrow().contains(item)
    }

    /// Adds or removes a favourite, showing the change at once. `done`
    /// runs once TIDAL has settled it: with the error if the change was
    /// rolled back. A toggle superseded by a newer one for the same item
    /// gets `Ok` (only the latest reports a failure).
    pub fn set_favorite(
        self: &Rc<Self>,
        item: Item,
        favorite: bool,
        done: impl FnOnce(Result<()>) + 'static,
    ) {
        let shown = self.is_favorite(&item);
        let step = self
            .writes
            .borrow_mut()
            .request(&item, shown, favorite, Box::new(done));
        match step {
            Request::Unchanged(done) => done(Ok(())),
            Request::Wait => self.apply(&item, favorite),
            Request::Send => {
                self.apply(&item, favorite);
                self.send(item, favorite);
            }
        }
    }

    fn send(self: &Rc<Self>, item: Item, favorite: bool) {
        let this = self.clone();
        let api = self.api.clone();
        glib::spawn_future_local(async move {
            let request = item.clone();
            let result =
                runtime::spawn(async move { write_favorite(&api, &request, favorite).await }).await;
            let outcome = this.writes.borrow_mut().finished(&item, result.is_ok());
            match outcome {
                Outcome::Send(next) => this.send(item, next),
                Outcome::Settled {
                    state,
                    failed,
                    mut waiters,
                } => {
                    if let Err(err) = &result
                        && !failed
                    {
                        log::debug!("superseded favourite change failed: {err:#}");
                    }
                    this.apply(&item, state);
                    let latest = waiters.pop();
                    for done in waiters {
                        done(Ok(()));
                    }
                    if let Some(done) = latest {
                        done(if failed { result } else { Ok(()) });
                    }
                }
            }
        });
    }

    fn apply(&self, item: &Item, favorite: bool) {
        let changed = if favorite {
            self.favorites.borrow_mut().insert(item.clone())
        } else {
            self.favorites.borrow_mut().remove(item)
        };
        if changed {
            self.emit(Change::Favorite(item.clone(), favorite));
        }
    }

    /// Playlists the user owns (can add tracks to), newest first.
    pub fn own_playlists(&self) -> Vec<Playlist> {
        self.playlists
            .borrow()
            .iter()
            .filter(|p| self.is_own_playlist(p))
            .cloned()
            .collect()
    }

    /// Own and favourited playlists, newest first.
    pub fn playlists(&self) -> Vec<Playlist> {
        self.playlists.borrow().clone()
    }

    pub fn favorite_mixes(&self) -> Vec<Mix> {
        self.mixes.borrow().clone()
    }

    /// False while the user's id is unknown (editorial playlists have
    /// creator 0, which must never count as the user's).
    pub fn is_own_playlist(&self, playlist: &Playlist) -> bool {
        self.api
            .known_user_id()
            .is_some_and(|user| playlist.creator.id == Some(user))
    }

    /// Reloads the playlist list (after creating, editing or deleting one).
    pub fn refresh_playlists(self: &Rc<Self>) {
        let this = self.clone();
        let api = self.api.clone();
        glib::spawn_future_local(async move {
            match runtime::spawn(async move { all_playlists(&api).await }).await {
                Ok(playlists) => {
                    this.playlists.replace(playlists);
                    this.emit(Change::Playlists);
                }
                Err(err) => log::warn!("could not reload playlists: {err:#}"),
            }
        });
    }
}

async fn write_favorite(api: &Api, item: &Item, favorite: bool) -> Result<()> {
    let (kind, id) = match item {
        Item::Mix(id) => return api.set_mix_favorite(id, favorite).await,
        Item::Track(id) => (Favorite::Track, id.to_string()),
        Item::Album(id) => (Favorite::Album, id.to_string()),
        Item::Artist(id) => (Favorite::Artist, id.to_string()),
        Item::Playlist(id) => (Favorite::Playlist, id.clone()),
    };
    if favorite {
        api.add_favorite(kind, &id).await
    } else {
        api.remove_favorite(kind, &id).await
    }
}

/// The favourite writes in flight, one per item at most. While one is in
/// flight, newer toggles only update the wanted state; when it ends, a
/// request for that state follows if it differs from what TIDAL now has.
/// A failure rolls back to the last state TIDAL confirmed. `W` is whatever
/// waits for the outcome (callbacks in the app, plain values in tests).
struct Writes<W> {
    pending: HashMap<Item, Write<W>>,
}

impl<W> Default for Writes<W> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }
}

struct Write<W> {
    /// The state TIDAL last confirmed (or the one shown before the first
    /// toggle).
    confirmed: bool,
    /// The state being sent.
    sending: bool,
    /// The state the user wants now.
    desired: bool,
    waiters: Vec<W>,
}

#[derive(Debug)]
enum Request<W> {
    /// Already in that state, nothing in flight: answer at once.
    Unchanged(W),
    /// Send a request for the wanted state.
    Send,
    /// A request is in flight; the outcome comes with it.
    Wait,
}

#[derive(Debug, PartialEq)]
enum Outcome<W> {
    /// Send another request, for this state.
    Send(bool),
    /// Done: show `state`; `failed` if it is not what the user wanted.
    Settled {
        state: bool,
        failed: bool,
        waiters: Vec<W>,
    },
}

impl<W> Writes<W> {
    /// The user wants `item` to be `desired`; `shown` is its state now.
    fn request(&mut self, item: &Item, shown: bool, desired: bool, waiter: W) -> Request<W> {
        if let Some(write) = self.pending.get_mut(item) {
            write.desired = desired;
            write.waiters.push(waiter);
            return Request::Wait;
        }
        if shown == desired {
            return Request::Unchanged(waiter);
        }
        self.pending.insert(
            item.clone(),
            Write {
                confirmed: shown,
                sending: desired,
                desired,
                waiters: vec![waiter],
            },
        );
        Request::Send
    }

    /// The request in flight for `item` succeeded (`ok`) or failed.
    fn finished(&mut self, item: &Item, ok: bool) -> Outcome<W> {
        let Some(write) = self.pending.get_mut(item) else {
            return Outcome::Settled {
                state: false,
                failed: !ok,
                waiters: Vec::new(),
            };
        };
        if ok {
            write.confirmed = write.sending;
            if write.desired != write.confirmed {
                write.sending = write.desired;
                return Outcome::Send(write.desired);
            }
        }
        let write = self.pending.remove(item).expect("pending write");
        Outcome::Settled {
            state: write.confirmed,
            failed: write.desired != write.confirmed,
            waiters: write.waiters,
        }
    }

    /// Items with a write in flight and the state the user wants for them.
    fn desired(&self) -> impl Iterator<Item = (&Item, bool)> {
        self.pending
            .iter()
            .map(|(item, write)| (item, write.desired))
    }
}

/// Every favourite mix (v2 pages by cursor, 50 at a time).
async fn all_mixes(api: &Api) -> Result<Vec<Mix>> {
    let mut page = api.favorite_mixes(None).await?;
    let mut all = std::mem::take(&mut page.items);
    // A generous bound in case the cursor never ends.
    for _ in 0..40 {
        let Some(cursor) = page.cursor.take() else {
            break;
        };
        page = api.favorite_mixes(Some(&cursor)).await?;
        if page.items.is_empty() {
            break;
        }
        all.append(&mut page.items);
    }
    Ok(all)
}

async fn all_playlists(api: &Api) -> Result<Vec<Playlist>> {
    let first = api.user_playlists(0, PLAYLIST_PAGE).await?;
    let mut all = first.items;
    let mut offset = PLAYLIST_PAGE;
    while offset < first.total {
        let page = api.user_playlists(offset, PLAYLIST_PAGE).await?;
        if page.items.is_empty() {
            break;
        }
        all.extend(page.items);
        offset += PLAYLIST_PAGE;
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: Item = Item::Track(1);

    fn settled(state: bool, failed: bool, waiters: Vec<u32>) -> Outcome<u32> {
        Outcome::Settled {
            state,
            failed,
            waiters,
        }
    }

    #[test]
    fn one_toggle() {
        let mut writes = Writes::default();
        assert!(matches!(
            writes.request(&ITEM, false, false, 1),
            Request::Unchanged(1)
        ));
        assert!(matches!(
            writes.request(&ITEM, false, true, 1),
            Request::Send
        ));
        assert_eq!(writes.finished(&ITEM, true), settled(true, false, vec![1]));
        assert_eq!(writes.desired().count(), 0);
    }

    #[test]
    fn failure_rolls_back() {
        let mut writes = Writes::default();
        writes.request(&ITEM, true, false, 1);
        assert_eq!(writes.finished(&ITEM, false), settled(true, true, vec![1]));
    }

    #[test]
    fn toggles_in_flight_are_coalesced() {
        let mut writes = Writes::default();
        assert!(matches!(
            writes.request(&ITEM, false, true, 1),
            Request::Send
        ));
        assert!(matches!(
            writes.request(&ITEM, true, false, 2),
            Request::Wait
        ));
        assert!(matches!(
            writes.request(&ITEM, false, true, 3),
            Request::Wait
        ));
        assert_eq!(writes.desired().collect::<Vec<_>>(), [(&ITEM, true)]);
        // TIDAL already has what the user wants last: nothing more to send.
        assert_eq!(
            writes.finished(&ITEM, true),
            settled(true, false, vec![1, 2, 3])
        );
    }

    #[test]
    fn the_latest_wish_is_sent_next() {
        let mut writes = Writes::default();
        writes.request(&ITEM, false, true, 1);
        writes.request(&ITEM, true, false, 2);
        assert_eq!(writes.finished(&ITEM, true), Outcome::Send(false));
        assert!(matches!(
            writes.request(&ITEM, false, false, 3),
            Request::Wait
        ));
        assert_eq!(
            writes.finished(&ITEM, true),
            settled(false, false, vec![1, 2, 3])
        );
    }

    #[test]
    fn a_failed_follow_up_rolls_back_to_the_last_confirmed_state() {
        let mut writes = Writes::default();
        writes.request(&ITEM, false, true, 1);
        writes.request(&ITEM, true, false, 2);
        assert_eq!(writes.finished(&ITEM, true), Outcome::Send(false));
        // The add went through, the remove did not: TIDAL has it.
        assert_eq!(
            writes.finished(&ITEM, false),
            settled(true, true, vec![1, 2])
        );
    }

    #[test]
    fn a_failure_the_user_already_undid_is_not_an_error() {
        let mut writes = Writes::default();
        writes.request(&ITEM, false, true, 1);
        writes.request(&ITEM, true, false, 2);
        assert_eq!(
            writes.finished(&ITEM, false),
            settled(false, false, vec![1, 2])
        );
    }

    #[test]
    fn items_are_independent() {
        let mut writes = Writes::default();
        let other = Item::Album(1);
        assert!(matches!(
            writes.request(&ITEM, false, true, 1),
            Request::Send
        ));
        assert!(matches!(
            writes.request(&other, false, true, 2),
            Request::Send
        ));
        assert_eq!(
            writes.finished(&other, false),
            settled(false, true, vec![2])
        );
        assert_eq!(writes.finished(&ITEM, true), settled(true, false, vec![1]));
    }
}
