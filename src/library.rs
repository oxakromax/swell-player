//! The user's library as the whole UI sees it: which items are favourites
//! (one `favorites/ids` request answers that for everything) and the
//! user's own playlists (for the sidebar and "Add to playlist").
//!
//! Main thread only. Changes are optimistic: the heart flips at once and
//! flips back with an error if TIDAL refuses.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gtk::glib;

use crate::api::Api;
use crate::runtime;
use crate::tidal::models::{Mix, Playlist};
use crate::tidal::{Favorite, MAX_PAGE};

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

pub struct Library {
    api: Arc<Api>,
    favorites: RefCell<HashSet<Item>>,
    playlists: RefCell<Vec<Playlist>>,
    mixes: RefCell<Vec<Mix>>,
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_id: Cell<u64>,
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
        })
    }

    pub fn connect_changed(&self, listener: impl Fn(&Change) + 'static) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.listeners.borrow_mut().push((id, Rc::new(listener)));
        id
    }

    pub fn disconnect(&self, id: u64) {
        self.listeners.borrow_mut().retain(|(other, _)| *other != id);
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
            let (ids, mixes, playlists) = tokio::join!(
                api.favorite_ids(),
                api.favorite_mixes(None),
                all_playlists(&api)
            );
            Ok((ids?, mixes.map(|p| p.items).unwrap_or_default(), playlists?))
        })
        .await?;
        let mut favorites = HashSet::new();
        favorites.extend(ids.track.iter().filter_map(|id| id.parse().ok()).map(Item::Track));
        favorites.extend(ids.album.iter().filter_map(|id| id.parse().ok()).map(Item::Album));
        favorites.extend(ids.artist.iter().filter_map(|id| id.parse().ok()).map(Item::Artist));
        favorites.extend(ids.playlist.iter().cloned().map(Item::Playlist));
        favorites.extend(mixes.iter().map(|mix| Item::Mix(mix.id.clone())));
        self.favorites.replace(favorites);
        self.mixes.replace(mixes);
        self.playlists.replace(playlists);
        self.emit(Change::Loaded);
        self.emit(Change::Playlists);
        Ok(())
    }

    pub fn is_favorite(&self, item: &Item) -> bool {
        self.favorites.borrow().contains(item)
    }

    /// Adds or removes a favourite. `done` gets the error, if any, after the
    /// change was rolled back.
    pub fn set_favorite(
        self: &Rc<Self>,
        item: Item,
        favorite: bool,
        done: impl FnOnce(Result<()>) + 'static,
    ) {
        if self.is_favorite(&item) == favorite {
            return done(Ok(()));
        }
        self.apply(&item, favorite);
        let this = self.clone();
        let api = self.api.clone();
        let request = item.clone();
        glib::spawn_future_local(async move {
            let result = runtime::spawn(async move {
                match &request {
                    Item::Mix(id) => api.set_mix_favorite(id, favorite).await,
                    other => {
                        let (kind, id) = match other {
                            Item::Track(id) => (Favorite::Track, id.to_string()),
                            Item::Album(id) => (Favorite::Album, id.to_string()),
                            Item::Artist(id) => (Favorite::Artist, id.to_string()),
                            Item::Playlist(id) => (Favorite::Playlist, id.clone()),
                            Item::Mix(_) => unreachable!(),
                        };
                        if favorite {
                            api.add_favorite(kind, &id).await
                        } else {
                            api.remove_favorite(kind, &id).await
                        }
                    }
                }
            })
            .await;
            if result.is_err() {
                this.apply(&item, !favorite);
            }
            done(result);
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
        let user = self.api.user_id();
        self.playlists
            .borrow()
            .iter()
            .filter(|p| p.creator.id == Some(user))
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

    pub fn is_own_playlist(&self, playlist: &Playlist) -> bool {
        playlist.creator.id == Some(self.api.user_id())
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

async fn all_playlists(api: &Api) -> Result<Vec<Playlist>> {
    let first = api.user_playlists(0, MAX_PAGE).await?;
    let mut all = first.items;
    let mut offset = MAX_PAGE;
    while offset < first.total {
        let page = api.user_playlists(offset, MAX_PAGE).await?;
        if page.items.is_empty() {
            break;
        }
        all.extend(page.items);
        offset += MAX_PAGE;
    }
    Ok(all)
}
