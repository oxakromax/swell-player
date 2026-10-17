//! The playlist page (docs/design.md §3.9), the user's own and others'.
//!
//! One virtualised list is the whole page: the header is its first item
//! and a footer item shows paging progress, a paging error with "Retry" or
//! the empty state. Loading ([`super::playlist_data`]) shows the cached copy
//! at once, then the first 100 items, then the rest a few requests at a
//! time. Others' playlists are trusted for 30 minutes; own ones are always
//! revalidated (one small request when nothing changed).
//!
//! Play, Shuffle and activating a row start as soon as every track is
//! loaded: the playback queue has no way to grow its context afterwards,
//! and starting with a partial list would silently drop the rest. Loading
//! is fast (4 requests in flight), so this only shows on very long
//! playlists opened for the first time.
//!
//! Own playlists can be edited in place: remove (menu, Delete; with Undo),
//! reorder (drag handle, Alt+↑/↓, menu) and add by dropping tracks on the
//! list. Edits are optimistic and sent to TIDAL one at a time, in order; a
//! failed edit is rolled back (or, if others were queued behind it, the
//! playlist is reloaded). Edit Details, Make Public/Private and Delete live
//! in the header (dialogs in `dialogs::playlist`); the page follows the
//! library's change notifications to refresh itself or leave once its
//! playlist is deleted.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::super::context::Ctx;
use super::super::menus;
use super::super::nav::{CollectionView, Route};
use super::super::sidebar;
use super::super::widgets::page_header::PageHeader;
use super::super::widgets::section;
use super::super::widgets::skeleton;
use super::super::widgets::status::{self, PageStack};
use super::super::widgets::track_list::{TrackList, Variant, compact_bin};
use super::super::widgets::track_row::{TrackDrag, TrackRow};
use super::playlist_data::{self as data, Slots, Update};
use crate::i18n::{tr, trn};
use crate::library::Change;
use crate::model::TrackInfo;
use crate::playback::Source;
use crate::runtime;
use crate::tidal::models::Playlist;
use crate::tidal::pages::Target;

/// How long others' playlists are shown from the cache without asking.
const OTHERS_FRESH_FOR: Duration = Duration::from_secs(30 * 60);

/// An edit of an own playlist, already applied on screen. List indices
/// are positions in the track list; item indices count hidden items too.
#[derive(Clone)]
enum Edit {
    Remove {
        index: usize,
        item: usize,
        track: TrackInfo,
    },
    /// Undo of a removal: append, then move into place.
    Restore {
        index: usize,
        item: usize,
        appended_at: usize,
        track: TrackInfo,
    },
    Move {
        from: usize,
        to: usize,
        item_from: usize,
        item_to: usize,
    },
    Add(Vec<u64>),
}

/// Reloads in a row because the playlist kept changing while loading,
/// after which the page settles for what it has.
const MAX_RELOADS: u32 = 3;

/// What the list's last item shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Footer {
    None,
    Loading,
    Failed,
    Empty,
}

struct PlaylistPage {
    ctx: Weak<Ctx>,
    uuid: String,
    list: TrackList,
    stack: Rc<PageStack>,
    header_bar: adw::HeaderBar,
    title: adw::WindowTitle,
    root_buttons: [gtk::Box; 2],
    stack_watch: RefCell<Option<(gio::ListModel, glib::SignalHandlerId)>>,
    page: glib::WeakRef<adw::NavigationPage>,
    header: RefCell<Option<PageHeader>>,
    playlist: RefCell<Option<Playlist>>,
    /// Where the shown tracks sit among the playlist's items.
    slots: RefCell<Slots>,
    /// Items the playlist has, hidden ones too, as TIDAL last said.
    total: Cell<usize>,
    loading: Cell<bool>,
    failed: RefCell<Option<String>>,
    footer: Cell<Footer>,
    editable: Cell<bool>,
    /// The library has listed this playlist (so its absence means it was
    /// deleted).
    in_library: Cell<bool>,
    compact: Cell<bool>,
    /// Play (from a row) or shuffle requested while loading.
    pending_play: Cell<Option<(Option<usize>, bool)>>,
    edits: RefCell<VecDeque<Edit>>,
    editing: Cell<bool>,
    /// Edits succeeded since the last refresh of the details.
    edited: Cell<bool>,
    resync: Cell<bool>,
    /// The ETag of the playlist version the list shows (with the edits
    /// sent so far); edits send it so they apply to the items on screen.
    etag: RefCell<Option<String>>,
    /// Reloads because the playlist changed while loading, in a row.
    reloads: Cell<u32>,
    /// An edit found the playlist changed elsewhere: say so once reloaded.
    changed_elsewhere: Cell<bool>,
    task: RefCell<Option<glib::JoinHandle<()>>>,
    edit_task: RefCell<Option<glib::JoinHandle<()>>>,
    listener: Cell<u64>,
}

pub fn page(ctx: &Rc<Ctx>, uuid: String) -> adw::NavigationPage {
    let list = TrackList::new(ctx, Variant::Cover);
    let stack = PageStack::new(&skeleton::page(240, false, 8, false));
    stack.set_content(list.widget());

    let title = adw::WindowTitle::new("", "");
    let header_bar = adw::HeaderBar::builder()
        .title_widget(&title)
        .show_title(false)
        .build();
    // Opened from the sidebar, the page is Collection's root (wide).
    let start = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    start.append(&sidebar::show_sidebar_button(&ctx.window));
    let end = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    end.append(&sidebar::narrow_main_menu_button(&ctx.window));
    header_bar.pack_start(&start);
    header_bar.pack_end(&end);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header_bar);
    let page = adw::NavigationPage::builder()
        .title(tr("Playlist"))
        .child(&toolbar)
        .build();

    let state = Rc::new(PlaylistPage {
        ctx: Rc::downgrade(ctx),
        uuid,
        list: list.clone(),
        stack: stack.clone(),
        header_bar,
        title,
        root_buttons: [start, end],
        stack_watch: RefCell::default(),
        page: page.downgrade(),
        header: RefCell::default(),
        playlist: RefCell::default(),
        slots: RefCell::default(),
        total: Cell::new(0),
        loading: Cell::new(false),
        failed: RefCell::default(),
        footer: Cell::new(Footer::None),
        editable: Cell::new(false),
        in_library: Cell::new(false),
        compact: Cell::new(false),
        pending_play: Cell::new(None),
        edits: RefCell::default(),
        editing: Cell::new(false),
        edited: Cell::new(false),
        resync: Cell::new(false),
        etag: RefCell::default(),
        reloads: Cell::new(0),
        changed_elsewhere: Cell::new(false),
        task: RefCell::default(),
        edit_task: RefCell::default(),
        listener: Cell::new(0),
    });

    let weak = Rc::downgrade(&state);
    let bin = compact_bin(stack.widget(), move |compact| {
        if let Some(state) = weak.upgrade() {
            state.set_compact(compact);
        }
    });
    toolbar.set_content(Some(&bin));

    let weak = Rc::downgrade(&state);
    list.connect_activated(move |index| {
        if let Some(state) = weak.upgrade() {
            state.play(Some(index), false);
        }
    });

    // The header bar shows the title once the page header scrolls away.
    if let Some(scrolled) = list.scrolled() {
        let weak = Rc::downgrade(&state);
        scrolled
            .vadjustment()
            .connect_value_changed(move |adjustment| {
                if let Some(state) = weak.upgrade() {
                    let height = state
                        .header
                        .borrow()
                        .as_ref()
                        .map_or(300, |h| h.widget().height());
                    state
                        .header_bar
                        .set_show_title(adjustment.value() > f64::from(height));
                }
            });
    }

    for signal in ["showing", "shown"] {
        let weak = Rc::downgrade(&state);
        page.connect_local(signal, false, move |_| {
            if let Some(state) = weak.upgrade() {
                state.update_root_buttons();
            }
            None
        });
    }

    let weak = Rc::downgrade(&state);
    let listener = ctx.library.connect_changed(move |change| {
        if let (Change::Playlists, Some(state)) = (change, weak.upgrade()) {
            state.library_changed();
        }
    });
    state.listener.set(listener);

    state.load();
    let owned = RefCell::new(Some(state));
    page.connect_destroy(move |_| {
        if let Some(state) = owned.take() {
            state.destroy();
        }
    });
    page
}

impl PlaylistPage {
    fn destroy(&self) {
        for task in [self.task.take(), self.edit_task.take()]
            .into_iter()
            .flatten()
        {
            task.abort();
        }
        if let Some(ctx) = self.ctx.upgrade() {
            ctx.library.disconnect(self.listener.get());
        }
        if let Some((stack, handler)) = self.stack_watch.take() {
            stack.disconnect(handler);
        }
    }

    fn source(&self) -> Source {
        Source {
            title: self
                .playlist
                .borrow()
                .as_ref()
                .map(|p| p.title.clone())
                .unwrap_or_default(),
            target: Some(Target::Playlist(self.uuid.clone())),
        }
    }

    /// The sidebar and main menu buttons only belong on a section root.
    /// Re-checked whenever the page shows and whenever its navigation
    /// stack changes (the narrow layout puts the Collection index under it).
    fn update_root_buttons(self: &Rc<Self>) {
        let Some(page) = self.page.upgrade() else {
            return;
        };
        let view = page.parent().and_downcast::<adw::NavigationView>();
        if let Some(view) = &view
            && self.stack_watch.borrow().is_none()
        {
            let stack = view.navigation_stack();
            let weak = Rc::downgrade(self);
            let handler = stack.connect_items_changed(move |_, _, _, _| {
                if let Some(state) = weak.upgrade() {
                    state.update_root_buttons();
                }
            });
            self.stack_watch.replace(Some((stack, handler)));
        }
        let root = view.is_none_or(|view| view.previous_page(&page).is_none());
        for buttons in &self.root_buttons {
            buttons.set_visible(root);
        }
    }

    // ---- Loading ----

    fn load(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if !self.stack.is_showing_content() {
            self.stack.show_loading();
        }
        let own = self.editable.get()
            || ctx
                .library
                .own_playlists()
                .iter()
                .any(|p| p.uuid == self.uuid);
        let fresh_for = if own {
            Duration::ZERO
        } else {
            OTHERS_FRESH_FOR
        };
        // Known again once the loader has checked the version.
        self.etag.replace(None);
        let updates = data::load(&ctx.api, self.uuid.clone(), fresh_for);
        self.follow(updates);
    }

    /// Fetches the items after the loaded ones (after a paging error, or
    /// to pick up tracks just added).
    fn load_rest(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let offset = self.slots.borrow().len();
        let etag = self.etag.borrow().clone();
        let updates = data::load_from(&ctx.api, self.uuid.clone(), offset, etag);
        self.follow(updates);
    }

    fn follow(self: &Rc<Self>, mut updates: tokio::sync::mpsc::UnboundedReceiver<Update>) {
        self.loading.set(true);
        self.failed.replace(None);
        self.update_footer();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            while let Some(update) = updates.recv().await {
                let Some(state) = weak.upgrade() else {
                    return;
                };
                state.apply(update);
            }
        });
        self.task.replace(Some(task));
    }

    fn apply(self: &Rc<Self>, update: Update) {
        match update {
            Update::Details(playlist) => self.show_details(*playlist),
            Update::Items {
                chunk,
                total,
                reset,
            } => {
                if reset {
                    self.slots.borrow_mut().clear();
                    self.list.set_tracks(chunk.tracks);
                } else {
                    self.list.append(chunk.tracks);
                }
                self.slots.borrow_mut().extend(&chunk.slots);
                self.total.set(total);
                if let Some(header) = self.header.borrow().as_ref() {
                    header.set_playable(!self.list.is_empty());
                }
                self.update_footer();
            }
            Update::Version(etag) => {
                self.etag.replace(etag);
            }
            Update::Changed => {
                self.loading.set(false);
                if self.reloads.get() < MAX_RELOADS {
                    self.reloads.set(self.reloads.get() + 1);
                    self.load();
                } else {
                    // Keeps changing: show what there is, edits will find
                    // out and reload.
                    self.etag.replace(None);
                    self.update_footer();
                    self.play_pending();
                    self.loaded();
                }
            }
            Update::Done => {
                log::debug!(
                    "playlist {}: {} tracks of {} items",
                    self.uuid,
                    self.list.len(),
                    self.total.get()
                );
                self.loading.set(false);
                self.reloads.set(0);
                self.update_footer();
                self.play_pending();
                if self.changed_elsewhere.take()
                    && let Some(ctx) = self.ctx.upgrade()
                {
                    ctx.toast(tr("The playlist changed elsewhere; refreshed"));
                }
                self.loaded();
            }
            Update::Failed(err) => {
                self.loading.set(false);
                self.loaded();
                let Some(ctx) = self.ctx.upgrade() else {
                    return;
                };
                if crate::tidal::is_auth_error(&err) {
                    ctx.report(tr("Couldn't load the playlist"), &err);
                }
                if self.playlist.borrow().is_none() {
                    let weak = Rc::downgrade(self);
                    self.stack.show_error(&err, move || {
                        if let Some(state) = weak.upgrade() {
                            state.load();
                        }
                    });
                    return;
                }
                log::warn!("loading playlist {}: {err:#}", self.uuid);
                self.failed.replace(Some(status::describe_error(&err)));
                self.update_footer();
                self.play_pending();
            }
        }
    }

    fn show_details(self: &Rc<Self>, playlist: Playlist) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if ctx.library.is_own_playlist(&playlist) && !self.editable.get() {
            self.enable_editing();
        }
        if ctx.library.playlists().iter().any(|p| p.uuid == self.uuid) {
            self.in_library.set(true);
        }
        let header = PageHeader::playlist(&ctx, &playlist);
        header.set_compact(self.compact.get());
        header.set_playable(!self.list.is_empty());
        let weak = Rc::downgrade(self);
        header.connect_play(move || {
            if let Some(state) = weak.upgrade() {
                state.play(None, false);
            }
        });
        let weak = Rc::downgrade(self);
        header.connect_shuffle(move || {
            if let Some(state) = weak.upgrade() {
                state.play(None, true);
            }
        });
        section::align_to_edge(header.widget(), 0);
        self.list.set_header(Some(header.widget()));
        self.title.set_title(&playlist.title);
        if let Some(page) = self.page.upgrade() {
            page.set_title(&playlist.title);
        }
        self.header.replace(Some(header));
        self.playlist.replace(Some(playlist));
        self.list.set_source(self.source());
        self.stack.show_content();
        self.update_footer();
    }

    fn update_footer(self: &Rc<Self>) {
        let footer = if self.playlist.borrow().is_none() {
            Footer::None
        } else if self.failed.borrow().is_some() {
            Footer::Failed
        } else if self.loading.get() && self.slots.borrow().len() < self.total.get().max(1) {
            Footer::Loading
        } else if self.list.is_empty() && !self.loading.get() {
            Footer::Empty
        } else {
            Footer::None
        };
        if self.footer.replace(footer) == footer && footer != Footer::Failed {
            return;
        }
        let widget = match footer {
            Footer::None => None,
            Footer::Loading => Some(loading_footer()),
            Footer::Failed => Some(self.error_footer()),
            Footer::Empty => Some(self.empty_footer()),
        };
        self.list.set_footer(widget.as_ref());
    }

    fn error_footer(self: &Rc<Self>) -> gtk::Widget {
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(36)
            .halign(gtk::Align::Center)
            .build();
        let message = self.failed.borrow().clone().unwrap_or_default();
        footer.append(
            &gtk::Label::builder()
                .label(
                    tr("Couldn't load the rest of the playlist. {reason}")
                        .replace("{reason}", &message),
                )
                .wrap(true)
                .justify(gtk::Justification::Center)
                .css_classes(["dimmed"])
                .build(),
        );
        let retry = gtk::Button::builder()
            .label(tr("Retry"))
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        let weak = Rc::downgrade(self);
        retry.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.load_rest();
            }
        });
        footer.append(&retry);
        footer.upcast()
    }

    fn empty_footer(&self) -> gtk::Widget {
        let editable = self.editable.get();
        let note = empty_note(
            "view-list-symbolic",
            tr("This Playlist Is Empty"),
            if editable {
                tr("Add tracks from their menu or drag them here.")
            } else {
                tr("There's nothing in it yet.")
            },
        );
        if editable {
            let search = gtk::Button::builder()
                .label(tr("Search"))
                .halign(gtk::Align::Center)
                .margin_top(12)
                .css_classes(["pill"])
                .build();
            let ctx = self.ctx.clone();
            search.connect_clicked(move |_| {
                if let Some(ctx) = ctx.upgrade() {
                    ctx.navigate(Route::Search(None));
                }
            });
            note.append(&search);
        }
        note.upcast()
    }

    /// Follows the library's playlist list: refreshes after edits made
    /// elsewhere (dialogs, other menus) and leaves once the playlist is
    /// deleted.
    fn library_changed(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let Some(mine) = self.playlist.borrow().clone() else {
            return;
        };
        let listed = ctx
            .library
            .playlists()
            .into_iter()
            .find(|p| p.uuid == self.uuid);
        match listed {
            Some(listed) => {
                self.in_library.set(true);
                let busy = self.editing.get() || !self.edits.borrow().is_empty();
                let changed = listed.last_updated != mine.last_updated
                    || listed.number_of_tracks != mine.number_of_tracks
                    || listed.title != mine.title
                    || listed.description != mine.description
                    || listed.public_playlist != mine.public_playlist;
                if self.editable.get() && changed && !busy {
                    self.load();
                }
            }
            None if self.editable.get() && self.in_library.get() => self.leave(),
            None => {}
        }
    }

    /// Closes the page (its playlist is gone): back to the page under it,
    /// or to the playlists view if it was Collection's root.
    fn leave(&self) {
        let (Some(ctx), Some(page)) = (self.ctx.upgrade(), self.page.upgrade()) else {
            return;
        };
        let Some(view) = page.parent().and_downcast::<adw::NavigationView>() else {
            return;
        };
        if let Some(previous) = view.previous_page(&page) {
            view.pop_to_page(&previous);
        } else {
            let route = Route::Collection(CollectionView::Playlists);
            let replacement = super::build(&ctx, &route);
            replacement.set_tag(Some(&route.tag()));
            view.replace(&[replacement]);
        }
    }

    // ---- Playing ----

    fn play(self: &Rc<Self>, start: Option<usize>, shuffle: bool) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if self.loading.get() {
            // Starts when the rest has arrived (see the module docs).
            self.pending_play.set(Some((start, shuffle)));
            return;
        }
        let tracks = self.list.tracks();
        if tracks.is_empty() {
            return;
        }
        if shuffle {
            menus::shuffle_tracks(&ctx, tracks, self.source());
        } else {
            menus::play_tracks(
                &ctx,
                tracks,
                start.unwrap_or(0),
                self.source(),
                start.is_some(),
            );
        }
    }

    fn play_pending(self: &Rc<Self>) {
        if let Some((start, shuffle)) = self.pending_play.take() {
            self.play(start, shuffle);
        }
    }

    fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.list.set_compact(compact);
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_compact(compact);
        }
    }

    // ---- Editing (own playlists) ----

    fn enable_editing(self: &Rc<Self>) {
        self.editable.set(true);
        let weak = Rc::downgrade(self);
        self.list.set_remove_handler(move |index, id| {
            if let Some(state) = weak.upgrade() {
                state.remove(index, id);
            }
        });
        let weak = Rc::downgrade(self);
        self.list.set_reorderable(move |from, to| {
            if let Some(state) = weak.upgrade() {
                state.moved(from, to);
            }
        });
        let Some(view) = self.list.view() else {
            return;
        };

        // Delete removes the focused row.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |controller, key, _, modifiers| {
            let removes = matches!(key, gdk::Key::Delete | gdk::Key::KP_Delete)
                && (modifiers & gtk::accelerator_get_default_mod_mask()).is_empty();
            let row = controller
                .widget()
                .and_then(|view| view.root())
                .and_then(|root| root.focus())
                .and_then(|focus| focused_row(&focus));
            match (removes, row, weak.upgrade()) {
                (true, Some(row), Some(state)) => {
                    state.list.remove_row(&row);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        view.add_controller(keys);

        // Tracks dropped from elsewhere (the queue, other lists) are added.
        let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::COPY);
        target.set_preload(true);
        let weak = Rc::downgrade(self);
        target.connect_motion(move |target, _, _| {
            let accepts = weak.upgrade().is_some_and(|state| {
                target
                    .value()
                    .and_then(|v| v.get::<TrackDrag>().ok())
                    .is_some_and(|drag| state.accepts(&drag))
            });
            if accepts {
                gdk::DragAction::COPY
            } else {
                gdk::DragAction::empty()
            }
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let (Some(state), Ok(drag)) = (weak.upgrade(), value.get::<TrackDrag>()) else {
                return false;
            };
            if !state.accepts(&drag) {
                return false;
            }
            state.enqueue(Edit::Add(drag.track_ids));
            true
        });
        view.add_controller(target);
    }

    /// Whether a drop adds tracks: not a reorder drag from this very list
    /// (those land on rows; a miss must not duplicate the track).
    fn accepts(&self, drag: &TrackDrag) -> bool {
        if drag.track_ids.is_empty() {
            return false;
        }
        drag.origin.is_none_or(|(list, _)| list != self.list.id())
    }

    /// Removes the track at list index `index`, which must be track `id`.
    fn remove(self: &Rc<Self>, index: usize, id: u64) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if self.list.track(index).map(|t| t.id) != Some(id) {
            // The list and what was acted on disagree: never remove
            // another track.
            log::warn!(
                "playlist {}: track {id} is not at {index}; reloading",
                self.uuid
            );
            if self.editing.get() {
                self.edited.set(true);
                self.resync.set(true);
            } else {
                self.load();
            }
            return;
        }
        let Some(item) = self.slots.borrow_mut().remove(index) else {
            return;
        };
        let Some(track) = self.list.remove(index) else {
            self.slots.borrow_mut().insert_at(item);
            return;
        };
        self.total.set(self.total.get().saturating_sub(1));
        self.update_footer();
        self.enqueue(Edit::Remove {
            index,
            item,
            track: track.clone(),
        });
        let title = self
            .playlist
            .borrow()
            .as_ref()
            .map(|p| p.title.clone())
            .unwrap_or_default();
        let weak = Rc::downgrade(self);
        ctx.toast_with_undo(
            &tr("Removed from “{title}”").replace("{title}", &glib::markup_escape_text(&title)),
            move || {
                if let Some(state) = weak.upgrade() {
                    state.restore(index, track.clone());
                }
            },
        );
    }

    /// Undo of [`Self::remove`]: back at the same place.
    fn restore(self: &Rc<Self>, index: usize, track: TrackInfo) {
        let index = index.min(self.list.len());
        let item = self.slots.borrow_mut().insert(index);
        self.list.insert(index, track.clone());
        self.total.set(self.total.get() + 1);
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_playable(true);
        }
        self.update_footer();
        self.enqueue(Edit::Restore {
            index,
            item,
            appended_at: self.total.get() - 1,
            track,
        });
    }

    /// The list moved a row from `from` to `to`.
    fn moved(self: &Rc<Self>, from: usize, to: usize) {
        let Some((item_from, item_to)) = self.slots.borrow_mut().move_track(from, to) else {
            self.list.move_track(to, from);
            return;
        };
        self.enqueue(Edit::Move {
            from,
            to,
            item_from,
            item_to,
        });
    }

    fn enqueue(self: &Rc<Self>, edit: Edit) {
        self.edits.borrow_mut().push_back(edit);
        if !self.editing.get() {
            self.next_edit();
        }
    }

    /// Loading has stopped: edits made meanwhile can be sent (they waited,
    /// as their indices are into the version being loaded).
    fn loaded(self: &Rc<Self>) {
        if !self.editing.get() && !self.edits.borrow().is_empty() {
            self.next_edit();
        }
    }

    fn next_edit(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if self.loading.get() {
            self.editing.set(false);
            return;
        }
        let Some(edit) = self.edits.borrow_mut().pop_front() else {
            self.editing.set(false);
            self.edits_done();
            return;
        };
        self.editing.set(true);
        let api = ctx.api.clone();
        let uuid = self.uuid.clone();
        let etag = self.etag.borrow().clone();
        let last_updated = self
            .playlist
            .borrow()
            .as_ref()
            .and_then(|p| p.last_updated.clone());
        let request = edit.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let result = runtime::spawn(async move {
                let etag = match etag {
                    Some(etag) => etag,
                    // Shown from the cache without asking TIDAL.
                    None => data::etag_if_unchanged(&api, &uuid, last_updated.as_deref()).await?,
                };
                match request {
                    Edit::Remove { item, .. } => data::remove(&api, &uuid, item, &etag).await,
                    Edit::Restore {
                        item,
                        appended_at,
                        track,
                        ..
                    } => data::restore(&api, &uuid, &etag, track.id, appended_at, item).await,
                    Edit::Move {
                        item_from, item_to, ..
                    } => data::move_item(&api, &uuid, item_from, item_to, &etag).await,
                    Edit::Add(ids) => data::add(&api, &uuid, &ids, &etag).await,
                }
            })
            .await;
            if let Some(state) = weak.upgrade() {
                state.edit_finished(edit, result);
            }
        });
        self.edit_task.replace(Some(task));
    }

    /// `result` is the playlist's next ETag.
    fn edit_finished(self: &Rc<Self>, edit: Edit, result: anyhow::Result<Option<String>>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        match result {
            Ok(etag) => {
                self.etag.replace(etag);
                self.edited.set(true);
                if let Edit::Add(ids) = &edit {
                    let title = self
                        .playlist
                        .borrow()
                        .as_ref()
                        .map(|p| p.title.clone())
                        .unwrap_or_default();
                    ctx.toast(&glib::markup_escape_text(&if ids.len() == 1 {
                        tr("Added to “{title}”").replace("{title}", &title)
                    } else {
                        trn(
                            "Added {n} track to “{title}”",
                            "Added {n} tracks to “{title}”",
                            ids.len() as u64,
                        )
                        .replace("{n}", &ids.len().to_string())
                        .replace("{title}", &title)
                    }));
                    // New tracks go at the end: fetch them (or everything,
                    // if the list is still loading).
                    if self.loading.get() {
                        self.resync.set(true);
                    } else {
                        self.load_rest();
                    }
                }
            }
            Err(err) if data::is_changed(&err) => {
                // Nothing was edited: what is on screen is not what TIDAL
                // has. Start over from TIDAL and say why.
                log::info!("playlist {} changed elsewhere", self.uuid);
                self.edits.borrow_mut().clear();
                self.edited.set(true);
                self.resync.set(true);
                self.changed_elsewhere.set(true);
            }
            Err(err) => {
                let message = match edit {
                    Edit::Remove { .. } => tr("Couldn't remove the track"),
                    Edit::Restore { .. } => tr("Couldn't put the track back"),
                    Edit::Move { .. } => tr("Couldn't move the track"),
                    Edit::Add(_) => tr("Couldn't add to the playlist"),
                };
                ctx.report(message, &err);
                if self.edits.borrow().is_empty() {
                    self.revert(edit);
                } else {
                    // Later edits assumed this one: start over from TIDAL.
                    self.edits.borrow_mut().clear();
                    self.edited.set(true);
                    self.resync.set(true);
                }
            }
        }
        self.next_edit();
    }

    /// Rolls back an edit that TIDAL refused.
    fn revert(self: &Rc<Self>, edit: Edit) {
        match edit {
            Edit::Remove { index, item, track } => {
                self.slots.borrow_mut().insert_at(item);
                self.list.insert(index.min(self.list.len()), track);
                self.total.set(self.total.get() + 1);
            }
            Edit::Restore { index, item, .. } => {
                self.slots.borrow_mut().remove_at(item);
                self.list.remove(index);
                self.total.set(self.total.get().saturating_sub(1));
            }
            Edit::Move { from, to, .. } => {
                self.slots.borrow_mut().move_track(to, from);
                self.list.move_track(to, from);
            }
            Edit::Add(_) => {}
        }
        if let Some(header) = self.header.borrow().as_ref() {
            header.set_playable(!self.list.is_empty());
        }
        self.update_footer();
    }

    /// After a run of edits: forget the cached copy, refresh the details
    /// (counts, duration) and the library (sidebar), or reload everything
    /// if the playlist got out of step.
    fn edits_done(self: &Rc<Self>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        if !self.edited.replace(false) {
            return;
        }
        let resync = self.resync.replace(false);
        let api = ctx.api.clone();
        let uuid = self.uuid.clone();
        let weak = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let result = runtime::spawn(async move {
                data::forget(&api, &uuid).await;
                data::details(&api, &uuid).await
            })
            .await;
            let Some(state) = weak.upgrade() else {
                return;
            };
            if resync {
                state.load();
            } else if let Ok(playlist) = result {
                state.show_details(playlist);
            }
            if let Some(ctx) = state.ctx.upgrade() {
                ctx.library.refresh_playlists();
            }
        });
        self.edit_task.replace(Some(task));
    }
}

/// A spinner while more items are on their way.
fn loading_footer() -> gtk::Widget {
    adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .margin_top(24)
        .margin_bottom(36)
        .build()
        .upcast()
}

/// An empty state that fits in a list item (an `AdwStatusPage` scrolls
/// on its own, so it collapses there).
pub(super) fn empty_note(icon: &str, title: &str, description: &str) -> gtk::Box {
    let note = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(36)
        .margin_bottom(48)
        .margin_start(12)
        .margin_end(12)
        .halign(gtk::Align::Center)
        .build();
    note.append(
        &gtk::Image::builder()
            .icon_name(icon)
            .pixel_size(64)
            .margin_bottom(12)
            .css_classes(["dimmed"])
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build(),
    );
    note.append(
        &gtk::Label::builder()
            .label(title)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["title-2"])
            .accessible_role(gtk::AccessibleRole::Heading)
            .build(),
    );
    note.append(
        &gtk::Label::builder()
            .label(description)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build(),
    );
    note
}

/// The track row that has the focus (it, or something inside it).
fn focused_row(focus: &gtk::Widget) -> Option<TrackRow> {
    if let Some(row) = focus.downcast_ref::<TrackRow>() {
        return Some(row.clone());
    }
    if let Some(row) = focus.ancestor(TrackRow::static_type()) {
        return row.downcast().ok();
    }
    focus.first_child().and_downcast::<TrackRow>()
}
