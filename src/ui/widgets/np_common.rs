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
use super::quality_badge::{BadgeKind, QualityBadge};
use crate::covers::{CoverLoader, Image, Ticket};
use crate::i18n::{tr, trn};
use crate::library::{Change, Item, Library};
use crate::model::TrackInfo;
use crate::playback::format::khz;
use crate::playback::{Event, FormatDetails, PlayState, Playback, SleepTimer};
use crate::session::LoginFlow;
use crate::settings::{Normalization, QualityPreference};
use crate::tidal::Quality;
use crate::ui::dialogs;

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

    pub fn timer() -> &'static str {
        icon(&["swell-timer-symbolic", "alarm-symbolic"])
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
    QualityBadge::new(BadgeKind::Explicit)
}

/// The available quality of the track: HI-RES or LOSSLESS (§4.5), one of
/// two shared badges shown at a time.
pub(super) struct TrackQuality {
    pub widget: gtk::Box,
    hires: gtk::Label,
    lossless: gtk::Label,
}

impl TrackQuality {
    pub fn new() -> Self {
        let hires = QualityBadge::new(BadgeKind::HiRes);
        let lossless = QualityBadge::new(BadgeKind::Lossless);
        let widget = gtk::Box::builder().valign(gtk::Align::Center).build();
        widget.append(&hires);
        widget.append(&lossless);
        Self {
            widget,
            hires,
            lossless,
        }
    }

    pub fn set(&self, hires: bool) {
        self.hires.set_visible(hires);
        self.lossless.set_visible(!hires);
    }
}

/// "Hi-Res Lossless", "Lossless", "High"...
fn quality_name(details: &FormatDetails) -> &'static str {
    match details.quality {
        Some(Quality::HiResLossless) => tr("Hi-Res Lossless"),
        Some(Quality::Lossless) => tr("Lossless"),
        Some(Quality::High) => tr("High"),
        Some(Quality::Low) => tr("Low"),
        None if details.is_hires() => tr("Hi-Res Lossless"),
        None if details.is_lossless() => tr("Lossless"),
        None => tr("Lossy"),
    }
}

/// The badge text in compact layouts: "HI-RES", "LOSSLESS" or the codec.
fn compact_label(details: &FormatDetails) -> String {
    if details.is_hires() {
        "HI-RES".into()
    } else if details.is_lossless() {
        "LOSSLESS".into()
    } else {
        details.codec.name().to_uppercase()
    }
}

/// Why the output is not bit-perfect, if it is not: the samples reach the
/// sound server untouched only without normalisation, at full volume and
/// unmuted.
fn alterations(details: &FormatDetails, volume: f64, muted: bool) -> Vec<String> {
    let mut reasons = Vec::new();
    if details.replay_gain_db.is_some() {
        reasons.push(tr("normalisation is on").to_owned());
    }
    if muted {
        reasons.push(tr("muted").to_owned());
    } else if volume < 0.999 {
        reasons.push(
            tr("volume at {percent} %").replace("{percent}", &format!("{:.0}", volume * 100.0)),
        );
    }
    if !details.is_lossless() {
        reasons.push(tr("lossy stream").to_owned());
    }
    reasons
}

/// "−3.2 dB" with a real minus sign.
fn decibels(db: f64) -> String {
    let text = format!("{db:+.1} dB");
    text.replace('-', "−")
}

/// The delivered-format badge: a flat menu button ("FLAC 24/96") whose
/// popover lists codec, bit depth, sample rate, bitrate, the TIDAL tier,
/// the ReplayGain applied and whether the output is bit-perfect (§4.5).
/// Follows playback by itself.
pub(super) struct FormatBadge {
    pub widget: gtk::MenuButton,
    ctx: Weak<Ctx>,
    label: gtk::Label,
    /// Key and value label per row, in [`FormatBadge::row_names`] order.
    rows: Vec<(gtk::Label, gtk::Label)>,
    note: gtk::Label,
    compact: Cell<bool>,
    /// Show the expected quality until the format is known.
    expected: Cell<bool>,
    _events: Subscription,
}

impl FormatBadge {
    const ROWS: usize = 8;

    fn row_names() -> [&'static str; Self::ROWS] {
        [
            tr("Quality"),
            tr("Codec"),
            tr("Bit depth"),
            tr("Sample rate"),
            tr("Bitrate"),
            tr("ReplayGain"),
            tr("Output"),
            tr("Output device"),
        ]
    }

    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let label = gtk::Label::builder()
            .css_classes(["badge", "numeric"])
            .build();
        let widget = gtk::MenuButton::builder()
            .child(&label)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "format-badge"])
            .tooltip_text(tr("Audio Format"))
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
        let rows: Vec<(gtk::Label, gtk::Label)> = Self::row_names()
            .iter()
            .enumerate()
            .map(|(row, name)| {
                let key = gtk::Label::builder()
                    .label(*name)
                    .xalign(0.0)
                    .css_classes(["dimmed"])
                    .build();
                let value = gtk::Label::builder()
                    .xalign(0.0)
                    .wrap(true)
                    .max_width_chars(28)
                    .selectable(true)
                    .css_classes(["numeric"])
                    .build();
                grid.attach(&key, 0, row as i32, 1, 1);
                grid.attach(&value, 1, row as i32, 1, 1);
                (key, value)
            })
            .collect();
        let note = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(36)
            .css_classes(["caption", "dimmed"])
            .visible(false)
            .build();
        grid.attach(&note, 0, Self::ROWS as i32, 2, 1);
        let popover = gtk::Popover::builder().child(&grid).build();
        widget.set_popover(Some(&popover));
        let badge = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget,
            ctx: Rc::downgrade(ctx),
            label,
            rows,
            note,
            compact: Cell::new(false),
            expected: Cell::new(false),
            _events: subscribe(&ctx.playback, weak, |badge, event| {
                if matches!(event, Event::Track(_) | Event::Format(_) | Event::Options) {
                    badge.refresh();
                }
            }),
        });
        badge.refresh();
        badge
    }

    pub fn set_compact(&self, compact: bool) {
        self.compact.set(compact);
        self.refresh();
    }

    fn set_row(&self, row: usize, value: Option<&str>) {
        let (key, label) = &self.rows[row];
        key.set_visible(value.is_some());
        label.set_visible(value.is_some());
        label.set_label(value.unwrap_or_default());
    }

    /// Shows what plays now; hidden until its stream is resolved.
    fn refresh(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let details = ctx.playback.format_details();
        let Some(details) = details.filter(|_| ctx.playback.current().is_some()) else {
            self.show_expected(&ctx);
            return;
        };
        let settings = ctx.settings.get();
        let alterations = alterations(&details, settings.volume, settings.muted);
        let output = if alterations.is_empty() {
            tr("Bit-perfect").to_owned()
        } else {
            tr("Not bit-perfect: {reasons}").replace("{reasons}", &alterations.join(", "))
        };
        let replay_gain = details.replay_gain_db.map(|db| {
            let text = match settings.normalization {
                Normalization::Album => tr("{gain} (album)"),
                _ => tr("{gain} (track)"),
            };
            text.replace("{gain}", &decibels(db))
        });
        self.set_row(0, Some(quality_name(&details)));
        self.set_row(1, Some(details.codec.name()));
        self.set_row(
            2,
            details
                .bit_depth
                .map(|b| tr("{n}-bit").replace("{n}", &b.to_string()))
                .as_deref(),
        );
        self.set_row(
            3,
            details
                .sample_rate
                .map(|r| format!("{} kHz", khz(r)))
                .as_deref(),
        );
        self.set_row(4, details.bitrate.map(|b| format!("{b} kbit/s")).as_deref());
        self.set_row(5, Some(replay_gain.as_deref().unwrap_or(tr("Off"))));
        self.set_row(6, Some(&output));
        self.set_row(
            7,
            Some(
                settings
                    .output_device
                    .as_ref()
                    .map_or(tr("System default"), String::as_str),
            ),
        );

        let track_hires = ctx.playback.current().is_some_and(|t| t.hires);
        let note = (track_hires && !details.is_hires()).then_some(match settings.quality {
            QualityPreference::Best => tr("Lossless delivered: this track has no Hi-Res master"),
            _ => tr("Lossless delivered: Hi-Res is turned off in Preferences"),
        });
        self.note.set_label(note.unwrap_or_default());
        self.note.set_visible(note.is_some());

        if details.is_hires() {
            self.label.add_css_class("hires");
        } else {
            self.label.remove_css_class("hires");
        }
        self.label.set_label(&if self.compact.get() {
            compact_label(&details)
        } else {
            details.short()
        });
        let spoken = tr("Audio format: {quality}, {format}. {output}")
            .replace("{quality}", quality_name(&details))
            .replace("{format}", &details.describe())
            .replace("{output}", &output);
        self.widget
            .update_property(&[gtk::accessible::Property::Label(&spoken)]);
        self.widget.set_visible(true);
    }

    /// Before the stream is resolved (a restored, paused track): with
    /// [`FormatBadge::set_expected`], the quality the track should play in,
    /// from its catalogue data and the settings; hidden otherwise.
    fn show_expected(&self, ctx: &Ctx) {
        let Some(track) = self
            .expected
            .get()
            .then(|| ctx.playback.current())
            .flatten()
        else {
            self.widget.set_visible(false);
            return;
        };
        let settings = ctx.settings.get();
        let hires = track.hires
            && settings.quality == QualityPreference::Best
            && ctx.api.flow == LoginFlow::Pkce;
        let lossless = matches!(
            settings.quality,
            QualityPreference::Best | QualityPreference::Lossless
        );
        let kind = if hires {
            BadgeKind::HiRes
        } else if lossless {
            BadgeKind::Lossless
        } else {
            self.widget.set_visible(false);
            return;
        };
        for row in 1..Self::ROWS {
            self.set_row(row, None);
        }
        self.set_row(0, Some(kind.description()));
        self.note
            .set_label(tr("The exact format shows once the track starts playing."));
        self.note.set_visible(true);
        if hires {
            self.label.add_css_class("hires");
        } else {
            self.label.remove_css_class("hires");
        }
        self.label
            .set_label(if hires { "HI-RES" } else { "LOSSLESS" });
        self.widget
            .update_property(&[gtk::accessible::Property::Label(
                &tr("Audio quality: {quality}").replace("{quality}", kind.description()),
            )]);
        self.widget.set_visible(true);
    }

    /// Shows the track's expected quality until the delivered format is
    /// known (the player bar, which has no other quality badge).
    pub fn set_expected(&self, expected: bool) {
        self.expected.set(expected);
        self.refresh();
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
            .tooltip_text(tr("Add to Favourites"))
            .sensitive(false)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(tr("Favourite"))]);
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
                            ctx.report(tr("Couldn't update favourites"), &err);
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
            tr("Remove from Favourites")
        } else {
            tr("Add to Favourites")
        }));
    }
}

/// The sleep timer choices (§4.9): action target and menu label.
fn sleep_choices() -> [(&'static str, &'static str); 6] {
    [
        ("off", tr("Off")),
        ("15", tr("15 Minutes")),
        ("30", tr("30 Minutes")),
        ("45", tr("45 Minutes")),
        ("60", tr("60 Minutes")),
        ("end", tr("End of Track")),
    ]
}

/// The `player.sleep` state for a timer.
fn sleep_target(timer: Option<SleepTimer>) -> String {
    match timer {
        None => "off".into(),
        Some(SleepTimer::EndOfTrack) => "end".into(),
        Some(SleepTimer::After(duration)) => (duration.as_secs() / 60).to_string(),
    }
}

fn sleep_timer_of(target: &str) -> Option<SleepTimer> {
    match target {
        "end" => Some(SleepTimer::EndOfTrack),
        minutes => minutes
            .parse::<u64>()
            .ok()
            .filter(|m| *m > 0)
            .map(|m| SleepTimer::After(Duration::from_secs(m * 60))),
    }
}

/// The "Sleep Timer" choices, as radio items of `player.sleep`.
fn sleep_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (target, label) in sleep_choices() {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("player.sleep"), Some(&target.to_variant()));
        menu.append_item(&item);
    }
    menu
}

/// Actions on the current track and the player ("player.*"): go to
/// album/artist, track radio, add to playlist, copy link and the sleep
/// timer. Insert [`PlayerActions::group`] on a widget that contains the
/// player's menus.
pub(super) struct PlayerActions {
    pub group: gio::SimpleActionGroup,
    _events: Subscription,
}

impl PlayerActions {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let group = gio::SimpleActionGroup::new();
        let add = |name: &str, run: fn(&Rc<Ctx>, &TrackInfo)| {
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
        add("add-to-playlist", |ctx, track| {
            dialogs::playlist::choose_playlist(ctx, vec![track.id]);
        });
        add("copy-link", |ctx, track| {
            ctx.copy(&track.share_url(), tr("Link copied"))
        });

        let sleep = gio::SimpleAction::new_stateful(
            "sleep",
            Some(glib::VariantTy::STRING),
            &sleep_target(ctx.playback.sleep_timer()).to_variant(),
        );
        sleep.connect_activate({
            let playback = Rc::downgrade(&ctx.playback);
            move |_, target| {
                if let (Some(playback), Some(target)) =
                    (playback.upgrade(), target.and_then(|t| t.str()))
                {
                    playback.set_sleep_timer(sleep_timer_of(target));
                }
            }
        });
        group.add_action(&sleep);
        let playback = Rc::downgrade(&ctx.playback);
        Rc::new_cyclic(|weak: &Weak<Self>| Self {
            group,
            _events: subscribe(&ctx.playback, weak, move |_, event| {
                if let (Event::SleepTimer, Some(playback)) = (event, playback.upgrade()) {
                    sleep.set_state(&sleep_target(playback.sleep_timer()).to_variant());
                }
            }),
        })
    }
}

/// The "⋯" menu of the player (§4.9): sleep timer, go to album/artist,
/// track radio, add to playlist, copy link. Uses [`PlayerActions`].
pub(super) fn track_menu_button() -> gtk::MenuButton {
    let menu = gio::Menu::new();
    let timer = gio::Menu::new();
    timer.append_submenu(Some(tr("Sleep Timer")), &sleep_menu());
    menu.append_section(None, &timer);
    let navigate = gio::Menu::new();
    navigate.append(Some(tr("Go to Album")), Some("player.album"));
    navigate.append(Some(tr("Go to Artist")), Some("player.artist"));
    navigate.append(Some(tr("Track Radio")), Some("player.radio"));
    menu.append_section(None, &navigate);
    let share = gio::Menu::new();
    share.append(Some(tr("Add to Playlist…")), Some("player.add-to-playlist"));
    share.append(Some(tr("Copy Link")), Some("player.copy-link"));
    menu.append_section(None, &share);
    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&menu)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .tooltip_text(tr("More"))
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tr("More"))]);
    button
}

/// While the sleep timer runs: its icon and the minutes left ("23 min"),
/// a button that opens the timer choices (§4.9). Uses [`PlayerActions`].
pub(super) struct SleepIndicator {
    pub widget: gtk::MenuButton,
    playback: Rc<Playback>,
    label: gtk::Label,
    _events: Subscription,
}

impl SleepIndicator {
    pub fn new(playback: &Rc<Playback>) -> Rc<Self> {
        let label = gtk::Label::builder().css_classes(["numeric"]).build();
        let content = gtk::Box::builder().spacing(6).build();
        content.append(&gtk::Image::from_icon_name(icons::timer()));
        content.append(&label);
        let widget = gtk::MenuButton::builder()
            .child(&content)
            .menu_model(&sleep_menu())
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .visible(false)
            .build();
        let indicator = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget,
            playback: playback.clone(),
            label,
            _events: subscribe(playback, weak, |indicator, event| {
                if matches!(event, Event::SleepTimer | Event::Track(_)) {
                    indicator.refresh();
                }
            }),
        });
        indicator.refresh();
        indicator
    }

    fn refresh(&self) {
        let timer = self.playback.sleep_timer();
        self.widget.set_visible(timer.is_some());
        let (text, tooltip) = match timer {
            None => return,
            Some(SleepTimer::EndOfTrack) => (
                String::new(),
                tr("Sleep timer: stops at the end of this track").to_owned(),
            ),
            Some(SleepTimer::After(_)) => {
                let minutes = self
                    .playback
                    .sleep_timer_remaining()
                    .map_or(0, |left| left.as_secs().div_ceil(60));
                (
                    tr("{n} min").replace("{n}", &minutes.to_string()),
                    trn(
                        "Sleep timer: stops in {n} minute",
                        "Sleep timer: stops in {n} minutes",
                        minutes,
                    )
                    .replace("{n}", &minutes.to_string()),
                )
            }
        };
        self.label.set_label(&text);
        self.label.set_visible(!text.is_empty());
        self.widget.set_tooltip_text(Some(&tooltip));
        self.widget
            .update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    }
}

/// "2 minutes 1 second", for screen readers.
pub(super) fn spoken_duration(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    let unit = |n: u64, one: &'static str, many: &'static str| {
        trn(one, many, n).replace("{n}", &n.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_details() {
        use crate::playback::Codec;
        if crate::i18n::ui_language().is_some() {
            // Checks the English texts.
            return;
        }
        let mut details = FormatDetails {
            codec: Codec::Flac,
            bit_depth: Some(24),
            sample_rate: Some(96_000),
            bitrate: None,
            quality: Some(Quality::HiResLossless),
            audio_mode: None,
            replay_gain_db: None,
        };
        assert_eq!(details.short(), "FLAC 24/96");
        assert_eq!(compact_label(&details), "HI-RES");
        assert_eq!(quality_name(&details), "Hi-Res Lossless");
        assert!(alterations(&details, 1.0, false).is_empty());
        assert_eq!(alterations(&details, 0.8, false), ["volume at 80 %"]);
        details.replay_gain_db = Some(-3.21);
        assert_eq!(
            alterations(&details, 1.0, true),
            ["normalisation is on", "muted"]
        );
        assert_eq!(decibels(-3.21), "−3.2 dB");
        let aac = FormatDetails {
            codec: Codec::Aac,
            bit_depth: None,
            sample_rate: Some(44_100),
            bitrate: Some(320),
            quality: Some(Quality::High),
            audio_mode: None,
            replay_gain_db: None,
        };
        assert_eq!(compact_label(&aac), "AAC");
        assert_eq!(alterations(&aac, 1.0, false), ["lossy stream"]);
    }

    #[test]
    fn sleep_targets() {
        assert_eq!(sleep_target(None), "off");
        assert_eq!(sleep_timer_of("off"), None);
        assert_eq!(sleep_timer_of("end"), Some(SleepTimer::EndOfTrack));
        let half_hour = sleep_timer_of("30");
        assert_eq!(
            half_hour,
            Some(SleepTimer::After(Duration::from_secs(1800)))
        );
        assert_eq!(sleep_target(half_hour), "30");
    }

    #[test]
    fn spoken() {
        if crate::i18n::ui_language().is_some() {
            return;
        }
        assert_eq!(spoken_duration(121), "2 minutes 1 second");
        assert_eq!(spoken_duration(0), "0 seconds");
        assert_eq!(spoken_duration(3600), "1 hour");
    }
}
