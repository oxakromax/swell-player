//! Small pieces shared by the player bar, the mini player and the
//! now-playing panel: playback/library subscriptions that end with their
//! owner, a position ticker, covers, links, badges, the heart and the
//! track menu.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::super::context::Ctx;
use super::super::nav::Route;
use crate::covers::{CoverLoader, Image, Ticket};
use crate::library::{Change, Item, Library};
use crate::model::TrackInfo;
use crate::playback::{Event, PlayState, Playback};
use crate::settings::{Normalization, QualityPreference};

/// A playback subscription, removed when dropped.
pub(super) struct Subscription {
    playback: Rc<Playback>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.playback.unsubscribe(self.id);
    }
}

/// Subscribes `f` to `playback` for as long as `owner` lives (and the
/// returned subscription is kept).
pub(super) fn subscribe<T: 'static>(
    playback: &Rc<Playback>,
    owner: &Weak<T>,
    f: impl Fn(&Rc<T>, &Event) + 'static,
) -> Subscription {
    let owner = owner.clone();
    let id = playback.subscribe(move |event| {
        if let Some(owner) = owner.upgrade() {
            f(&owner, event);
        }
    });
    Subscription {
        playback: playback.clone(),
        id,
    }
}

/// A library listener, disconnected when dropped.
pub(super) struct LibraryListener {
    library: Rc<Library>,
    id: u64,
}

impl Drop for LibraryListener {
    fn drop(&mut self) {
        self.library.disconnect(self.id);
    }
}

/// Calls a function every `interval` while its widget is mapped and the
/// player is playing (or buffering), and once whenever that starts or the track changes.
/// Stops by itself when dropped.
pub(super) struct Ticker {
    inner: Rc<TickerInner>,
    _events: Subscription,
    handlers: Vec<glib::SignalHandlerId>,
}

struct TickerInner {
    widget: glib::WeakRef<gtk::Widget>,
    playback: Rc<Playback>,
    interval: Duration,
    callback: Box<dyn Fn()>,
    source: RefCell<Option<glib::SourceId>>,
}

impl Ticker {
    pub fn new(
        widget: &impl IsA<gtk::Widget>,
        playback: &Rc<Playback>,
        interval: Duration,
        callback: impl Fn() + 'static,
    ) -> Self {
        let widget = widget.upcast_ref::<gtk::Widget>();
        let inner = Rc::new(TickerInner {
            widget: widget.downgrade(),
            playback: playback.clone(),
            interval,
            callback: Box::new(callback),
            source: RefCell::default(),
        });
        let weak = Rc::downgrade(&inner);
        let handlers = vec![
            widget.connect_map({
                let weak = weak.clone();
                move |_| {
                    if let Some(inner) = weak.upgrade() {
                        inner.update(true);
                    }
                }
            }),
            widget.connect_unmap({
                let weak = weak.clone();
                move |_| {
                    if let Some(inner) = weak.upgrade() {
                        inner.update(false);
                    }
                }
            }),
        ];
        let events = subscribe(playback, &weak, |inner, event| match event {
            Event::State(_) => inner.update(false),
            Event::Track(_) => inner.update(true),
            _ => {}
        });
        Self {
            inner,
            _events: events,
            handlers,
        }
    }
}

impl TickerInner {
    fn update(self: &Rc<Self>, force: bool) {
        let Some(widget) = self.widget.upgrade() else {
            return;
        };
        let mapped = widget.is_mapped();
        let run = mapped
            && matches!(
                self.playback.state(),
                PlayState::Playing | PlayState::Loading
            );
        if run && self.source.borrow().is_none() {
            (self.callback)();
            let weak = Rc::downgrade(self);
            let id = glib::timeout_add_local(self.interval, move || match weak.upgrade() {
                Some(inner) => {
                    (inner.callback)();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
            self.source.replace(Some(id));
            return;
        }
        if !run && let Some(id) = self.source.take() {
            id.remove();
        }
        if mapped && (force || !run) {
            (self.callback)();
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        if let Some(id) = self.inner.source.take() {
            id.remove();
        }
        if let Some(widget) = self.inner.widget.upgrade() {
            for handler in self.handlers.drain(..) {
                widget.disconnect(handler);
            }
        }
    }
}

/// The first of `candidates` the icon theme has (the first one if none).
fn icon(candidates: &[&'static str]) -> &'static str {
    thread_local! {
        static FOUND: RefCell<HashMap<&'static str, &'static str>> = RefCell::default();
    }
    FOUND.with_borrow_mut(|found| {
        *found.entry(candidates[0]).or_insert_with(|| {
            let Some(display) = gdk::Display::default() else {
                return candidates[0];
            };
            let theme = gtk::IconTheme::for_display(&display);
            candidates
                .iter()
                .copied()
                .find(|name| theme.has_icon(name))
                .unwrap_or(candidates[0])
        })
    })
}

/// Icon names with fallbacks: Swell's own icons are not bundled yet, and
/// some themes (Breeze) lack a few standard names.
pub(super) mod icons {
    use super::icon;

    pub fn track() -> &'static str {
        icon(&["audio-x-generic-symbolic", "emblem-music-symbolic"])
    }

    pub fn queue() -> &'static str {
        icon(&["view-list-ordered-symbolic", "view-media-playlist-symbolic"])
    }

    pub fn lyrics() -> &'static str {
        icon(&[
            "swell-lyrics-symbolic",
            "view-media-lyrics-symbolic",
            "format-justify-left-symbolic",
        ])
    }

    pub fn radio() -> &'static str {
        icon(&[
            "swell-radio-symbolic",
            "audio-radio-symbolic",
            "network-wireless-symbolic",
        ])
    }

    pub fn heart() -> &'static str {
        icon(&["swell-heart-symbolic", "non-starred-symbolic"])
    }

    pub fn heart_filled() -> &'static str {
        icon(&["swell-heart-filled-symbolic", "starred-symbolic"])
    }

    pub fn drag_handle() -> &'static str {
        icon(&["list-drag-handle-symbolic", "drag-handle-symbolic"])
    }
}

/// Device pixels for `logical` pixels of `widget` (fractional scaling aware).
pub(super) fn device_pixels(widget: &impl IsA<gtk::Widget>, logical: i32) -> u32 {
    let scale = widget
        .native()
        .and_then(|native| native.surface())
        .map(|surface| surface.scale())
        .unwrap_or_else(|| f64::from(widget.scale_factor()));
    (f64::from(logical) * scale.max(1.0)).ceil() as u32
}

/// A small square cover (`gtk::Image`) that loads through the cover loader.
pub(super) struct Cover {
    pub image: gtk::Image,
    size: i32,
    shown: RefCell<Option<String>>,
    ticket: RefCell<Option<Ticket>>,
}

impl Cover {
    pub fn new(size: i32) -> Self {
        let image = gtk::Image::builder()
            .icon_name(icons::track())
            .pixel_size(size)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["cover"])
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        Self {
            image,
            size,
            shown: RefCell::default(),
            ticket: RefCell::default(),
        }
    }

    pub fn set(&self, covers: &Rc<CoverLoader>, cover: Option<&str>) {
        if self.shown.borrow().as_deref() == cover && cover.is_some() {
            return;
        }
        self.ticket.take();
        self.shown.replace(cover.map(str::to_owned));
        self.image.set_icon_name(Some(icons::track()));
        let Some(cover) = cover else { return };
        let image = self.image.downgrade();
        let ticket = covers.load(
            &Image::Cover(cover.to_owned()),
            device_pixels(&self.image, self.size),
            0,
            move |texture| {
                if let Some(image) = image.upgrade() {
                    image.set_paintable(Some(texture));
                }
            },
        );
        self.ticket.replace(ticket);
    }
}

/// Escapes text for Pango markup.
pub(super) fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// "<a>Artist A</a>, <a>Artist B</a>" with `artist:ID` links.
pub(super) fn artists_markup(track: &TrackInfo) -> String {
    track
        .artists
        .iter()
        .map(|artist| {
            format!(
                "<a href=\"artist:{}\">{}</a>",
                artist.id,
                escape(&artist.name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `<a href="album:ID">text</a>`.
pub(super) fn album_markup(track: &TrackInfo, text: &str) -> String {
    if track.album_id == 0 {
        return escape(text);
    }
    format!("<a href=\"album:{}\">{}</a>", track.album_id, escape(text))
}

/// A label for markup with `album:`/`artist:` links that navigate.
pub(super) fn link_label(ctx: &Rc<Ctx>, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .use_markup(true)
        .css_classes(
            classes
                .iter()
                .copied()
                .chain(["np-link"])
                .collect::<Vec<_>>(),
        )
        .build();
    let ctx = Rc::downgrade(ctx);
    label.connect_activate_link(move |_, uri| {
        if let Some(ctx) = ctx.upgrade()
            && let Some(route) = link_route(uri)
        {
            ctx.navigate(route);
        }
        glib::Propagation::Stop
    });
    label
}

fn link_route(uri: &str) -> Option<Route> {
    let (kind, id) = uri.split_once(':')?;
    let id = id.parse().ok()?;
    match kind {
        "album" => Some(Route::Album(id)),
        "artist" => Some(Route::Artist(id)),
        _ => None,
    }
}

/// The explicit badge (§4.5).
pub(super) fn explicit_badge() -> gtk::Label {
    let badge = gtk::Label::builder()
        .label("E")
        .valign(gtk::Align::Center)
        .css_classes(["badge", "explicit"])
        .tooltip_text("Explicit")
        .build();
    badge.update_property(&[gtk::accessible::Property::Label("Explicit")]);
    badge
}

/// The available-quality badge: HI-RES or LOSSLESS (§4.5). A private copy
/// until the shared `QualityBadge` lands.
pub(super) fn quality_badge() -> gtk::Label {
    gtk::Label::builder()
        .valign(gtk::Align::Center)
        .css_classes(["badge"])
        .build()
}

pub(super) fn set_quality_badge(badge: &gtk::Label, hires: bool) {
    let (text, name) = if hires {
        ("HI-RES", "Hi-Res Lossless")
    } else {
        ("LOSSLESS", "Lossless")
    };
    badge.set_label(text);
    badge.set_tooltip_text(Some(name));
    badge.update_property(&[gtk::accessible::Property::Label(name)]);
    if hires {
        badge.add_css_class("hires");
    } else {
        badge.remove_css_class("hires");
    }
}

/// The delivered format, parsed from `Playback::format()` ("24-bit/96 kHz
/// FLAC", "AAC 320 kbit/s").
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Format {
    pub codec: String,
    pub bits: Option<u32>,
    /// "96", "44.1".
    pub khz: Option<String>,
    pub bitrate: Option<String>,
}

impl Format {
    pub fn parse(text: &str) -> Self {
        if let Some((bits, rest)) = text.split_once("-bit/") {
            let mut words = rest.split_whitespace();
            let khz = words.next().map(str::to_owned);
            let codec = rest
                .split_whitespace()
                .last()
                .filter(|w| *w != "kHz")
                .unwrap_or("FLAC")
                .to_owned();
            return Self {
                codec,
                bits: bits.trim().parse().ok(),
                khz,
                bitrate: None,
            };
        }
        let mut words = text.split_whitespace();
        let codec = words.next().unwrap_or("").to_owned();
        let bitrate = words.next().map(str::to_owned);
        Self {
            codec,
            bits: None,
            khz: None,
            bitrate,
        }
    }

    pub fn hires(&self) -> bool {
        self.bits.is_some_and(|bits| bits >= 24)
    }

    pub fn lossless(&self) -> bool {
        self.bits.is_some() || self.codec.eq_ignore_ascii_case("flac")
    }

    /// "FLAC 24/96", "AAC 320".
    pub fn short(&self) -> String {
        match (&self.bits, &self.khz, &self.bitrate) {
            (Some(bits), Some(khz), _) => format!("{} {bits}/{khz}", self.codec),
            (_, _, Some(rate)) => format!("{} {rate}", self.codec),
            _ => self.codec.clone(),
        }
    }

    /// "HI-RES", "LOSSLESS" or the codec.
    pub fn compact(&self) -> String {
        if self.hires() {
            "HI-RES".into()
        } else if self.lossless() {
            "LOSSLESS".into()
        } else {
            self.codec.to_uppercase()
        }
    }

    pub fn quality(&self) -> &'static str {
        if self.hires() {
            "Hi-Res Lossless"
        } else if self.lossless() {
            "Lossless"
        } else {
            "High"
        }
    }

    /// "24-bit · 96 kHz" or "320 kbit/s".
    pub fn delivered(&self) -> String {
        match (&self.bits, &self.khz) {
            (Some(bits), Some(khz)) => format!("{bits}-bit · {khz} kHz"),
            _ => self
                .bitrate
                .as_ref()
                .map(|rate| format!("{rate} kbit/s"))
                .unwrap_or_else(|| "—".into()),
        }
    }
}

/// The delivered-format badge: a flat menu button ("FLAC 24/96") whose
/// popover lists the details (§4.5).
pub(super) struct FormatBadge {
    pub widget: gtk::MenuButton,
    label: gtk::Label,
    values: [gtk::Label; 5],
    note: gtk::Label,
    compact: Cell<bool>,
    format: RefCell<Option<Format>>,
}

impl FormatBadge {
    pub fn new() -> Self {
        let label = gtk::Label::builder()
            .css_classes(["badge", "numeric"])
            .build();
        let widget = gtk::MenuButton::builder()
            .child(&label)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "format-badge"])
            .tooltip_text("Audio Format")
            .visible(false)
            .build();

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(18)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let values: [gtk::Label; 5] = std::array::from_fn(|_| {
            gtk::Label::builder()
                .xalign(0.0)
                .selectable(true)
                .css_classes(["numeric"])
                .build()
        });
        for (row, name) in [
            "Quality",
            "Codec",
            "Delivered",
            "Output device",
            "Normalisation",
        ]
        .into_iter()
        .enumerate()
        {
            let key = gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .css_classes(["dimmed"])
                .build();
            grid.attach(&key, 0, row as i32, 1, 1);
            grid.attach(&values[row], 1, row as i32, 1, 1);
        }
        let note = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(36)
            .css_classes(["caption", "dimmed"])
            .visible(false)
            .build();
        grid.attach(&note, 0, 5, 2, 1);
        let popover = gtk::Popover::builder().child(&grid).build();
        widget.set_popover(Some(&popover));
        Self {
            widget,
            label,
            values,
            note,
            compact: Cell::new(false),
            format: RefCell::default(),
        }
    }

    pub fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.refresh_label();
    }

    /// Shows `format` (hidden with `None`). `track` and `ctx` explain a
    /// delivered quality below what the track offers.
    pub fn set(&self, ctx: &Ctx, track: Option<&TrackInfo>, format: Option<&str>) {
        let Some(text) = format else {
            self.format.replace(None);
            self.widget.set_visible(false);
            return;
        };
        let format = Format::parse(text);
        let settings = ctx.settings.get();
        self.values[0].set_label(format.quality());
        self.values[1].set_label(&format.codec);
        self.values[2].set_label(&format.delivered());
        self.values[3].set_label("System default");
        self.values[4].set_label(match settings.normalization {
            Normalization::Off => "Off",
            Normalization::Track => "Track",
            Normalization::Album => "Album",
        });
        let note = match track {
            Some(track) if track.hires && !format.hires() => Some(match settings.quality {
                QualityPreference::Best => "Lossless delivered: this track has no Hi-Res master",
                _ => "Lossless delivered: Hi-Res is turned off in Preferences",
            }),
            _ => None,
        };
        self.note.set_label(note.unwrap_or_default());
        self.note.set_visible(note.is_some());
        if format.hires() {
            self.label.add_css_class("hires");
        } else {
            self.label.remove_css_class("hires");
        }
        self.widget
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Audio format: {}, {}",
                format.quality(),
                format.delivered()
            ))]);
        self.format.replace(Some(format));
        self.refresh_label();
        self.widget.set_visible(true);
    }

    fn refresh_label(&self) {
        if let Some(format) = self.format.borrow().as_ref() {
            self.label.set_label(&if self.compact.get() {
                format.compact()
            } else {
                format.short()
            });
        }
    }
}

/// The favourite toggle for the current track (§4.6).
pub(super) struct Heart {
    pub button: gtk::ToggleButton,
    ctx: Weak<Ctx>,
    library: Rc<Library>,
    track: Cell<Option<u64>>,
    syncing: Cell<bool>,
    icons: (&'static str, &'static str),
    _listener: LibraryListener,
}

impl Heart {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let icons = (icons::heart(), icons::heart_filled());
        let button = gtk::ToggleButton::builder()
            .icon_name(icons.0)
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat", "heart"])
            .tooltip_text("Add to Favourites")
            .sensitive(false)
            .build();
        button.update_property(&[gtk::accessible::Property::Label("Favourite")]);
        let library = ctx.library.clone();
        let heart = Rc::new_cyclic(|weak: &Weak<Self>| {
            let weak = weak.clone();
            let id = library.connect_changed(move |change| {
                let Some(heart) = weak.upgrade() else { return };
                match change {
                    Change::Loaded => heart.sync(),
                    Change::Favorite(Item::Track(id), _) if Some(*id) == heart.track.get() => {
                        heart.sync()
                    }
                    _ => {}
                }
            });
            Self {
                button,
                ctx: Rc::downgrade(ctx),
                library: library.clone(),
                track: Cell::new(None),
                syncing: Cell::new(false),
                icons,
                _listener: LibraryListener { library, id },
            }
        });
        heart.button.connect_toggled({
            let heart = Rc::downgrade(&heart);
            move |button| {
                let Some(heart) = heart.upgrade() else { return };
                if heart.syncing.get() {
                    return;
                }
                let Some(id) = heart.track.get() else { return };
                let ctx = heart.ctx.clone();
                heart
                    .library
                    .set_favorite(Item::Track(id), button.is_active(), move |result| {
                        if let (Err(err), Some(ctx)) = (result, ctx.upgrade()) {
                            ctx.report("Couldn't update favourites", &err);
                        }
                    });
                heart.sync();
            }
        });
        heart
    }

    pub fn set_track(&self, track: Option<u64>) {
        self.track.set(track);
        self.sync();
    }

    fn sync(&self) {
        let favorite = self
            .track
            .get()
            .is_some_and(|id| self.library.is_favorite(&Item::Track(id)));
        self.syncing.set(true);
        self.button.set_active(favorite);
        self.syncing.set(false);
        self.button.set_sensitive(self.track.get().is_some());
        self.button
            .set_icon_name(if favorite { self.icons.1 } else { self.icons.0 });
        self.button.set_tooltip_text(Some(if favorite {
            "Remove from Favourites"
        } else {
            "Add to Favourites"
        }));
    }
}

/// Actions on the current track ("player.*"): go to album/artist, track
/// radio, copy link.
fn track_actions(ctx: &Rc<Ctx>) -> gio::SimpleActionGroup {
    let group = gio::SimpleActionGroup::new();
    let add = |name: &str, run: fn(&Ctx, &TrackInfo)| {
        let action = gio::SimpleAction::new(name, None);
        let ctx = Rc::downgrade(ctx);
        action.connect_activate(move |_, _| {
            if let Some(ctx) = ctx.upgrade()
                && let Some(track) = ctx.playback.current()
            {
                run(&ctx, &track);
            }
        });
        group.add_action(&action);
    };
    add("album", |ctx, track| {
        if track.album_id != 0 {
            ctx.navigate(Route::Album(track.album_id));
        }
    });
    add("artist", |ctx, track| {
        if let Some(artist) = track.main_artist() {
            ctx.navigate(Route::Artist(artist.id));
        }
    });
    add("radio", |ctx, track| {
        ctx.navigate(Route::TrackRadio(track.id))
    });
    add("copy-link", |ctx, track| {
        ctx.copy(&track.share_url(), "Link")
    });
    group
}

/// The "⋯" menu button for the current track.
pub(super) fn track_menu_button(ctx: &Rc<Ctx>) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    let navigate = gio::Menu::new();
    navigate.append(Some("Go to Album"), Some("player.album"));
    navigate.append(Some("Go to Artist"), Some("player.artist"));
    navigate.append(Some("Track Radio"), Some("player.radio"));
    menu.append_section(None, &navigate);
    let share = gio::Menu::new();
    share.append(Some("Copy Link"), Some("player.copy-link"));
    menu.append_section(None, &share);
    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&menu)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .tooltip_text("More")
        .build();
    button.insert_action_group("player", Some(&track_actions(ctx)));
    button
}

/// "2 minutes 1 second", for screen readers.
pub(super) fn spoken_duration(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let unit = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(unit(h, "hour", "hours"));
    }
    if m > 0 {
        parts.push(unit(m, "minute", "minutes"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(unit(s, "second", "seconds"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let hires = Format::parse("24-bit/96 kHz FLAC");
        assert_eq!(hires.short(), "FLAC 24/96");
        assert_eq!(hires.compact(), "HI-RES");
        assert_eq!(hires.delivered(), "24-bit · 96 kHz");
        let cd = Format::parse("16-bit/44.1 kHz FLAC");
        assert_eq!(cd.short(), "FLAC 16/44.1");
        assert_eq!(cd.compact(), "LOSSLESS");
        let aac = Format::parse("AAC 320 kbit/s");
        assert_eq!(aac.short(), "AAC 320");
        assert_eq!(aac.compact(), "AAC");
        assert_eq!(aac.quality(), "High");
    }

    #[test]
    fn spoken() {
        assert_eq!(spoken_duration(121), "2 minutes 1 second");
        assert_eq!(spoken_duration(0), "0 seconds");
        assert_eq!(spoken_duration(3600), "1 hour");
    }
}
