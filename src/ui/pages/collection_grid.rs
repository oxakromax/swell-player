//! Collection grids (docs/design.md §3.5): favourite albums, followed
//! artists, playlists (own and favourited) and saved mixes, as virtualised
//! `GridView`s of [`MediaCard`]s in the shared collection template.
//!
//! - Albums and artists: cached in SQLite as one answer, shown at once and
//!   checked with one request (`Api::sync_favorite_list`).
//! - Playlists and mixes: what the library loaded at startup (fetched here
//!   only while the library has none), following its changes.
//!
//! Hearts flipped anywhere add or remove cards.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use anyhow::Result;
use gtk::{gio, glib};

use super::super::context::Ctx;
use super::super::dialogs;
use super::super::menus::ItemMenu;
use super::super::nav::CollectionView;
use super::super::widgets::media_card::MediaCard;
use super::super::widgets::section;
use super::super::widgets::{icon_name, label_button};
use super::collection::{
    SortOption, Template, count_label, empty_page, fold, follow_library, skeleton_cards,
};
use crate::api::{self, ListUpdate};
use crate::library::{self, Change};
use crate::runtime;
use crate::tidal::Favorite;
use crate::tidal::models::{Album, Artist, Mix, Playlist};
use crate::tidal::pages::Item;

const ALBUM_SORTS: &[SortOption] = &[
    ("added", "Date Added", true),
    ("title", "Title", false),
    ("artist", "Artist", false),
    ("date", "Release Date", true),
];
const ARTIST_SORTS: &[SortOption] = &[("added", "Date Added", true), ("title", "Name", false)];
const PLAYLIST_SORTS: &[SortOption] = &[("date", "Last Updated", true), ("title", "Title", false)];
const MIX_SORTS: &[SortOption] = &[("added", "Date Added", true), ("title", "Title", false)];

/// `playlistsAndFavoritePlaylists` serves at most 50 per page.
const PLAYLIST_PAGE: usize = 50;
/// A new favourite playlist or mix is listed after TIDAL has stored it.
const SETTLE: Duration = Duration::from_millis(1200);

/// One card with its folded texts for filtering and sorting.
struct Card {
    item: Item,
    favorite: library::Item,
    title: String,
    subtitle: String,
    /// Release date or last update (ISO 8601, so it sorts as text).
    date: String,
    own: bool,
}

impl Card {
    fn album(album: Album) -> Rc<Self> {
        let artist = album.artists.first().map(|a| a.name.as_str()).unwrap_or("");
        Rc::new(Self {
            favorite: library::Item::Album(album.id),
            title: fold(&album.title),
            subtitle: fold(artist),
            date: album.release_date.clone().unwrap_or_default(),
            own: false,
            item: Item::Album(Box::new(album)),
        })
    }

    fn artist(artist: Artist) -> Rc<Self> {
        Rc::new(Self {
            favorite: library::Item::Artist(artist.id),
            title: fold(&artist.name),
            subtitle: String::new(),
            date: String::new(),
            own: false,
            item: Item::Artist(Box::new(artist)),
        })
    }

    fn playlist(playlist: Playlist, user_id: u64) -> Rc<Self> {
        Rc::new(Self {
            favorite: library::Item::Playlist(playlist.uuid.clone()),
            title: fold(&playlist.title),
            subtitle: fold(playlist.creator.name.as_deref().unwrap_or("")),
            date: playlist
                .last_updated
                .clone()
                .or_else(|| playlist.last_item_added_at.clone())
                .or_else(|| playlist.created.clone())
                .unwrap_or_default(),
            own: playlist.creator.id == Some(user_id),
            item: Item::Playlist(Box::new(playlist)),
        })
    }

    fn mix(mix: Mix) -> Rc<Self> {
        Rc::new(Self {
            favorite: library::Item::Mix(mix.id.clone()),
            title: fold(&mix.title),
            subtitle: fold(&mix.subtitle),
            date: String::new(),
            own: false,
            item: Item::Mix(Box::new(mix)),
        })
    }

    fn name(&self) -> String {
        match &self.item {
            Item::Album(album) => album.title.clone(),
            Item::Artist(artist) => artist.name.clone(),
            Item::Playlist(playlist) => playlist.title.clone(),
            Item::Mix(mix) => mix.title.clone(),
            _ => String::new(),
        }
    }

    fn matches(&self, words: &[String]) -> bool {
        words
            .iter()
            .all(|word| self.title.contains(word.as_str()) || self.subtitle.contains(word.as_str()))
    }
}

/// Which playlists show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    All,
    Mine,
    Favourited,
}

struct GridPage {
    ctx: Weak<Ctx>,
    view: CollectionView,
    template: Rc<Template>,
    store: gio::ListStore,
    /// Every card, in TIDAL's order (newest first).
    cards: RefCell<Vec<Rc<Card>>>,
    group: Cell<Group>,
    /// The cards match TIDAL (an empty grid is really empty).
    synced: Cell<bool>,
    /// Cards were drawn once (errors after that are only toasted).
    drawn: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
}

pub(super) fn page(ctx: &Rc<Ctx>, view: CollectionView) -> adw::NavigationPage {
    let (name, title, placeholder, sorts) = match view {
        CollectionView::Albums => ("albums", "Albums", "Filter albums", ALBUM_SORTS),
        CollectionView::Artists => ("artists", "Artists", "Filter artists", ARTIST_SORTS),
        CollectionView::Playlists => ("playlists", "Playlists", "Filter playlists", PLAYLIST_SORTS),
        _ => ("mixes", "Mixes & Radios", "Filter mixes", MIX_SORTS),
    };

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let factory = gtk::SignalListItemFactory::new();
    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(store.clone())))
        .factory(&factory)
        .min_columns(1)
        .max_columns(24)
        .margin_top(6)
        .margin_bottom(24)
        .css_classes(["media-grid"])
        .build();
    grid.update_property(&[gtk::accessible::Property::Label(title)]);
    // Covers on the page's content edge (cells and cards have padding).
    section::align_to_edge(&grid, section::GRID_INSET);
    let menu = ItemMenu::new(ctx, &grid);
    bind_cards(ctx, &factory, &menu);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&grid)
        .build();
    watch_covers(ctx, &scrolled, &store);

    let (template, page) = Template::new(
        ctx,
        name,
        title,
        placeholder,
        sorts,
        &skeleton_cards(view == CollectionView::Artists),
        &scrolled,
    );
    template.capture_keys_from(&grid);
    template.set_empty(&empty_state(ctx, view));

    let state = Rc::new(GridPage {
        ctx: Rc::downgrade(ctx),
        view,
        template: template.clone(),
        store,
        cards: RefCell::default(),
        group: Cell::new(Group::All),
        synced: Cell::new(false),
        drawn: Cell::new(false),
        task: RefCell::default(),
    });

    if view == CollectionView::Playlists {
        state.add_playlist_controls(ctx);
    }
    let weak = Rc::downgrade(&state);
    template.connect_changed(move || {
        if let Some(state) = weak.upgrade() {
            state.apply();
        }
    });
    let weak = Rc::downgrade(&state);
    follow_library(ctx, &page, move |change| {
        if let Some(state) = weak.upgrade() {
            state.library_changed(change);
        }
    });

    state.load();
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take()
            && let Some(task) = state.task.take()
        {
            task.abort();
        }
    });
    page
}

/// Cards are built when bound and dropped when unbound (which cancels
/// their cover downloads): only what is on screen exists.
fn bind_cards(ctx: &Rc<Ctx>, factory: &gtk::SignalListItemFactory, menu: &Rc<ItemMenu>) {
    let weak_ctx = Rc::downgrade(ctx);
    let weak_menu = Rc::downgrade(menu);
    factory.connect_bind(move |_, object| {
        let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(ctx), Some(card)) = (
            weak_ctx.upgrade(),
            item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let card = card.borrow::<Rc<Card>>();
        let menu = weak_menu.upgrade();
        let widget = MediaCard::new(&ctx, &card.item, menu.as_ref())
            .widget()
            .clone();
        widget.set_halign(gtk::Align::Start);
        item.set_child(Some(&widget));
        item.set_activatable(false);
        item.set_focusable(false);
    });
    factory.connect_unbind(|_, object| {
        if let Some(item) = object.downcast_ref::<gtk::ListItem>() {
            item.set_child(None::<&gtk::Widget>);
        }
    });
}

/// Tells the cover loader which part of the grid is on screen.
fn watch_covers(ctx: &Rc<Ctx>, scrolled: &gtk::ScrolledWindow, store: &gio::ListStore) {
    let covers = Rc::downgrade(&ctx.covers);
    let store = store.downgrade();
    scrolled
        .vadjustment()
        .connect_value_changed(move |adjustment| {
            let (Some(covers), Some(store)) = (covers.upgrade(), store.upgrade()) else {
                return;
            };
            let upper = adjustment.upper();
            if upper > 0.0 {
                let middle = (adjustment.value() + adjustment.page_size() / 2.0) / upper;
                covers.set_focus((middle * f64::from(store.n_items())) as u32);
            }
        });
}

fn empty_state(ctx: &Rc<Ctx>, view: CollectionView) -> adw::StatusPage {
    match view {
        CollectionView::Albums => empty_page(
            "media-optical-symbolic",
            "No Favourite Albums",
            "Tap the heart on any album to save it here.",
            None,
        ),
        CollectionView::Artists => empty_page(
            "avatar-default-symbolic",
            "No Followed Artists",
            "Follow artists you like to find them here.",
            None,
        ),
        CollectionView::Playlists => {
            let button = gtk::Button::builder()
                .label("New Playlist")
                .css_classes(["suggested-action"])
                .build();
            let weak = Rc::downgrade(ctx);
            button.connect_clicked(move |_| {
                if let Some(ctx) = weak.upgrade() {
                    dialogs::playlist::new_playlist(&ctx, Vec::new());
                }
            });
            empty_page(
                "view-list-symbolic",
                "No Playlists",
                "Create a playlist, or tap the heart on someone else's to save it here.",
                Some(&button),
            )
        }
        _ => empty_page(
            icon_name("swell-mix-symbolic", "media-playlist-shuffle-symbolic"),
            "No Saved Mixes",
            "Tap the heart on any mix or radio to save it here.",
            None,
        ),
    }
}

impl GridPage {
    fn add_playlist_controls(self: &Rc<Self>, ctx: &Rc<Ctx>) {
        let new = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .build();
        label_button(&new, "New Playlist");
        let weak = Rc::downgrade(ctx);
        new.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                dialogs::playlist::new_playlist(&ctx, Vec::new());
            }
        });
        self.template.header_bar.pack_end(&new);

        let toggles = adw::ToggleGroup::builder().homogeneous(true).build();
        for (name, label) in [
            ("all", "All"),
            ("mine", "Created by Me"),
            ("favourited", "Favourited"),
        ] {
            toggles.add(adw::Toggle::builder().name(name).label(label).build());
        }
        toggles.set_active_name(Some("all"));
        let weak = Rc::downgrade(self);
        toggles.connect_active_name_notify(move |toggles| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            state.group.set(match toggles.active_name().as_deref() {
                Some("mine") => Group::Mine,
                Some("favourited") => Group::Favourited,
                _ => Group::All,
            });
            state.apply();
        });
        self.template.actions.append(&toggles);
        self.template.actions.set_visible(true);
        self.template.wrap_actions_when_compact();
    }

    fn load(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if !self.template.is_showing_content() {
            self.template.stack.show_loading();
        }
        let task = match self.view {
            CollectionView::Albums => self.sync(
                ctx.api.sync_favorite_list::<Album>(Favorite::Album),
                Card::album,
            ),
            CollectionView::Artists => self.sync(
                ctx.api.sync_favorite_list::<Artist>(Favorite::Artist),
                Card::artist,
            ),
            CollectionView::Playlists => {
                let playlists = ctx.library.playlists();
                if !playlists.is_empty() {
                    return self.set_playlists(playlists);
                }
                let api = ctx.api.clone();
                self.fetch(
                    async move { all_playlists(&api).await },
                    |state, playlists| state.set_playlists(playlists),
                )
            }
            _ => {
                let mixes = ctx.library.favorite_mixes();
                if !mixes.is_empty() {
                    return self.set_mixes(mixes);
                }
                let api = ctx.api.clone();
                self.fetch(async move { all_mixes(&api).await }, |state, mixes| {
                    state.set_mixes(mixes)
                })
            }
        };
        self.task.replace(Some(task));
    }

    /// Follows a favourite-list sync.
    fn sync<T: 'static>(
        self: &Rc<Self>,
        mut updates: tokio::sync::mpsc::UnboundedReceiver<Result<ListUpdate<T>>>,
        card: fn(T) -> Rc<Card>,
    ) -> glib::JoinHandle<()> {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(update) = updates.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                match update {
                    Ok(ListUpdate::Cached(items) | ListUpdate::Replace(items)) => {
                        state.cards.replace(items.into_iter().map(card).collect());
                        state.apply();
                    }
                    Ok(ListUpdate::Append(items)) => {
                        let cards: Vec<Rc<Card>> = items.into_iter().map(card).collect();
                        state.cards.borrow_mut().extend(cards.iter().cloned());
                        if state.template.is_natural() {
                            let objects: Vec<glib::BoxedAnyObject> =
                                cards.into_iter().map(glib::BoxedAnyObject::new).collect();
                            state.store.splice(state.store.n_items(), 0, &objects);
                            state.refresh_state();
                        } else {
                            state.apply();
                        }
                    }
                    Ok(ListUpdate::Finished) => {
                        state.synced.set(true);
                        state.refresh_state();
                    }
                    Err(err) => return state.failed(&err),
                }
            }
        })
    }

    /// Runs a one-off fetch (while the library has nothing to offer).
    fn fetch<T: Send + 'static>(
        self: &Rc<Self>,
        request: impl Future<Output = Result<T>> + Send + 'static,
        show: fn(&Rc<Self>, T),
    ) -> glib::JoinHandle<()> {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = runtime::spawn(request).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(items) => show(&state, items),
                Err(err) => state.failed(&err),
            }
        })
    }

    fn failed(self: &Rc<Self>, err: &anyhow::Error) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if crate::tidal::is_auth_error(err) || self.drawn.get() {
            ctx.report("Couldn't update your collection", err);
            return;
        }
        let weak = Rc::downgrade(self);
        self.template.stack.show_error(err, move || {
            if let Some(state) = weak.upgrade() {
                state.load();
            }
        });
    }

    fn set_playlists(self: &Rc<Self>, playlists: Vec<Playlist>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let user = ctx.api.user_id();
        self.cards.replace(
            playlists
                .into_iter()
                .map(|playlist| Card::playlist(playlist, user))
                .collect(),
        );
        self.synced.set(true);
        self.apply();
    }

    fn set_mixes(self: &Rc<Self>, mixes: Vec<Mix>) {
        self.cards
            .replace(mixes.into_iter().map(Card::mix).collect());
        self.synced.set(true);
        self.apply();
    }

    /// Filters and sorts the cards into the grid.
    fn apply(&self) {
        let words = self.template.query();
        let (sort, descending) = self.template.sort();
        let group = self.group.get();
        let cards = self.cards.borrow();
        let mut shown: Vec<&Rc<Card>> = cards
            .iter()
            .filter(|card| match group {
                Group::All => true,
                Group::Mine => card.own,
                Group::Favourited => !card.own,
            })
            .filter(|card| card.matches(&words))
            .collect();
        match sort {
            // Cards come newest first.
            "added" => {
                if !descending {
                    shown.reverse();
                }
            }
            other => shown.sort_by(|a, b| {
                let order = match other {
                    "title" => a.title.cmp(&b.title),
                    "artist" => a.subtitle.cmp(&b.subtitle).then(a.date.cmp(&b.date)),
                    _ => a.date.cmp(&b.date).then(a.title.cmp(&b.title)),
                };
                if descending { order.reverse() } else { order }
            }),
        }
        let objects: Vec<glib::BoxedAnyObject> = shown
            .into_iter()
            .map(|card| glib::BoxedAnyObject::new(card.clone()))
            .collect();
        drop(cards);
        self.store.splice(0, self.store.n_items(), &objects);
        self.refresh_state();
    }

    fn noun(&self) -> (&'static str, &'static str) {
        match self.view {
            CollectionView::Albums => ("album", "albums"),
            CollectionView::Artists => ("artist", "artists"),
            CollectionView::Playlists => ("playlist", "playlists"),
            _ => ("mix", "mixes"),
        }
    }

    fn refresh_state(&self) {
        let total = self.cards.borrow().len();
        if total == 0 && !self.synced.get() {
            // Nothing cached: keep loading until TIDAL answers.
            return;
        }
        self.drawn.set(true);
        let shown = self.store.n_items() as usize;
        let (one, many) = self.noun();
        self.template
            .title
            .set_subtitle(&count_label(shown, total, one, many));
        self.template.show_items(shown, total);
    }

    fn library_changed(self: &Rc<Self>, change: &Change) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        match (self.view, change) {
            (CollectionView::Playlists, Change::Playlists) => {
                self.set_playlists(ctx.library.playlists());
            }
            (CollectionView::Mixes, Change::Loaded) => self.set_mixes(ctx.library.favorite_mixes()),
            (_, Change::Favorite(item, false)) => self.removed(item),
            (CollectionView::Albums, Change::Favorite(library::Item::Album(id), true)) => {
                let (api, id) = (ctx.api.clone(), *id);
                self.added(
                    library::Item::Album(id),
                    async move { api.album(id).await },
                    Card::album,
                );
            }
            (CollectionView::Artists, Change::Favorite(library::Item::Artist(id), true)) => {
                let (api, id) = (ctx.api.clone(), *id);
                self.added(
                    library::Item::Artist(id),
                    async move { api.artist(id).await },
                    Card::artist,
                );
            }
            (CollectionView::Playlists, Change::Favorite(library::Item::Playlist(_), true)) => {
                let library = Rc::downgrade(&ctx.library);
                glib::timeout_add_local_once(SETTLE, move || {
                    if let Some(library) = library.upgrade() {
                        library.refresh_playlists();
                    }
                });
            }
            (CollectionView::Mixes, Change::Favorite(library::Item::Mix(_), true)) => {
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(SETTLE, move || {
                    let Some(state) = weak.upgrade() else {
                        return;
                    };
                    let Some(ctx) = state.ctx.upgrade() else {
                        return;
                    };
                    let api = ctx.api.clone();
                    let task = state.fetch(async move { all_mixes(&api).await }, |state, mixes| {
                        state.set_mixes(mixes)
                    });
                    if let Some(old) = state.task.replace(Some(task)) {
                        old.abort();
                    }
                });
            }
            _ => {}
        }
    }

    /// A favourite went away: its card goes, with "Undo".
    fn removed(&self, item: &library::Item) {
        let Some(position) = self
            .cards
            .borrow()
            .iter()
            .position(|card| &card.favorite == item && !card.own)
        else {
            return;
        };
        let card = self.cards.borrow_mut().remove(position);
        let index = (0..self.store.n_items()).find(|&i| {
            self.store
                .item(i)
                .and_downcast::<glib::BoxedAnyObject>()
                .is_some_and(|object| Rc::ptr_eq(&object.borrow::<Rc<Card>>(), &card))
        });
        if let Some(index) = index {
            self.store.remove(index);
        }
        self.refresh_state();
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if !self.template.is_mapped() {
            return;
        }
        let weak = Rc::downgrade(&ctx);
        let item = item.clone();
        let what = if matches!(item, library::Item::Artist(_)) {
            "Unfollowed"
        } else {
            "Removed from Favourites:"
        };
        ctx.toast_with_undo(
            &format!("{what} {}", glib::markup_escape_text(&card.name())),
            move || {
                if let Some(ctx) = weak.upgrade() {
                    let report = Rc::downgrade(&ctx);
                    ctx.library.set_favorite(item.clone(), true, move |result| {
                        if let (Err(err), Some(ctx)) = (result, report.upgrade()) {
                            ctx.report("Couldn't undo that", &err);
                        }
                    });
                }
            },
        );
    }

    /// A new favourite: fetch it and put it first.
    fn added<T: Send + 'static>(
        self: &Rc<Self>,
        item: library::Item,
        request: impl Future<Output = Result<T>> + Send + 'static,
        card: fn(T) -> Rc<Card>,
    ) {
        if self.cards.borrow().iter().any(|c| c.favorite == item) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = runtime::spawn(request).await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            let Some(ctx) = state.ctx.upgrade() else {
                return;
            };
            match result {
                // Unless it was undone meanwhile, or a sync listed it.
                Ok(value)
                    if ctx.library.is_favorite(&item)
                        && !state.cards.borrow().iter().any(|c| c.favorite == item) =>
                {
                    state.cards.borrow_mut().insert(0, card(value));
                    state.apply();
                }
                Ok(_) => {}
                Err(err) => log::warn!("could not fetch new favourite {item:?}: {err:#}"),
            }
        });
    }
}

async fn all_playlists(api: &api::Api) -> Result<Vec<Playlist>> {
    let first = api.user_playlists(0, PLAYLIST_PAGE).await?;
    let mut all = first.items;
    while all.len() < first.total {
        let page = api.user_playlists(all.len(), PLAYLIST_PAGE).await?;
        if page.items.is_empty() {
            break;
        }
        all.extend(page.items);
    }
    Ok(all)
}

async fn all_mixes(api: &api::Api) -> Result<Vec<Mix>> {
    let mut page = api.favorite_mixes(None).await?;
    let mut all = std::mem::take(&mut page.items);
    while let Some(cursor) = page.cursor.take() {
        page = api.favorite_mixes(Some(&cursor)).await?;
        if page.items.is_empty() {
            break;
        }
        all.append(&mut page.items);
    }
    Ok(all)
}
