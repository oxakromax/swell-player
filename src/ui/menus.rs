//! Context menus for every kind of object (docs/design.md §4.11) and the
//! playback helpers they share with buttons and cards.
//!
//! One [`ItemMenu`] per list, grid or header: a single `GtkPopoverMenu`
//! parented to that widget plus an `item` action group installed on it. The
//! object the menu is about ([`Subject`]) is stored right before the popover
//! is shown and every action acts on it. Menu models are tiny and rebuilt
//! per popup, so items that do not apply are simply left out.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;
use anyhow::Result;
use gtk::{gdk, gio, glib};

use super::context::{Ctx, PanelTab};
use super::dialogs;
use super::nav::Route;
use crate::api::Api;
use crate::i18n::{tr, trn};
use crate::library;
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::runtime;
use crate::tidal::models::{Album, Artist, Cursor, Mix, Playlist};
use crate::tidal::pages::{self, Target};

/// Track lists fetched for "Play" on a collection stop here.
const MAX_TRACKS: usize = 2000;
const PAGE: usize = 100;

/// What a menu (or a play button) acts on.
#[derive(Clone, Debug)]
pub enum Subject {
    /// A track; `index` is its place in the list the menu belongs to (for
    /// "Remove from This Playlist" and "Move Up/Down").
    Track {
        track: TrackInfo,
        index: Option<usize>,
    },
    Album(Rc<Album>),
    Artist(Rc<Artist>),
    Playlist(Rc<Playlist>),
    Mix(Rc<Mix>),
}

impl Subject {
    pub fn track(track: TrackInfo) -> Self {
        Self::Track { track, index: None }
    }

    /// The subject of a page item (links have none).
    pub fn from_item(item: &pages::Item) -> Option<Self> {
        Some(match item {
            pages::Item::Track(track) => Self::track(TrackInfo::from((**track).clone())),
            pages::Item::Album(album) => Self::Album(Rc::new((**album).clone())),
            pages::Item::Artist(artist) => Self::Artist(Rc::new((**artist).clone())),
            pages::Item::Playlist(playlist) => Self::Playlist(Rc::new((**playlist).clone())),
            pages::Item::Mix(mix) => Self::Mix(Rc::new((**mix).clone())),
            pages::Item::Link(_) => return None,
        })
    }

    pub fn title(&self) -> String {
        match self {
            Self::Track { track, .. } => track.title.clone(),
            Self::Album(album) => album.title.clone(),
            Self::Artist(artist) => artist.name.clone(),
            Self::Playlist(playlist) => playlist.title.clone(),
            Self::Mix(mix) => mix.title.clone(),
        }
    }

    /// The library item its heart toggles.
    pub fn favorite_item(&self) -> library::Item {
        match self {
            Self::Track { track, .. } => library::Item::Track(track.id),
            Self::Album(album) => library::Item::Album(album.id),
            Self::Artist(artist) => library::Item::Artist(artist.id),
            Self::Playlist(playlist) => library::Item::Playlist(playlist.uuid.clone()),
            Self::Mix(mix) => library::Item::Mix(mix.id.clone()),
        }
    }

    pub fn share_url(&self) -> String {
        match self {
            Self::Track { track, .. } => track.share_url(),
            Self::Album(album) => format!("https://tidal.com/browse/album/{}", album.id),
            Self::Artist(artist) => format!("https://tidal.com/browse/artist/{}", artist.id),
            Self::Playlist(playlist) => {
                format!("https://tidal.com/browse/playlist/{}", playlist.uuid)
            }
            Self::Mix(mix) => format!("https://tidal.com/browse/mix/{}", mix.id),
        }
    }

    /// Where its tracks come from, for "Playing from …".
    pub fn source(&self) -> Source {
        let target = match self {
            Self::Track { track, .. } => Some(Target::Album(track.album_id)),
            Self::Album(album) => Some(Target::Album(album.id)),
            Self::Artist(artist) => Some(Target::Artist(artist.id)),
            Self::Playlist(playlist) => Some(Target::Playlist(playlist.uuid.clone())),
            Self::Mix(mix) => Some(Target::Mix(mix.id.clone())),
        };
        let title = match self {
            Self::Track { track, .. } => track.album.clone(),
            other => other.title(),
        };
        Source { title, target }
    }

    /// How to fetch its tracks (`None` for a track: it is its own list).
    pub fn query(&self) -> Option<TrackQuery> {
        Some(match self {
            Self::Track { .. } => return None,
            Self::Album(album) => TrackQuery::Album(album.id),
            Self::Artist(artist) => TrackQuery::ArtistTop(artist.id),
            Self::Playlist(playlist) => TrackQuery::Playlist(playlist.uuid.clone()),
            Self::Mix(mix) => TrackQuery::Mix(mix.id.clone()),
        })
    }
}

/// The tracks of a collection, fetched on the I/O runtime.
#[derive(Clone, Debug)]
pub enum TrackQuery {
    Album(u64),
    Playlist(String),
    Mix(String),
    /// An artist's top tracks (up to 100).
    ArtistTop(u64),
}

impl TrackQuery {
    pub async fn fetch(self, api: Arc<Api>) -> Result<Vec<TrackInfo>> {
        runtime::spawn(async move {
            let mut tracks = Vec::new();
            match self {
                // Page by what TIDAL sent, not by what parsed: videos and
                // malformed entries are dropped, which would re-fetch the
                // tail of every page.
                Self::Album(id) => {
                    let mut cursor = Cursor::default();
                    loop {
                        let page = api.album_tracks(id, cursor.offset, PAGE).await?;
                        cursor.advance(page.sent, PAGE, page.total);
                        tracks.extend(page.items.into_iter().map(TrackInfo::from));
                        if !cursor.has_more() || tracks.len() >= MAX_TRACKS {
                            break;
                        }
                    }
                }
                Self::Playlist(uuid) => {
                    let mut cursor = Cursor::default();
                    loop {
                        let page = api.playlist_tracks(&uuid, cursor.offset, PAGE).await?;
                        cursor.advance(page.sent, PAGE, page.total);
                        tracks.extend(page.items.into_iter().map(TrackInfo::from));
                        if !cursor.has_more() || tracks.len() >= MAX_TRACKS {
                            break;
                        }
                    }
                }
                Self::Mix(id) => {
                    let page = api.mix_tracks(&id, 0, PAGE).await?;
                    tracks.extend(page.items.into_iter().map(TrackInfo::from));
                }
                Self::ArtistTop(id) => {
                    let page = api.artist_top_tracks(id, 0, PAGE).await?;
                    tracks.extend(page.items.into_iter().map(TrackInfo::from));
                }
            }
            Ok(tracks)
        })
        .await
    }
}

/// What to do with a subject's tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    Play,
    Shuffle,
    Next,
    Queue,
}

/// Plays (or queues) a subject's tracks, fetching them first if needed.
pub fn play(ctx: &Rc<Ctx>, subject: &Subject, mode: PlayMode) {
    let source = subject.source();
    let Some(query) = subject.query() else {
        if let Subject::Track { track, .. } = subject {
            apply(ctx, vec![track.clone()], source, mode);
        }
        return;
    };
    // Held weakly across the fetch: a sign-out meanwhile drops this
    // session's context, and nothing may reach it afterwards.
    let api = ctx.api.clone();
    let weak = Rc::downgrade(ctx);
    glib::spawn_future_local(async move {
        let result = query.fetch(api).await;
        let Some(ctx) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(tracks) if tracks.is_empty() => ctx.toast(tr("There's nothing to play here")),
            Ok(tracks) => apply(&ctx, tracks, source, mode),
            Err(err) => ctx.report(tr("Couldn't load the tracks"), &err),
        }
    });
}

fn apply(ctx: &Rc<Ctx>, tracks: Vec<TrackInfo>, source: Source, mode: PlayMode) {
    match mode {
        PlayMode::Play => play_tracks(ctx, tracks, 0, source, false),
        PlayMode::Shuffle => shuffle_tracks(ctx, tracks, source),
        PlayMode::Next => enqueue(ctx, tracks, true),
        PlayMode::Queue => enqueue(ctx, tracks, false),
    }
}

/// Plays `tracks` from `start`, leaving out tracks that are not available.
/// With `strict`, an unavailable `start` is refused with a toast; otherwise
/// the next available track plays.
pub fn play_tracks(
    ctx: &Rc<Ctx>,
    tracks: Vec<TrackInfo>,
    start: usize,
    source: Source,
    strict: bool,
) {
    if strict
        && let Some(track) = tracks.get(start)
        && !track.available
    {
        ctx.toast(
            &tr("“{title}” isn't available in your region")
                .replace("{title}", &glib::markup_escape_text(&track.title)),
        );
        return;
    }
    let start_id = tracks[start.min(tracks.len().saturating_sub(1))..]
        .iter()
        .find(|t| t.available)
        .map(|t| t.id);
    let playable: Vec<TrackInfo> = tracks.into_iter().filter(|t| t.available).collect();
    if playable.is_empty() {
        ctx.toast(tr("None of these tracks are available in your region"));
        return;
    }
    let index = start_id
        .and_then(|id| playable.iter().position(|t| t.id == id))
        .unwrap_or(0);
    ctx.playback.play_tracks(playable, index, source);
}

/// Turns shuffle on and plays `tracks` from a random track.
pub fn shuffle_tracks(ctx: &Rc<Ctx>, tracks: Vec<TrackInfo>, source: Source) {
    let playable: Vec<TrackInfo> = tracks.into_iter().filter(|t| t.available).collect();
    if playable.is_empty() {
        ctx.toast(tr("None of these tracks are available in your region"));
        return;
    }
    ctx.playback.set_shuffle(true);
    let start = glib::random_int_range(0, playable.len() as i32) as usize;
    ctx.playback.play_tracks(playable, start, source);
}

/// "Play Next" / "Add to Queue", with a toast that opens the queue.
pub fn enqueue(ctx: &Rc<Ctx>, tracks: Vec<TrackInfo>, next: bool) {
    let playable: Vec<TrackInfo> = tracks.into_iter().filter(|t| t.available).collect();
    let message = match playable.as_slice() {
        [] => {
            ctx.toast(tr("None of these tracks are available in your region"));
            return;
        }
        [one] if next => tr("“{title}” will play next").replace("{title}", &one.title),
        [one] => tr("Added “{title}” to the queue").replace("{title}", &one.title),
        many => trn(
            "Added {n} track to the queue",
            "Added {n} tracks to the queue",
            many.len() as u64,
        )
        .replace("{n}", &many.len().to_string()),
    };
    if next {
        ctx.playback.play_next(playable);
    } else {
        ctx.playback.add_to_queue(playable);
    }
    let weak = Rc::downgrade(ctx);
    toast_with_action(ctx, &message, tr("Show"), move || {
        if let Some(ctx) = weak.upgrade() {
            ctx.show_panel(Some(PanelTab::Queue));
        }
    });
}

/// A toast with a button (8 s, as every toast with an action).
pub fn toast_with_action(ctx: &Ctx, message: &str, label: &str, action: impl Fn() + 'static) {
    let toast = adw::Toast::builder()
        .title(glib::markup_escape_text(message).as_str())
        .button_label(label)
        .timeout(8)
        .build();
    toast.connect_button_clicked(move |_| action());
    ctx.toasts.add_toast(toast);
}

/// Plays a single track from its id (links to tracks in pages).
pub fn play_track_id(ctx: &Rc<Ctx>, id: u64) {
    let api = ctx.api.clone();
    let weak = Rc::downgrade(ctx);
    glib::spawn_future_local(async move {
        let result = runtime::spawn(async move { api.track(id).await }).await;
        let Some(ctx) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(track) => {
                let track = TrackInfo::from(track);
                let source = Subject::track(track.clone()).source();
                play_tracks(&ctx, vec![track], 0, source, true);
            }
            Err(err) => ctx.report(tr("Couldn't play this track"), &err),
        }
    });
}

type IndexHook = Rc<dyn Fn(usize)>;
type MoveHook = Rc<dyn Fn(usize, usize)>;
type LenHook = Rc<dyn Fn() -> usize>;

/// One popover menu and its `item` action group, for one list/grid/header.
pub struct ItemMenu {
    ctx: Weak<Ctx>,
    parent: glib::WeakRef<gtk::Widget>,
    popover: gtk::PopoverMenu,
    group: gio::SimpleActionGroup,
    subject: RefCell<Option<Subject>>,
    /// "Remove from This Playlist" (own playlists).
    remove: RefCell<Option<IndexHook>>,
    /// "Move Up" / "Move Down" (reorderable lists) and the list length.
    move_to: RefCell<Option<(MoveHook, LenHook)>>,
}

impl ItemMenu {
    /// A menu shown over `parent`, which keeps it alive.
    pub fn new(ctx: &Rc<Ctx>, parent: &impl IsA<gtk::Widget>) -> Rc<Self> {
        let parent = parent.upcast_ref::<gtk::Widget>();
        let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        popover.set_parent(parent);
        let this = Rc::new(Self {
            ctx: Rc::downgrade(ctx),
            parent: parent.downgrade(),
            popover,
            group: gio::SimpleActionGroup::new(),
            subject: RefCell::default(),
            remove: RefCell::default(),
            move_to: RefCell::default(),
        });
        this.add_actions();
        parent.insert_action_group("item", Some(&this.group));
        let keep = RefCell::new(Some(this.clone()));
        parent.connect_destroy(move |_| {
            if let Some(menu) = keep.take() {
                menu.popover.unparent();
            }
        });
        this
    }

    /// Enables "Remove from This Playlist" for tracks with an index.
    pub fn set_remove_handler(&self, remove: impl Fn(usize) + 'static) {
        self.remove.replace(Some(Rc::new(remove)));
    }

    /// Enables "Move Up" / "Move Down" for tracks with an index; `len` is
    /// the number of tracks in the list.
    pub fn set_move_handler(
        &self,
        move_to: impl Fn(usize, usize) + 'static,
        len: impl Fn() -> usize + 'static,
    ) {
        self.move_to.replace(Some((Rc::new(move_to), Rc::new(len))));
    }

    /// Shows the menu for `subject`, pointing at `rect` (in the parent's
    /// coordinates; `None`: the whole parent).
    pub fn popup(&self, subject: Subject, rect: Option<gdk::Rectangle>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let model = self.model(&ctx, &subject);
        self.subject.replace(Some(subject));
        self.popover.set_menu_model(Some(&model));
        self.popover
            .set_has_arrow(rect.is_some_and(|r| r.width() > 1));
        self.popover.set_pointing_to(rect.as_ref());
        self.popover.popup();
    }

    /// Shows the menu pointing at `anchor` (a "⋯" button, a row).
    pub fn popup_at(&self, subject: Subject, anchor: &impl IsA<gtk::Widget>) {
        let rect = self.parent.upgrade().and_then(|parent| {
            let bounds = anchor.compute_bounds(&parent)?;
            Some(gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                (bounds.width() as i32).max(2),
                bounds.height() as i32,
            ))
        });
        self.popup(subject, rect);
    }

    /// Shows the menu at a point of the parent (a right click).
    pub fn popup_at_point(&self, subject: Subject, x: f64, y: f64) {
        self.popup(subject, Some(gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    fn model(&self, ctx: &Rc<Ctx>, subject: &Subject) -> gio::Menu {
        let menu = gio::Menu::new();
        let section = |items: &[(&str, &str)]| {
            let section = gio::Menu::new();
            for (label, action) in items {
                section.append(Some(label), Some(action));
            }
            section
        };
        let favorite = ctx.library.is_favorite(&subject.favorite_item());
        let favorite_label = if favorite {
            tr("Remove from Favourites")
        } else {
            tr("Add to Favourites")
        };
        match subject {
            Subject::Track { track, index } => {
                menu.append_section(
                    None,
                    &section(&[
                        (tr("Play Next"), "item.play-next"),
                        (tr("Add to Queue"), "item.add-to-queue"),
                    ]),
                );
                let library = section(&[]);
                library.append_submenu(Some(tr("Add to Playlist")), &playlists_menu(ctx));
                library.append(Some(favorite_label), Some("item.toggle-favorite"));
                menu.append_section(None, &library);
                let go = gio::Menu::new();
                if track.album_id != 0 {
                    go.append(Some(tr("Go to Album")), Some("item.go-album"));
                }
                match track.artists.as_slice() {
                    [] => {}
                    [artist] => go.append_item(&target_item(
                        tr("Go to Artist"),
                        "item.go-artist",
                        artist.id,
                    )),
                    artists => {
                        let submenu = gio::Menu::new();
                        for artist in artists {
                            submenu.append_item(&target_item(
                                &artist.name,
                                "item.go-artist",
                                artist.id,
                            ));
                        }
                        go.append_submenu(Some(tr("Go to Artist")), &submenu);
                    }
                }
                go.append(Some(tr("Track Radio")), Some("item.track-radio"));
                menu.append_section(None, &go);
                menu.append_section(
                    None,
                    &section(&[
                        (tr("Credits"), "item.credits"),
                        (tr("Copy Link"), "item.copy-link"),
                    ]),
                );
                if let Some(index) = index {
                    let edit = gio::Menu::new();
                    if self.remove.borrow().is_some() {
                        edit.append(Some(tr("Remove from This Playlist")), Some("item.remove"));
                    }
                    if let Some((_, len)) = self.move_to.borrow().as_ref() {
                        if *index > 0 {
                            edit.append(Some(tr("Move Up")), Some("item.move-up"));
                        }
                        if index + 1 < len() {
                            edit.append(Some(tr("Move Down")), Some("item.move-down"));
                        }
                    }
                    if edit.n_items() > 0 {
                        menu.append_section(None, &edit);
                    }
                }
                if let Some(artist) = track.main_artist() {
                    let block = gio::Menu::new();
                    block.append_item(&target_item(
                        &tr("Block {name}").replace("{name}", &artist.name),
                        "item.block-artist",
                        artist.id,
                    ));
                    menu.append_section(None, &block);
                }
            }
            Subject::Album(album) => {
                menu.append_section(None, &play_section());
                let library = section(&[(favorite_label, "item.toggle-favorite")]);
                library.append_submenu(Some(tr("Add to Playlist")), &playlists_menu(ctx));
                menu.append_section(None, &library);
                if let Some(artist) = album.artists.first() {
                    let go = gio::Menu::new();
                    go.append_item(&target_item(
                        tr("Go to Artist"),
                        "item.go-artist",
                        artist.id,
                    ));
                    go.append_item(&target_item(
                        tr("Artist Radio"),
                        "item.artist-radio",
                        artist.id,
                    ));
                    menu.append_section(None, &go);
                }
                menu.append_section(None, &section(&[(tr("Copy Link"), "item.copy-link")]));
            }
            Subject::Playlist(playlist) if ctx.library.is_own_playlist(playlist) => {
                menu.append_section(None, &play_section());
                let edit = section(&[(tr("Edit Details…"), "item.edit-playlist")]);
                let public = gio::MenuItem::new(
                    Some(if playlist.public_playlist {
                        tr("Make Private")
                    } else {
                        tr("Make Public")
                    }),
                    None,
                );
                public.set_action_and_target_value(
                    Some("item.set-public"),
                    Some(&(!playlist.public_playlist).to_variant()),
                );
                edit.append_item(&public);
                menu.append_section(None, &edit);
                menu.append_section(None, &section(&[(tr("Copy Link"), "item.copy-link")]));
                menu.append_section(
                    None,
                    &section(&[(tr("Delete Playlist…"), "item.delete-playlist")]),
                );
            }
            Subject::Playlist(_) | Subject::Mix(_) => {
                menu.append_section(None, &play_section());
                menu.append_section(None, &section(&[(favorite_label, "item.toggle-favorite")]));
                menu.append_section(None, &section(&[(tr("Copy Link"), "item.copy-link")]));
            }
            Subject::Artist(artist) => {
                let play = section(&[
                    (tr("Play Top Tracks"), "item.play"),
                    (tr("Shuffle"), "item.shuffle"),
                ]);
                play.append_item(&target_item(
                    tr("Artist Radio"),
                    "item.artist-radio",
                    artist.id,
                ));
                menu.append_section(None, &play);
                menu.append_section(
                    None,
                    &section(&[(
                        if favorite {
                            tr("Unfollow")
                        } else {
                            tr("Follow")
                        },
                        "item.toggle-favorite",
                    )]),
                );
                menu.append_section(None, &section(&[(tr("Copy Link"), "item.copy-link")]));
                let block = gio::Menu::new();
                block.append_item(&target_item(
                    tr("Block Artist"),
                    "item.block-artist",
                    artist.id,
                ));
                menu.append_section(None, &block);
            }
        }
        menu
    }

    fn add_actions(self: &Rc<Self>) {
        let add = |name: &str,
                   parameter: Option<&glib::VariantTy>,
                   f: fn(&Self, &Rc<Ctx>, Subject, Option<&glib::Variant>)| {
            let action = gio::SimpleAction::new(name, parameter);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, parameter| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let (Some(ctx), Some(subject)) =
                    (this.ctx.upgrade(), this.subject.borrow().clone())
                else {
                    return;
                };
                f(&this, &ctx, subject, parameter);
            });
            self.group.add_action(&action);
        };
        add("play", None, |_, ctx, subject, _| {
            play(ctx, &subject, PlayMode::Play)
        });
        add("shuffle", None, |_, ctx, subject, _| {
            play(ctx, &subject, PlayMode::Shuffle)
        });
        add("play-next", None, |_, ctx, subject, _| {
            play(ctx, &subject, PlayMode::Next)
        });
        add("add-to-queue", None, |_, ctx, subject, _| {
            play(ctx, &subject, PlayMode::Queue)
        });
        add("toggle-favorite", None, |_, ctx, subject, _| {
            let item = subject.favorite_item();
            let favorite = !ctx.library.is_favorite(&item);
            let weak = Rc::downgrade(ctx);
            ctx.library.set_favorite(item, favorite, move |result| {
                if let (Err(err), Some(ctx)) = (result, weak.upgrade()) {
                    ctx.report(tr("Couldn't update favourites"), &err);
                }
            });
        });
        add(
            "add-to-playlist",
            Some(glib::VariantTy::STRING),
            |_, ctx, subject, parameter| {
                let Some(uuid) = parameter.and_then(|p| p.get::<String>()) else {
                    return;
                };
                let Some(playlist) = ctx
                    .library
                    .own_playlists()
                    .into_iter()
                    .find(|p| p.uuid == uuid)
                else {
                    return;
                };
                with_track_ids(ctx, &subject, move |ctx, ids| {
                    dialogs::playlist::add_to_playlist(ctx, &playlist, ids)
                });
            },
        );
        add("new-playlist", None, |_, ctx, subject, _| {
            with_track_ids(ctx, &subject, |ctx, ids| {
                dialogs::playlist::new_playlist(ctx, ids)
            });
        });
        add("choose-playlist", None, |_, ctx, subject, _| {
            with_track_ids(ctx, &subject, |ctx, ids| {
                dialogs::playlist::choose_playlist(ctx, ids)
            });
        });
        add("go-album", None, |_, ctx, subject, _| {
            if let Subject::Track { track, .. } = subject {
                ctx.navigate(Route::Album(track.album_id));
            }
        });
        add(
            "go-artist",
            Some(glib::VariantTy::UINT64),
            |_, ctx, _, parameter| {
                if let Some(id) = parameter.and_then(|p| p.get::<u64>()) {
                    ctx.navigate(Route::Artist(id));
                }
            },
        );
        add("track-radio", None, |_, ctx, subject, _| {
            if let Subject::Track { track, .. } = subject {
                ctx.navigate(Route::TrackRadio(track.id));
            }
        });
        add(
            "artist-radio",
            Some(glib::VariantTy::UINT64),
            |_, ctx, _, parameter| {
                if let Some(id) = parameter.and_then(|p| p.get::<u64>()) {
                    ctx.navigate(Route::ArtistRadio(id));
                }
            },
        );
        add("credits", None, |_, ctx, subject, _| {
            if let Subject::Track { track, .. } = subject {
                dialogs::credits::show(ctx, &track);
            }
        });
        add("copy-link", None, |_, ctx, subject, _| {
            ctx.copy(&subject.share_url(), tr("Link copied"));
        });
        add("remove", None, |this, _, subject, _| {
            let hook = this.remove.borrow().clone();
            if let (
                Some(hook),
                Subject::Track {
                    index: Some(index), ..
                },
            ) = (hook, subject)
            {
                hook(index);
            }
        });
        add("move-up", None, |this, _, subject, _| {
            let hook = this.move_to.borrow().clone();
            if let (
                Some((hook, _)),
                Subject::Track {
                    index: Some(index), ..
                },
            ) = (hook, subject)
                && index > 0
            {
                hook(index, index - 1);
            }
        });
        add("move-down", None, |this, _, subject, _| {
            let hook = this.move_to.borrow().clone();
            if let (
                Some((hook, len)),
                Subject::Track {
                    index: Some(index), ..
                },
            ) = (hook, subject)
                && index + 1 < len()
            {
                hook(index, index + 1);
            }
        });
        add("edit-playlist", None, |_, ctx, subject, _| {
            if let Subject::Playlist(playlist) = subject {
                dialogs::playlist::edit_playlist(ctx, &playlist);
            }
        });
        add(
            "set-public",
            Some(glib::VariantTy::BOOLEAN),
            |_, ctx, subject, parameter| {
                if let (Subject::Playlist(playlist), Some(public)) =
                    (subject, parameter.and_then(|p| p.get::<bool>()))
                {
                    dialogs::playlist::set_public(ctx, &playlist, public);
                }
            },
        );
        add("delete-playlist", None, |_, ctx, subject, _| {
            if let Subject::Playlist(playlist) = subject {
                // An open page of it leaves by itself when the library
                // drops the playlist.
                dialogs::playlist::delete_playlist(ctx, &playlist, || {});
            }
        });
        add(
            "block-artist",
            Some(glib::VariantTy::UINT64),
            |_, ctx, subject, parameter| {
                let Some(id) = parameter.and_then(|p| p.get::<u64>()) else {
                    return;
                };
                let name = match &subject {
                    Subject::Artist(artist) => artist.name.clone(),
                    Subject::Track { track, .. } => track
                        .artists
                        .iter()
                        .find(|a| a.id == id)
                        .map(|a| a.name.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                block_artist(ctx, id, name, true);
            },
        );
    }
}

fn play_section() -> gio::Menu {
    let section = gio::Menu::new();
    for (label, action) in [
        (tr("Play"), "item.play"),
        (tr("Shuffle"), "item.shuffle"),
        (tr("Play Next"), "item.play-next"),
        (tr("Add to Queue"), "item.add-to-queue"),
    ] {
        section.append(Some(label), Some(action));
    }
    section
}

fn target_item(label: &str, action: &str, id: u64) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&id.to_variant()));
    item
}

/// "Add to Playlist ▸": New Playlist…, the 8 newest own playlists, More….
fn playlists_menu(ctx: &Ctx) -> gio::Menu {
    let menu = gio::Menu::new();
    let new = gio::Menu::new();
    new.append(Some(tr("New Playlist…")), Some("item.new-playlist"));
    menu.append_section(None, &new);
    let own = ctx.library.own_playlists();
    let recent = gio::Menu::new();
    for playlist in own.iter().take(8) {
        let item = gio::MenuItem::new(Some(&playlist.title), None);
        item.set_action_and_target_value(
            Some("item.add-to-playlist"),
            Some(&playlist.uuid.to_variant()),
        );
        recent.append_item(&item);
    }
    if recent.n_items() > 0 {
        menu.append_section(None, &recent);
    }
    if own.len() > 8 {
        let more = gio::Menu::new();
        more.append(Some(tr("More…")), Some("item.choose-playlist"));
        menu.append_section(None, &more);
    }
    menu
}

/// Runs `f` with the subject's track ids (fetching a collection's first).
fn with_track_ids(ctx: &Rc<Ctx>, subject: &Subject, f: impl FnOnce(&Rc<Ctx>, Vec<u64>) + 'static) {
    let Some(query) = subject.query() else {
        if let Subject::Track { track, .. } = subject {
            f(ctx, vec![track.id]);
        }
        return;
    };
    let api = ctx.api.clone();
    let weak = Rc::downgrade(ctx);
    glib::spawn_future_local(async move {
        let result = query.fetch(api).await;
        let Some(ctx) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(tracks) => f(&ctx, tracks.into_iter().map(|t| t.id).collect()),
            Err(err) => ctx.report(tr("Couldn't load the tracks"), &err),
        }
    });
}

/// Blocks (or unblocks) an artist, with Undo.
fn block_artist(ctx: &Rc<Ctx>, id: u64, name: String, blocked: bool) {
    let api = ctx.api.clone();
    let weak = Rc::downgrade(ctx);
    glib::spawn_future_local(async move {
        let result = runtime::spawn(async move { api.set_artist_blocked(id, blocked).await }).await;
        let Some(ctx) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(()) if blocked => {
                let weak = Rc::downgrade(&ctx);
                let name_again = name.clone();
                ctx.toast_with_undo(
                    &tr("Blocked {name}").replace("{name}", &glib::markup_escape_text(&name)),
                    move || {
                        if let Some(ctx) = weak.upgrade() {
                            block_artist(&ctx, id, name_again.clone(), false);
                        }
                    },
                );
            }
            Ok(()) => ctx
                .toast(&tr("Unblocked {name}").replace("{name}", &glib::markup_escape_text(&name))),
            Err(err) => ctx.report(tr("Couldn't update blocked artists"), &err),
        }
    });
}
