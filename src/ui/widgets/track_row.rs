//! One row of a track list (docs/design.md §4.1). Rows are recycled by
//! `gtk::ListView`: [`TrackRow::bind`] fills a row with a track,
//! [`TrackRow::unbind`] releases what it was showing (cover ticket).
//!
//! Variants (fixed per list, so every row of a list has one height):
//!
//! ```text
//! Cover (60)     [⠿] [cover 48▶] Title [E][HI-RES]      Album    ♥  4:34  ⋯
//!                                Artist, Artist
//! Numbered (54)      [ 3 / ▶ ]   Title [E]                       ♥  4:34  ⋯
//!                                feat. Artist (only if not the album's)
//! Queue (54)     [⠿] [cover 42]  Title [E]                          4:34  ✕
//! compact            [cover 48]  Title [E][HI-RES]                        ⋯
//!                                Artist · Album
//! ```
//!
//! Rows hold no menus or popovers: the "⋯" button and the drag-and-drop
//! handlers call back into the list ([`RowShared`]).

use std::cell::{Cell, OnceCell, RefCell};
use std::ops::Range;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

use super::heart_button::HeartButton;
use crate::covers::{Image, Ticket};
use crate::i18n::{tr, trn};
use crate::library;
use crate::model::{TrackInfo, format_duration};
use crate::ui::context::Ctx;
use crate::ui::nav::Route;

const COVER_PLACEHOLDER: &str = "audio-x-generic-symbolic";

/// Row layout, fixed when the list is created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Cover, title, artists, album column (favourites, playlists, mixes).
    Cover,
    /// Track number instead of the cover (albums).
    Numbered,
    /// Small cover and a remove button (the queue).
    Queue,
}

impl Variant {
    /// Row height without its 6 px top and bottom margins.
    fn inner_height(self) -> i32 {
        match self {
            Self::Cover => 48,
            Self::Numbered | Self::Queue => 42,
        }
    }

    fn cover_size(self) -> i32 {
        match self {
            Self::Queue => 42,
            _ => 48,
        }
    }
}

/// What the drag-and-drop of a track row carries inside Swell. Drop
/// targets that add tracks (sidebar playlists, the queue) accept this type;
/// other applications get `text/uri-list` and `text/plain` with the tracks'
/// TIDAL links.
#[derive(Clone, Debug, Default, glib::Boxed)]
#[boxed_type(name = "SwellTrackDrag")]
pub struct TrackDrag {
    pub track_ids: Vec<u64>,
    /// List id and track index the drag started from (for reordering).
    pub origin: Option<(u64, usize)>,
}

/// MIME type of the links offered to other applications.
pub const TRACK_URI_MIME: &str = "text/uri-list";

/// A track with what its row shows precomputed (formatting happens when
/// tracks are loaded, not on every bind).
#[derive(Debug)]
pub struct TrackEntry {
    pub info: TrackInfo,
    pub duration: String,
    /// "Title, by Artist, from Album, 3 minutes 5 seconds, Hi-Res, Explicit".
    pub accessible: String,
    /// Shown instead of the duration ("5 min ago" in Recently Played).
    pub note: Option<String>,
}

impl TrackEntry {
    pub fn new(info: TrackInfo) -> Self {
        let seconds = info.duration;
        let mut parts = vec![info.title.clone()];
        if !info.artists.is_empty() {
            parts.push(tr("by {artists}").replace("{artists}", &info.artist_names()));
        }
        if !info.album.is_empty() {
            parts.push(tr("from {album}").replace("{album}", &info.album));
        }
        parts.push(spoken_duration(seconds));
        if info.hires {
            parts.push(tr("Hi-Res").to_owned());
        }
        if info.explicit {
            parts.push(tr("Explicit").to_owned());
        }
        if !info.available {
            parts.push(tr("not available").to_owned());
        }
        let accessible = parts.join(", ");
        Self {
            duration: format_duration(seconds.into()),
            accessible,
            info,
            note: None,
        }
    }
}

/// "3 minutes 5 seconds".
pub fn spoken_duration(seconds: u32) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let unit = |n: u32, one: &'static str, many: &'static str| {
        trn(one, many, n.into()).replace("{n}", &n.to_string())
    };
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(unit(h, "{n} hour", "{n} hours"));
    }
    if m > 0 {
        parts.push(unit(m, "{n} minute", "{n} minutes"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(unit(s, "{n} second", "{n} seconds"));
    }
    parts.join(" ")
}

type RowCallback = Box<dyn Fn(&TrackRow, &gtk::Widget)>;
type MoveCallback = Box<dyn Fn(usize, usize)>;
type LocateCallback = Box<dyn Fn(&Rc<TrackEntry>, Option<u32>) -> Option<usize>>;

/// State a list shares with all its rows.
pub struct RowShared {
    pub ctx: Weak<Ctx>,
    pub variant: Variant,
    /// Unique per list, so a reorder drag only drops on its own list.
    pub list_id: u64,
    pub compact: Cell<bool>,
    pub reorderable: Cell<bool>,
    /// The list fills a page: rows span it, and each row's content starts
    /// at the page's content edge (instead of 12 px in).
    pub full_page: Cell<bool>,
    /// Rows show notes (wider than durations) instead of durations.
    pub notes: Cell<bool>,
    /// Numbered rows show their artists only when they differ from these.
    pub album_artists: RefCell<String>,
    /// The track playing now and whether it is paused.
    pub playing: Cell<Option<u64>>,
    pub paused: Cell<bool>,
    /// The "⋯" (or "✕" in the queue) button of a row was clicked.
    pub on_end_button: RefCell<Option<RowCallback>>,
    /// A row was dropped at another place: `(from, to)` track indices.
    pub on_move: RefCell<Option<MoveCallback>>,
    /// The current index of a track entry, given the model position its
    /// row is bound to (checked first) or found by looking for it. Rows
    /// are not bound again when tracks before them are added, removed or
    /// moved, so an index is only ever computed when it is used.
    pub locate: OnceCell<LocateCallback>,
    /// The entry a drag from this list started on (for the drop).
    pub dragging: RefCell<Option<Rc<TrackEntry>>>,
}

impl RowShared {
    pub fn new(ctx: &Rc<Ctx>, variant: Variant) -> Rc<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Rc::new(Self {
            ctx: Rc::downgrade(ctx),
            variant,
            list_id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            compact: Cell::new(false),
            reorderable: Cell::new(false),
            full_page: Cell::new(false),
            notes: Cell::new(false),
            album_artists: RefCell::default(),
            playing: Cell::new(None),
            paused: Cell::new(false),
            on_end_button: RefCell::default(),
            on_move: RefCell::default(),
            locate: OnceCell::new(),
            dragging: RefCell::default(),
        })
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TrackRow {
        pub shared: OnceCell<Rc<RowShared>>,
        pub handle: gtk::Image,
        pub cover: gtk::Image,
        pub cover_overlay: gtk::Image,
        pub cover_box: gtk::Overlay,
        pub number: gtk::Label,
        pub number_icon: gtk::Image,
        pub number_box: gtk::Overlay,
        pub title: gtk::Label,
        pub explicit: OnceCell<gtk::Label>,
        pub hires: OnceCell<gtk::Label>,
        pub secondary: gtk::Label,
        pub album: gtk::Label,
        pub heart: OnceCell<HeartButton>,
        pub duration: gtk::Label,
        pub end_button: gtk::Button,
        pub entry: RefCell<Option<Rc<TrackEntry>>>,
        pub item: RefCell<glib::WeakRef<gtk::ListItem>>,
        pub cover_ticket: RefCell<Option<Ticket>>,
        /// Byte ranges of the secondary line that link somewhere.
        pub links: RefCell<Vec<(Range<usize>, Route)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackRow {
        const NAME: &'static str = "SwellTrackRow";
        type Type = super::TrackRow;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for TrackRow {}
    impl WidgetImpl for TrackRow {}
    impl BoxImpl for TrackRow {}
}

glib::wrapper! {
    pub struct TrackRow(ObjectSubclass<imp::TrackRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl TrackRow {
    pub fn new(ctx: &Rc<Ctx>, shared: &Rc<RowShared>) -> Self {
        let row: Self = glib::Object::new();
        let _ = row.imp().shared.set(shared.clone());
        row.build(ctx, shared);
        row
    }

    fn shared(&self) -> &Rc<RowShared> {
        self.imp()
            .shared
            .get()
            .expect("rows are built with their list")
    }

    fn build(&self, ctx: &Rc<Ctx>, shared: &RowShared) {
        let imp = self.imp();
        let variant = shared.variant;
        self.add_css_class("track-row");
        self.set_spacing(12);
        self.set_margin_top(6);
        self.set_margin_bottom(6);
        if shared.full_page.get() {
            super::section::align_to_edge(self, 0);
        } else {
            self.set_margin_start(12);
            self.set_margin_end(12);
        }
        self.set_height_request(variant.inner_height());

        imp.handle.set_icon_name(Some("list-drag-handle-symbolic"));
        imp.handle.set_pixel_size(16);
        imp.handle.set_width_request(24);
        imp.handle.set_cursor_from_name(Some("grab"));
        imp.handle
            .set_tooltip_text(Some(tr("Drag to Reorder (Alt+↑/↓)")));
        imp.handle.set_visible(false);
        self.append(&imp.handle);

        match variant {
            Variant::Cover | Variant::Queue => {
                let size = variant.cover_size();
                imp.cover.set_pixel_size(size);
                imp.cover.add_css_class("cover");
                imp.cover.set_overflow(gtk::Overflow::Hidden);
                imp.cover_overlay.set_pixel_size(size);
                imp.cover_overlay
                    .set_icon_name(Some("media-playback-start-symbolic"));
                imp.cover_overlay.add_css_class("cover-overlay");
                imp.cover_overlay.set_can_target(false);
                imp.cover_overlay
                    .set_accessible_role(gtk::AccessibleRole::Presentation);
                imp.cover_box.set_child(Some(&imp.cover));
                imp.cover_box.add_overlay(&imp.cover_overlay);
                imp.cover_box.set_valign(gtk::Align::Center);
                self.append(&imp.cover_box);
            }
            Variant::Numbered => {
                imp.number.set_width_chars(3);
                imp.number.set_xalign(1.0);
                imp.number.set_width_request(36);
                imp.number.add_css_class("numeric");
                imp.number.add_css_class("dimmed");
                imp.number.add_css_class("track-number");
                imp.number_icon
                    .set_icon_name(Some("media-playback-start-symbolic"));
                imp.number_icon.set_halign(gtk::Align::End);
                imp.number_icon.set_margin_end(2);
                imp.number_icon.add_css_class("row-play-icon");
                imp.number_icon.set_can_target(false);
                imp.number_box.set_child(Some(&imp.number));
                imp.number_box.add_overlay(&imp.number_icon);
                imp.number_box.set_valign(gtk::Align::Center);
                self.append(&imp.number_box);
            }
        }

        for label in [&imp.title, &imp.secondary, &imp.album] {
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_single_line_mode(true);
        }
        imp.title.add_css_class("track-title");
        imp.secondary.add_css_class("dimmed");
        imp.album.add_css_class("dimmed");

        let title_line = gtk::Box::builder().spacing(6).build();
        title_line.append(&imp.title);
        let explicit =
            super::quality_badge::QualityBadge::new(super::quality_badge::BadgeKind::Explicit);
        title_line.append(&explicit);
        let _ = imp.explicit.set(explicit);
        if variant == Variant::Cover {
            let hires =
                super::quality_badge::QualityBadge::new(super::quality_badge::BadgeKind::HiRes);
            title_line.append(&hires);
            let _ = imp.hires.set(hires);
        }

        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .build();
        text.append(&title_line);
        text.append(&imp.secondary);

        if variant == Variant::Cover {
            // Equal widths for the title and album columns, so albums line
            // up from row to row whatever the length of the titles.
            let columns = gtk::Box::builder()
                .homogeneous(true)
                .spacing(12)
                .hexpand(true)
                .build();
            columns.append(&text);
            columns.append(&imp.album);
            self.append(&columns);
        } else {
            text.set_hexpand(true);
            self.append(&text);
        }

        if variant != Variant::Queue {
            let heart = HeartButton::unlistened(ctx);
            self.append(heart.widget());
            let _ = imp.heart.set(heart);
        }

        imp.duration.set_width_chars(5);
        imp.duration.set_xalign(1.0);
        imp.duration.add_css_class("numeric");
        imp.duration.add_css_class("dimmed");
        self.append(&imp.duration);

        imp.end_button.set_valign(gtk::Align::Center);
        imp.end_button.add_css_class("circular");
        imp.end_button.add_css_class("flat");
        if variant == Variant::Queue {
            imp.end_button.set_icon_name("window-close-symbolic");
            super::label_button(&imp.end_button, tr("Remove from Queue"));
        } else {
            imp.end_button.set_icon_name("view-more-symbolic");
            imp.end_button.add_css_class("row-more");
            super::label_button(&imp.end_button, tr("More Options"));
        }
        let weak = self.downgrade();
        imp.end_button.connect_clicked(move |button| {
            if let Some(row) = weak.upgrade()
                && let Some(callback) = row.shared().on_end_button.borrow().as_ref()
            {
                callback(&row, button.upcast_ref());
            }
        });
        self.append(&imp.end_button);

        self.add_link_gesture(&imp.secondary);
        self.add_link_gesture(&imp.album);
        self.add_drag_source();
        self.add_drop_target();
    }

    /// Clicks on the artist and album names open them. The labels are not
    /// focusable links (Tab stays on rows); the menu offers the same.
    fn add_link_gesture(&self, label: &gtk::Label) {
        label.set_cursor_from_name(Some("pointer"));
        let gesture = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        let weak = self.downgrade();
        let target = label.downgrade();
        gesture.connect_released(move |gesture, n_press, x, y| {
            let (Some(row), Some(label)) = (weak.upgrade(), target.upgrade()) else {
                return;
            };
            if n_press != 1 || row.shared().compact.get() {
                return;
            }
            let route = if label == row.imp().album {
                row.imp()
                    .entry
                    .borrow()
                    .as_ref()
                    .filter(|e| e.info.album_id != 0)
                    .map(|e| Route::Album(e.info.album_id))
            } else {
                row.link_at(&label, x, y)
            };
            if let Some(route) = route
                && let Some(ctx) = row.shared().ctx.upgrade()
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                ctx.navigate(route);
            }
        });
        label.add_controller(gesture);
    }

    fn link_at(&self, label: &gtk::Label, x: f64, y: f64) -> Option<Route> {
        let layout = label.layout();
        let (ox, oy) = label.layout_offsets();
        let scale = f64::from(gtk::pango::SCALE);
        let (inside, index, _) = layout.xy_to_index(
            ((x - f64::from(ox)) * scale) as i32,
            ((y - f64::from(oy)) * scale) as i32,
        );
        if !inside {
            return None;
        }
        let index = index as usize;
        self.imp()
            .links
            .borrow()
            .iter()
            .find(|(range, _)| range.contains(&index))
            .map(|(_, route)| route.clone())
    }

    fn add_drag_source(&self) {
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::COPY | gdk::DragAction::MOVE)
            .build();
        let weak = self.downgrade();
        source.connect_prepare(move |source, _, _| {
            let row = weak.upgrade()?;
            let entry = row.imp().entry.borrow().clone()?;
            let shared = row.shared();
            let origin = if shared.reorderable.get() {
                let index = row.index()?;
                shared.dragging.replace(Some(entry.clone()));
                Some((shared.list_id, index))
            } else {
                None
            };
            let drag = TrackDrag {
                track_ids: vec![entry.info.id],
                origin,
            };
            let uri = format!("{}\r\n", entry.info.share_url());
            if let Some(paintable) = row.imp().cover.paintable() {
                source.set_icon(Some(&paintable), 0, 0);
            }
            Some(gdk::ContentProvider::new_union(&[
                gdk::ContentProvider::for_value(&drag.to_value()),
                gdk::ContentProvider::for_bytes(
                    TRACK_URI_MIME,
                    &glib::Bytes::from_owned(uri.clone()),
                ),
                gdk::ContentProvider::for_value(&entry.info.share_url().to_value()),
            ]))
        });
        let weak = self.downgrade();
        source.connect_drag_end(move |_, _, _| {
            if let Some(row) = weak.upgrade() {
                row.shared().dragging.take();
            }
        });
        self.add_controller(source);
    }

    fn add_drop_target(&self) {
        let target = gtk::DropTarget::new(TrackDrag::static_type(), gdk::DragAction::MOVE);
        target.set_preload(true);
        let weak = self.downgrade();
        target.connect_motion(move |target, _, y| {
            let Some(row) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            if !row.accepts(target.value().as_ref()) {
                return gdk::DragAction::empty();
            }
            let below = y > f64::from(row.height()) / 2.0;
            row.set_drop_marker(Some(below));
            gdk::DragAction::MOVE
        });
        let weak = self.downgrade();
        target.connect_leave(move |_| {
            if let Some(row) = weak.upgrade() {
                row.set_drop_marker(None);
            }
        });
        let weak = self.downgrade();
        target.connect_drop(move |_, value, _, y| {
            let Some(row) = weak.upgrade() else {
                return false;
            };
            row.set_drop_marker(None);
            if !row.accepts(Some(value)) {
                return false;
            }
            // Where the dragged track and this row are now: the list may
            // have changed since the drag started.
            let shared = row.shared();
            let dragged = shared.dragging.borrow().clone();
            let (Some(from), Some(here)) = (
                dragged.and_then(|entry| shared.locate.get()?(&entry, None)),
                row.index(),
            ) else {
                return false;
            };
            let below = y > f64::from(row.height()) / 2.0;
            let mut to = here + usize::from(below);
            if from < to {
                to -= 1;
            }
            if from != to
                && let Some(on_move) = row.shared().on_move.borrow().as_ref()
            {
                on_move(from, to);
            }
            true
        });
        self.add_controller(target);
    }

    fn accepts(&self, value: Option<&glib::Value>) -> bool {
        let shared = self.shared();
        shared.reorderable.get()
            && value
                .and_then(|v| v.get::<TrackDrag>().ok())
                .and_then(|drag| drag.origin)
                .is_some_and(|(list, _)| list == shared.list_id)
    }

    fn set_drop_marker(&self, below: Option<bool>) {
        self.remove_css_class("drop-above");
        self.remove_css_class("drop-below");
        match below {
            Some(true) => self.add_css_class("drop-below"),
            Some(false) => self.add_css_class("drop-above"),
            None => {}
        }
    }

    /// Fills the row with the entry at model `position`.
    pub fn bind(&self, entry: &Rc<TrackEntry>, position: u32, item: &gtk::ListItem) {
        let imp = self.imp();
        let shared = self.shared().clone();
        let track = &entry.info;
        imp.entry.replace(Some(entry.clone()));
        imp.item.replace(item.downgrade());

        imp.title.set_label(&track.title);
        imp.title.set_tooltip_text(Some(&track.title));
        if let Some(explicit) = imp.explicit.get() {
            explicit.set_visible(track.explicit);
        }
        if let Some(hires) = imp.hires.get() {
            hires.set_visible(track.hires);
        }
        imp.album.set_label(&track.album);
        imp.duration
            .set_label(entry.note.as_deref().unwrap_or(&entry.duration));
        if shared.variant == Variant::Numbered {
            imp.number.set_label(&track.track_number.max(1).to_string());
        }
        if let Some(heart) = imp.heart.get() {
            heart.set_item(Some(library::Item::Track(track.id)));
        }
        if track.available {
            self.remove_css_class("dimmed");
            self.set_tooltip_text(None);
        } else {
            self.add_css_class("dimmed");
            self.set_tooltip_text(Some(tr("Not available in your region")));
        }
        self.apply_layout();
        self.update_playing();

        if matches!(shared.variant, Variant::Cover | Variant::Queue) {
            imp.cover.set_icon_name(Some(COVER_PLACEHOLDER));
            let ticket = match (track.cover.as_deref(), shared.ctx.upgrade()) {
                (Some(id), Some(ctx)) => {
                    let row = self.downgrade();
                    let pixels = super::device_pixels(self, shared.variant.cover_size());
                    ctx.covers.load(
                        &Image::Cover(id.to_owned()),
                        pixels,
                        position,
                        move |texture| {
                            if let Some(row) = row.upgrade() {
                                row.imp().cover.set_paintable(Some(texture));
                            }
                        },
                    )
                }
                _ => None,
            };
            imp.cover_ticket.replace(ticket);
        }
    }

    pub fn unbind(&self) {
        let imp = self.imp();
        imp.cover_ticket.take();
        imp.entry.take();
        imp.item.replace(glib::WeakRef::new());
        self.set_drop_marker(None);
    }

    pub fn entry(&self) -> Option<Rc<TrackEntry>> {
        self.imp().entry.borrow().clone()
    }

    pub fn track_id(&self) -> Option<u64> {
        self.imp().entry.borrow().as_ref().map(|e| e.info.id)
    }

    /// The track's index among the list's tracks now (`None` when the row
    /// is not bound). Computed on every call from the position of the row's
    /// list item, which GTK keeps current while tracks before it come and
    /// go without binding the row again.
    pub fn index(&self) -> Option<usize> {
        let imp = self.imp();
        let entry = imp.entry.borrow().clone()?;
        let position = imp
            .item
            .borrow()
            .upgrade()
            .map(|item| item.position())
            .filter(|&position| position != gtk::INVALID_LIST_POSITION);
        self.shared().locate.get()?(&entry, position)
    }

    pub fn heart(&self) -> Option<&HeartButton> {
        self.imp().heart.get()
    }

    /// Applies the list's `compact` and `reorderable` settings.
    pub fn apply_layout(&self) {
        let imp = self.imp();
        let shared = self.shared();
        let compact = shared.compact.get();
        imp.handle.set_visible(shared.reorderable.get() && !compact);
        imp.album
            .set_visible(!compact && shared.variant == Variant::Cover);
        imp.duration.set_visible(!compact);
        imp.duration
            .set_width_chars(if shared.notes.get() { 10 } else { 5 });
        if let Some(heart) = imp.heart.get() {
            heart.widget().set_visible(!compact);
        }
        let Some(entry) = imp.entry.borrow().clone() else {
            return;
        };
        let track = &entry.info;
        let mut links = Vec::new();
        let mut text = String::new();
        let show_artists = match shared.variant {
            Variant::Numbered => *shared.album_artists.borrow() != track.artist_names(),
            _ => true,
        };
        if show_artists {
            for (n, artist) in track.artists.iter().enumerate() {
                if n > 0 {
                    text.push_str(", ");
                }
                let start = text.len();
                text.push_str(&artist.name);
                links.push((start..text.len(), Route::Artist(artist.id)));
            }
        }
        if compact && shared.variant == Variant::Cover && !track.album.is_empty() {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            let start = text.len();
            text.push_str(&track.album);
            if track.album_id != 0 {
                links.push((start..text.len(), Route::Album(track.album_id)));
            }
        }
        imp.secondary.set_visible(!text.is_empty());
        imp.secondary.set_label(&text);
        imp.links.replace(links);
    }

    /// Shows whether this row's track is the one playing.
    pub fn update_playing(&self) {
        let imp = self.imp();
        let shared = self.shared();
        let playing = self.track_id().is_some() && self.track_id() == shared.playing.get();
        let paused = shared.paused.get();
        if playing {
            self.add_css_class("playing");
        } else {
            self.remove_css_class("playing");
        }
        // Playing: the speaker; paused (or not this row): play, which is
        // what a click does.
        let icon = match (playing, paused) {
            (true, false) => "audio-volume-high-symbolic",
            _ => "media-playback-start-symbolic",
        };
        match shared.variant {
            Variant::Numbered => {
                imp.number_icon.set_icon_name(Some(icon));
                // The hover rule hides the icon; the playing row keeps it.
                if playing {
                    imp.number_icon.remove_css_class("row-play-icon");
                    imp.number_icon.add_css_class("accent");
                    imp.number.set_opacity(0.0);
                } else {
                    imp.number_icon.add_css_class("row-play-icon");
                    imp.number_icon.remove_css_class("accent");
                    imp.number.set_opacity(1.0);
                }
            }
            _ => imp.cover_overlay.set_icon_name(Some(icon)),
        }
        if let (Some(item), Some(entry)) =
            (imp.item.borrow().upgrade(), imp.entry.borrow().as_ref())
        {
            if playing {
                item.set_accessible_label(
                    &tr("{track}, playing").replace("{track}", &entry.accessible),
                );
            } else {
                item.set_accessible_label(&entry.accessible);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken() {
        if crate::i18n::ui_language().is_some() {
            // Checks the English texts.
            return;
        }
        assert_eq!(spoken_duration(369), "6 minutes 9 seconds");
        assert_eq!(spoken_duration(60), "1 minute");
        assert_eq!(spoken_duration(0), "0 seconds");
        assert_eq!(spoken_duration(3601), "1 hour 1 second");
    }
}
