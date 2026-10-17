//! A virtualised list of tracks (docs/design.md §4.1, §4.11, §4.12, §6).
//!
//! A `gtk::ListView` whose model holds the tracks plus, optionally, a page
//! header as the first item, "Disc N" separators and a footer as the last
//! item: album and playlist pages scroll as one surface and stay
//! virtualised however long the list is.
//!
//! - Activation (double click, Enter; single click when compact) plays the
//!   list from that track ([`TrackList::set_source`] names it), unless
//!   [`TrackList::connect_activated`] replaces that.
//! - One shared menu ([`ItemMenu`]) opens from the row's "⋯" button, a right
//!   click, a long press, the Menu key or Shift+F10.
//! - Rows are drag sources ([`TrackDrag`], plus TIDAL links for other apps).
//!   With [`TrackList::set_reorderable`] they can be dropped elsewhere in the
//!   list and moved with Alt+↑/↓ or the menu.
//! - The list subscribes once to playback (the playing row) and to the
//!   library (hearts) and updates only the rows that are bound.
//!
//! The view owns the list's state: a [`TrackList`] is a handle that can be
//! dropped freely once the widget is in the page.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::track_row::{RowShared, TrackEntry, TrackRow};
pub use super::track_row::{TRACK_URI_MIME, Variant};
use crate::library::{Change, Item};
use crate::model::TrackInfo;
use crate::playback::{Event, PlayState, Source};
use crate::ui::context::Ctx;
use crate::ui::menus::{self, ItemMenu, Subject};

/// One item of the model.
enum Entry {
    Header,
    Footer,
    /// "Disc N" separator.
    Disc(u32),
    Track(Rc<TrackEntry>),
}

type IndexHook = Rc<dyn Fn(usize)>;
type MoveHook = Rc<dyn Fn(usize, usize)>;

struct Inner {
    ctx: Weak<Ctx>,
    shared: Rc<RowShared>,
    store: gio::ListStore,
    view: glib::WeakRef<gtk::ListView>,
    scrolled: glib::WeakRef<gtk::ScrolledWindow>,
    menu: RefCell<Option<Rc<ItemMenu>>>,
    header: RefCell<Option<gtk::Widget>>,
    footer: RefCell<Option<gtk::Widget>>,
    /// The list item currently showing the header/footer widget, so it can
    /// be taken back before another item shows it.
    holders: RefCell<[glib::WeakRef<gtk::ListItem>; 2]>,
    group_discs: Cell<bool>,
    /// Model positions of the disc separators, ascending.
    discs: RefCell<Vec<u32>>,
    bound: RefCell<Vec<TrackRow>>,
    source: RefCell<Source>,
    on_activate: RefCell<Option<IndexHook>>,
    on_remove: RefCell<Option<IndexHook>>,
    on_move: RefCell<Option<MoveHook>>,
    on_near_end: RefCell<Option<Rc<dyn Fn()>>>,
    subscriptions: Cell<(u64, u64)>,
}

/// A handle on a track list; cheap to clone.
#[derive(Clone)]
pub struct TrackList {
    inner: Rc<Inner>,
    widget: gtk::Widget,
}

/// A weak handle, for closures stored inside the list's own widgets.
#[derive(Clone)]
pub struct WeakTrackList {
    inner: Weak<Inner>,
    widget: glib::WeakRef<gtk::Widget>,
}

impl WeakTrackList {
    pub fn upgrade(&self) -> Option<TrackList> {
        Some(TrackList {
            inner: self.inner.upgrade()?,
            widget: self.widget.upgrade()?,
        })
    }
}

impl TrackList {
    /// A list in its own scrolled window (limited to 1200 px wide), for
    /// pages that are mostly this list.
    pub fn new(ctx: &Rc<Ctx>, variant: Variant) -> Self {
        Self::build(ctx, variant, true)
    }

    /// A list without scrolling, for a few tracks inside a scrolled page
    /// (an artist's top tracks, a Home module). All its rows are built.
    pub fn embedded(ctx: &Rc<Ctx>, variant: Variant) -> Self {
        Self::build(ctx, variant, false)
    }

    fn build(ctx: &Rc<Ctx>, variant: Variant, scrolled: bool) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .tab_behavior(gtk::ListTabBehavior::Item)
            .build();
        view.update_property(&[gtk::accessible::Property::Label("Tracks")]);

        let widget: gtk::Widget = if scrolled {
            view.add_css_class("navigation-sidebar");
            let clamp = adw::ClampScrollable::builder()
                .maximum_size(1200)
                .tightening_threshold(1200)
                .child(&view)
                .build();
            gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&clamp)
                .build()
                .upcast()
        } else {
            view.add_css_class("card");
            view.set_overflow(gtk::Overflow::Hidden);
            view.clone().upcast()
        };

        let current = ctx.playback.current().map(|t| t.id);
        let shared = RowShared::new(ctx, variant);
        shared.playing.set(current);
        shared.paused.set(ctx.playback.state() == PlayState::Paused);

        let inner = Rc::new(Inner {
            ctx: Rc::downgrade(ctx),
            shared,
            store,
            view: view.downgrade(),
            scrolled: widget
                .downcast_ref::<gtk::ScrolledWindow>()
                .map(|s| s.downgrade())
                .unwrap_or_default(),
            menu: RefCell::default(),
            header: RefCell::default(),
            footer: RefCell::default(),
            holders: RefCell::default(),
            group_discs: Cell::new(false),
            discs: RefCell::default(),
            bound: RefCell::default(),
            source: RefCell::new(Source {
                title: String::new(),
                target: None,
            }),
            on_activate: RefCell::default(),
            on_remove: RefCell::default(),
            on_move: RefCell::default(),
            on_near_end: RefCell::default(),
            subscriptions: Cell::new((0, 0)),
        });

        let weak = Rc::downgrade(&inner);
        factory.connect_bind(move |_, item| {
            if let Some(inner) = weak.upgrade() {
                inner.bind(list_item(item));
            }
        });
        let weak = Rc::downgrade(&inner);
        factory.connect_unbind(move |_, item| {
            if let Some(inner) = weak.upgrade() {
                inner.unbind(list_item(item));
            }
        });
        let weak = Rc::downgrade(&inner);
        view.connect_activate(move |_, position| {
            if let Some(inner) = weak.upgrade() {
                inner.activate(position);
            }
        });

        let menu = ItemMenu::new(ctx, &view);
        inner.menu.replace(Some(menu));
        let weak = Rc::downgrade(&inner);
        inner
            .shared
            .on_end_button
            .replace(Some(Box::new(move |row, button| {
                if let Some(inner) = weak.upgrade() {
                    inner.end_button(row, button);
                }
            })));
        inner.add_gestures(&view);
        inner.subscribe(ctx);
        if let Some(scrolled) = widget.downcast_ref::<gtk::ScrolledWindow>() {
            inner.watch_scrolling(ctx, scrolled);
        }

        // The view owns the state; the subscriptions end with it.
        let owned = RefCell::new(Some(inner.clone()));
        view.connect_destroy(move |_| {
            if let Some(inner) = owned.take() {
                inner.unsubscribe();
            }
        });
        Self { inner, widget }
    }

    /// The widget to put in the page (the scrolled window, or the view).
    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
    }

    pub fn view(&self) -> Option<gtk::ListView> {
        self.inner.view.upgrade()
    }

    /// The scrolled window of a list made with [`TrackList::new`].
    pub fn scrolled(&self) -> Option<gtk::ScrolledWindow> {
        self.inner.scrolled.upgrade()
    }

    pub fn downgrade(&self) -> WeakTrackList {
        WeakTrackList {
            inner: Rc::downgrade(&self.inner),
            widget: self.widget.downgrade(),
        }
    }

    /// The shared menu, e.g. to open it from a header for one of the rows.
    pub fn menu(&self) -> Option<Rc<ItemMenu>> {
        self.inner.menu.borrow().clone()
    }

    /// What "Playing from …" says when a row is activated.
    pub fn set_source(&self, source: Source) {
        self.inner.source.replace(source);
    }

    /// Numbered rows hide their artists when they equal `names` (the
    /// album's artists, as [`TrackInfo::artist_names`] formats them).
    pub fn set_album_artists(&self, names: &str) {
        self.inner.shared.album_artists.replace(names.to_owned());
        self.inner.relayout_rows();
    }

    /// Inserts "Disc N" separators when the tracks span several volumes.
    /// Takes effect on the next [`TrackList::set_tracks`].
    pub fn set_group_discs(&self, group: bool) {
        self.inner.group_discs.set(group);
    }

    /// Compact rows (no album column, heart or duration; single click
    /// plays). Driven by one `AdwBreakpointBin` per page: see
    /// [`compact_bin`].
    pub fn set_compact(&self, compact: bool) {
        self.inner.shared.compact.set(compact);
        if let Some(view) = self.view() {
            view.set_single_click_activate(compact);
        }
        self.inner.relayout_rows();
    }

    /// Follows the compactness of the nearest `AdwBreakpointBin` around the
    /// list (the page's [`compact_bin`]) instead of having its own: for
    /// embedded lists inside a page.
    pub fn follow_page_compact(&self) {
        let Some(view) = self.view() else {
            return;
        };
        let weak = self.downgrade();
        let connected = Rc::new(Cell::new(false));
        view.connect_map(move |view| {
            if connected.replace(true) {
                return;
            }
            let Some(bin) = view
                .ancestor(adw::BreakpointBin::static_type())
                .and_downcast::<adw::BreakpointBin>()
            else {
                return;
            };
            let apply = {
                let weak = weak.clone();
                move |bin: &adw::BreakpointBin| {
                    if let Some(list) = weak.upgrade() {
                        list.set_compact(bin.current_breakpoint().is_some());
                    }
                }
            };
            apply(&bin);
            bin.connect_current_breakpoint_notify(apply);
        });
    }

    /// A widget shown as the list's first item (scrolls with the rows).
    pub fn set_header(&self, header: Option<&impl IsA<gtk::Widget>>) {
        self.inner.set_edge(0, header.map(|h| h.as_ref().clone()));
    }

    /// A widget shown as the list's last item.
    pub fn set_footer(&self, footer: Option<&impl IsA<gtk::Widget>>) {
        self.inner.set_edge(1, footer.map(|f| f.as_ref().clone()));
    }

    /// Replaces all the tracks.
    pub fn set_tracks(&self, tracks: Vec<TrackInfo>) {
        let inner = &self.inner;
        let start = u32::from(inner.header.borrow().is_some());
        let end = inner.store.n_items() - u32::from(inner.footer.borrow().is_some());
        let group = inner.group_discs.get() && {
            let first = tracks.first().map(|t| t.volume_number);
            tracks.iter().any(|t| Some(t.volume_number) != first)
        };
        let mut objects = Vec::with_capacity(tracks.len() + 4);
        let mut volume = None;
        for track in tracks {
            if group && volume != Some(track.volume_number) {
                volume = Some(track.volume_number);
                objects.push(glib::BoxedAnyObject::new(Entry::Disc(track.volume_number)));
            }
            objects.push(glib::BoxedAnyObject::new(Entry::Track(Rc::new(
                TrackEntry::new(track),
            ))));
        }
        inner.store.splice(start, end - start, &objects);
        inner.index_discs();
    }

    /// Same as [`TrackList::set_tracks`].
    pub fn replace(&self, tracks: Vec<TrackInfo>) {
        self.set_tracks(tracks);
    }

    /// Adds tracks at the end (pagination).
    pub fn append(&self, tracks: Vec<TrackInfo>) {
        let inner = &self.inner;
        let at = inner.store.n_items() - u32::from(inner.footer.borrow().is_some());
        let mut volume = inner.tracks().last().map(|t| t.volume_number);
        let group = inner.group_discs.get() && !inner.discs.borrow().is_empty();
        let mut objects = Vec::with_capacity(tracks.len());
        for track in tracks {
            if group && volume != Some(track.volume_number) {
                objects.push(glib::BoxedAnyObject::new(Entry::Disc(track.volume_number)));
            }
            volume = Some(track.volume_number);
            objects.push(glib::BoxedAnyObject::new(Entry::Track(Rc::new(
                TrackEntry::new(track),
            ))));
        }
        inner.store.splice(at, 0, &objects);
        inner.index_discs();
    }

    /// Inserts a track at `index` (e.g. to undo a removal).
    pub fn insert(&self, index: usize, track: TrackInfo) {
        let position = self.inner.position_of(index);
        self.inner.store.insert(
            position,
            &glib::BoxedAnyObject::new(Entry::Track(Rc::new(TrackEntry::new(track)))),
        );
        self.inner.index_discs();
    }

    /// Removes and returns the track at `index`.
    pub fn remove(&self, index: usize) -> Option<TrackInfo> {
        let track = self.track(index)?;
        self.inner.store.remove(self.inner.position_of(index));
        self.inner.index_discs();
        Some(track)
    }

    /// Moves the track at `from` so it ends up at `to` (no callbacks).
    pub fn move_track(&self, from: usize, to: usize) {
        if from == to || to >= self.len() {
            return;
        }
        if let Some(track) = self.remove(from) {
            self.insert(to, track);
        }
    }

    pub fn len(&self) -> usize {
        self.inner.store.n_items() as usize
            - usize::from(self.inner.header.borrow().is_some())
            - usize::from(self.inner.footer.borrow().is_some())
            - self.inner.discs.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn track(&self, index: usize) -> Option<TrackInfo> {
        match &*entry_at(&self.inner.store, self.inner.position_of(index))?.borrow::<Entry>() {
            Entry::Track(entry) => Some(entry.info.clone()),
            _ => None,
        }
    }

    /// All the tracks, in order.
    pub fn tracks(&self) -> Vec<TrackInfo> {
        self.inner.tracks()
    }

    /// Replaces what activating a row does; gets the track index.
    pub fn connect_activated(&self, f: impl Fn(usize) + 'static) {
        self.inner.on_activate.replace(Some(Rc::new(f)));
    }

    /// Shows "Remove from This Playlist" in the menu (and handles the ✕ of
    /// queue rows). Gets the track index; the list does not remove it
    /// itself.
    pub fn set_remove_handler(&self, f: impl Fn(usize) + 'static) {
        let f: IndexHook = Rc::new(f);
        self.inner.on_remove.replace(Some(f.clone()));
        if self.inner.shared.variant != Variant::Queue
            && let Some(menu) = self.menu()
        {
            menu.set_remove_handler(move |index| f(index));
        }
    }

    /// Enables reordering: drag handles, drops inside the list, Alt+↑/↓ and
    /// "Move Up/Down" in the menu. The list moves the row itself, then calls
    /// `on_move(from, to)` to persist it (on failure, move it back with
    /// [`TrackList::move_track`]).
    pub fn set_reorderable(&self, on_move: impl Fn(usize, usize) + 'static) {
        let inner = &self.inner;
        inner.shared.reorderable.set(true);
        inner.on_move.replace(Some(Rc::new(on_move)));
        let weak = self.downgrade();
        let do_move = move |from: usize, to: usize| {
            if let Some(list) = weak.upgrade() {
                list.move_and_notify(from, to);
            }
        };
        inner
            .shared
            .on_move
            .replace(Some(Box::new(do_move.clone())));
        if let Some(menu) = self.menu() {
            let weak = self.downgrade();
            menu.set_move_handler(do_move, move || weak.upgrade().map_or(0, |list| list.len()));
        }
        inner.relayout_rows();
    }

    fn move_and_notify(&self, from: usize, to: usize) {
        if from == to || to >= self.len() {
            return;
        }
        self.move_track(from, to);
        if let Some(view) = self.view() {
            view.scroll_to(
                self.inner.position_of(to),
                gtk::ListScrollFlags::FOCUS,
                None,
            );
        }
        let hook = self.inner.on_move.borrow().clone();
        if let Some(hook) = hook {
            hook(from, to);
        }
    }

    /// Called (repeatedly) while the end of a scrolled list is less than
    /// two screens away; for loading the next page.
    pub fn connect_near_end(&self, f: impl Fn() + 'static) {
        self.inner.on_near_end.replace(Some(Rc::new(f)));
    }

    pub fn scroll_to_top(&self) {
        if let Some(scrolled) = self.scrolled() {
            scrolled.vadjustment().set_value(0.0);
        }
    }
}

impl Inner {
    fn ctx(&self) -> Option<Rc<Ctx>> {
        self.ctx.upgrade()
    }

    fn tracks(&self) -> Vec<TrackInfo> {
        (0..self.store.n_items())
            .filter_map(|i| match &*entry_at(&self.store, i)?.borrow::<Entry>() {
                Entry::Track(entry) => Some(entry.info.clone()),
                _ => None,
            })
            .collect()
    }

    fn index_discs(&self) {
        let discs = (0..self.store.n_items())
            .filter(|&i| {
                entry_at(&self.store, i)
                    .is_some_and(|e| matches!(&*e.borrow::<Entry>(), Entry::Disc(_)))
            })
            .collect();
        self.discs.replace(discs);
    }

    fn header_offset(&self) -> u32 {
        u32::from(self.header.borrow().is_some())
    }

    /// Track index of the model position (whatever is there).
    fn index_of(&self, position: u32) -> usize {
        let discs = self.discs.borrow();
        let before = discs.partition_point(|&d| d < position) as u32;
        position.saturating_sub(self.header_offset() + before) as usize
    }

    fn position_of(&self, index: usize) -> u32 {
        let mut position = index as u32 + self.header_offset();
        for &disc in self.discs.borrow().iter() {
            if disc <= position {
                position += 1;
            }
        }
        position
    }

    fn set_edge(&self, edge: usize, widget: Option<gtk::Widget>) {
        let slot = if edge == 0 {
            &self.header
        } else {
            &self.footer
        };
        let had = slot.borrow().is_some();
        if let Some(holder) = self.holders.borrow()[edge].upgrade()
            && holder.child().as_ref() == slot.borrow().as_ref()
        {
            holder.set_child(None::<&gtk::Widget>);
        }
        let has = widget.is_some();
        slot.replace(widget);
        let position = if edge == 0 {
            0
        } else {
            self.store.n_items() - u32::from(had)
        };
        let object = glib::BoxedAnyObject::new(if edge == 0 {
            Entry::Header
        } else {
            Entry::Footer
        });
        match (had, has) {
            (false, true) => self.store.insert(position, &object),
            (true, false) => self.store.remove(position),
            // Re-insert so the item is bound again with the new widget.
            (true, true) => self.store.splice(position, 1, &[object]),
            (false, false) => {}
        }
        self.index_discs();
    }

    fn bind(&self, item: &gtk::ListItem) {
        let Some(object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let entry = object.borrow::<Entry>();
        match &*entry {
            Entry::Header | Entry::Footer => {
                let edge = usize::from(matches!(&*entry, Entry::Footer));
                let slot = if edge == 0 {
                    &self.header
                } else {
                    &self.footer
                };
                let Some(widget) = slot.borrow().clone() else {
                    return;
                };
                if let Some(holder) = self.holders.borrow()[edge].upgrade()
                    && holder != *item
                    && holder.child().as_ref() == Some(&widget)
                {
                    holder.set_child(None::<&gtk::Widget>);
                }
                item.set_child(Some(&widget));
                self.holders.borrow_mut()[edge] = item.downgrade();
                item.set_activatable(false);
                item.set_selectable(false);
                item.set_focusable(true);
                item.set_accessible_label("");
            }
            Entry::Disc(volume) => {
                let label = match item.child().and_downcast::<gtk::Label>() {
                    Some(label) => label,
                    None => {
                        let label = gtk::Label::builder()
                            .xalign(0.0)
                            .height_request(36)
                            .margin_start(12)
                            .margin_top(12)
                            .css_classes(["heading", "dimmed"])
                            .build();
                        item.set_child(Some(&label));
                        label
                    }
                };
                label.set_label(&format!("Disc {volume}"));
                item.set_activatable(false);
                item.set_selectable(false);
                item.set_focusable(false);
                item.set_accessible_label(&format!("Disc {volume}"));
            }
            Entry::Track(track) => {
                let row = match item.child().and_downcast::<TrackRow>() {
                    Some(row) => row,
                    None => {
                        let Some(ctx) = self.ctx() else {
                            return;
                        };
                        let row = TrackRow::new(&ctx, &self.shared);
                        item.set_child(Some(&row));
                        row
                    }
                };
                item.set_activatable(true);
                item.set_selectable(false);
                item.set_focusable(true);
                let position = item.position();
                row.bind(track, self.index_of(position), position, item);
                self.bound.borrow_mut().push(row);
            }
        }
    }

    fn unbind(&self, item: &gtk::ListItem) {
        if let Some(row) = item.child().and_downcast::<TrackRow>() {
            row.unbind();
            self.bound.borrow_mut().retain(|other| other != &row);
        }
    }

    fn relayout_rows(&self) {
        for row in self.bound.borrow().iter() {
            row.apply_layout();
        }
    }

    fn activate(&self, position: u32) {
        let is_track = entry_at(&self.store, position)
            .is_some_and(|e| matches!(&*e.borrow::<Entry>(), Entry::Track(_)));
        if !is_track {
            return;
        }
        let index = self.index_of(position);
        let hook = self.on_activate.borrow().clone();
        if let Some(hook) = hook {
            return hook(index);
        }
        if let Some(ctx) = self.ctx() {
            menus::play_tracks(
                &ctx,
                self.tracks(),
                index,
                self.source.borrow().clone(),
                true,
            );
        }
    }

    fn subject(row: &TrackRow) -> Option<Subject> {
        Some(Subject::Track {
            track: row.entry()?.info.clone(),
            index: Some(row.index()),
        })
    }

    fn end_button(&self, row: &TrackRow, button: &gtk::Widget) {
        if self.shared.variant == Variant::Queue {
            let hook = self.on_remove.borrow().clone();
            if let Some(hook) = hook {
                hook(row.index());
            }
            return;
        }
        if let (Some(menu), Some(subject)) = (self.menu.borrow().clone(), Self::subject(row)) {
            menu.popup_at(subject, button);
        }
    }

    fn row_at(view: &gtk::ListView, x: f64, y: f64) -> Option<TrackRow> {
        let picked = view.pick(x, y, gtk::PickFlags::DEFAULT)?;
        row_of(&picked)
    }

    fn add_gestures(self: &Rc<Self>, view: &gtk::ListView) {
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_SECONDARY)
            .build();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let Some(view) = inner.view.upgrade() else {
                return;
            };
            if let Some(subject) = Self::row_at(&view, x, y).as_ref().and_then(Self::subject)
                && let Some(menu) = inner.menu.borrow().clone()
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                menu.popup_at_point(subject, x, y);
            }
        });
        view.add_controller(click);

        let press = gtk::GestureLongPress::builder().touch_only(true).build();
        let weak = Rc::downgrade(self);
        press.connect_pressed(move |gesture, x, y| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let Some(view) = inner.view.upgrade() else {
                return;
            };
            if let Some(subject) = Self::row_at(&view, x, y).as_ref().and_then(Self::subject)
                && let Some(menu) = inner.menu.borrow().clone()
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                menu.popup_at_point(subject, x, y);
            }
        });
        view.add_controller(press);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(inner) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            inner.key_pressed(key, modifiers)
        });
        view.add_controller(keys);
    }

    fn key_pressed(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let Some(view) = self.view.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
        let menu_key = key == gdk::Key::Menu
            || (key == gdk::Key::F10 && modifiers == gdk::ModifierType::SHIFT_MASK);
        let move_by = match key {
            gdk::Key::Up | gdk::Key::KP_Up if modifiers == gdk::ModifierType::ALT_MASK => -1,
            gdk::Key::Down | gdk::Key::KP_Down if modifiers == gdk::ModifierType::ALT_MASK => 1,
            _ => 0,
        };
        if !menu_key && move_by == 0 {
            return glib::Propagation::Proceed;
        }
        let Some(row) = view
            .root()
            .and_then(|root| root.focus())
            .and_then(|focus| row_of(&focus))
        else {
            return glib::Propagation::Proceed;
        };
        if menu_key {
            if let (Some(menu), Some(subject)) = (self.menu.borrow().clone(), Self::subject(&row)) {
                menu.popup_at(subject, &row);
            }
            return glib::Propagation::Stop;
        }
        if !self.shared.reorderable.get() {
            return glib::Propagation::Proceed;
        }
        let from = row.index();
        let Some(to) = from.checked_add_signed(move_by) else {
            return glib::Propagation::Stop;
        };
        if let Some(on_move) = self.shared.on_move.borrow().as_ref() {
            on_move(from, to);
        }
        glib::Propagation::Stop
    }

    fn subscribe(self: &Rc<Self>, ctx: &Rc<Ctx>) {
        let weak = Rc::downgrade(self);
        let playback = ctx.playback.subscribe(move |event| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            match event {
                Event::Track(track) => inner.shared.playing.set(track.as_ref().map(|t| t.id)),
                Event::State(state) => inner.shared.paused.set(*state == PlayState::Paused),
                _ => return,
            }
            for row in inner.bound.borrow().iter() {
                row.update_playing();
            }
        });
        let weak = Rc::downgrade(self);
        let library = ctx.library.connect_changed(move |change| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let id = match change {
                Change::Favorite(Item::Track(id), _) => Some(*id),
                Change::Loaded => None,
                _ => return,
            };
            for row in inner.bound.borrow().iter() {
                if (id.is_none() || row.track_id() == id)
                    && let Some(heart) = row.heart()
                {
                    heart.sync();
                }
            }
        });
        self.subscriptions.set((playback, library));
    }

    fn unsubscribe(&self) {
        let (playback, library) = self.subscriptions.get();
        if let Some(ctx) = self.ctx() {
            ctx.playback.unsubscribe(playback);
            ctx.library.disconnect(library);
        }
    }

    /// Tells the cover pool which rows are on screen (rows have one height,
    /// so the scroll fraction maps to a position) and asks for more near
    /// the end.
    fn watch_scrolling(self: &Rc<Self>, ctx: &Rc<Ctx>, scrolled: &gtk::ScrolledWindow) {
        let covers = Rc::downgrade(&ctx.covers);
        let weak = Rc::downgrade(self);
        let update = move |adjustment: &gtk::Adjustment| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let upper = adjustment.upper();
            if upper <= 0.0 {
                return;
            }
            if let Some(covers) = covers.upgrade() {
                let middle = (adjustment.value() + adjustment.page_size() / 2.0) / upper;
                covers.set_focus((middle * f64::from(inner.store.n_items())) as u32);
            }
            let remaining = upper - adjustment.value() - adjustment.page_size();
            if remaining < adjustment.page_size() * 2.0 {
                let hook = inner.on_near_end.borrow().clone();
                if let Some(hook) = hook {
                    hook();
                }
            }
        };
        let adjustment = scrolled.vadjustment();
        adjustment.connect_value_changed(update.clone());
        adjustment.connect_changed(update);
    }
}

/// The track row containing `widget` (a row, something inside one, or
/// the list item widget around one).
fn row_of(widget: &gtk::Widget) -> Option<TrackRow> {
    if let Some(row) = widget.downcast_ref::<TrackRow>() {
        return Some(row.clone());
    }
    if let Some(row) = widget.ancestor(TrackRow::static_type()) {
        return row.downcast().ok();
    }
    widget.first_child().and_downcast::<TrackRow>()
}

fn entry_at(store: &gio::ListStore, position: u32) -> Option<glib::BoxedAnyObject> {
    store.item(position).and_downcast()
}

fn list_item(item: &glib::Object) -> &gtk::ListItem {
    item.downcast_ref()
        .expect("the factory only receives list items")
}

/// Wraps a page's content in an `AdwBreakpointBin` that reports whether it
/// is narrower than 640 px (compact track rows, stacked headers).
pub fn compact_bin(
    child: &impl IsA<gtk::Widget>,
    on_change: impl Fn(bool) + 'static,
) -> adw::BreakpointBin {
    let bin = adw::BreakpointBin::builder()
        .width_request(360)
        .height_request(200)
        .child(child)
        .build();
    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 640px").expect("valid breakpoint condition"),
    );
    let on_change = Rc::new(on_change);
    breakpoint.connect_apply({
        let on_change = on_change.clone();
        move |_| on_change(true)
    });
    breakpoint.connect_unapply(move |_| on_change(false));
    bin.add_breakpoint(breakpoint);
    bin
}
