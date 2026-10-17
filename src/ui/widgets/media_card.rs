//! Cards for albums, playlists, mixes, artists and links (docs/design.md
//! §4.2), and the cover/avatar widgets they and the page headers use.
//!
//! A card is a `GtkOverlay` with two sibling buttons (never nested, so focus
//! and screen readers see two controls): the flat card button that opens the
//! item, and a round play button over the cover's corner that plays it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::covers::{CoverLoader, Image, Ticket};
use crate::i18n::tr;
use crate::tidal::pages::{Item, Target};
use crate::ui::context::Ctx;
use crate::ui::format;
use crate::ui::menus::{self, ItemMenu, PlayMode, Subject};

/// Logical size of a card's cover (144 in narrow windows is left to CSS
/// scaling; covers are downloaded for 160).
pub const CARD_SIZE: i32 = 160;
/// Cover size in narrow windows (docs/design.md §1.5).
const NARROW_CARD_SIZE: i32 = 144;
/// Padding of `.media-card-button` (the play button is placed from it).
const CARD_PADDING: i32 = 6;
const PLAY_SIZE: i32 = 40;

/// The artwork of an item, if it has one.
pub fn item_image(item: &Item) -> Option<Image> {
    match item {
        Item::Track(track) => track.album.cover.clone().map(Image::Cover),
        Item::Album(album) => album.cover.clone().map(Image::Cover),
        Item::Artist(artist) => artist.picture.clone().map(Image::Artist).or_else(|| {
            artist
                .selected_album_cover_fallback
                .clone()
                .map(Image::Cover)
        }),
        Item::Playlist(playlist) => playlist.square_image.clone().map(Image::Playlist),
        Item::Mix(mix) => mix.image.clone().map(Image::Url),
        // Promotions and page links use landscape images.
        Item::Link(link) => link.image.as_ref().map(|id| {
            Image::Url(format!(
                "https://resources.tidal.com/images/{}/550x400.jpg",
                id.replace('-', "/")
            ))
        }),
    }
}

fn placeholder_icon(item: &Item) -> &'static str {
    match item {
        Item::Playlist(_) | Item::Mix(_) => "view-list-symbolic",
        Item::Link(link) => match &link.target {
            Target::Collection(view) => match view.as_str() {
                "albums" => "media-optical-symbolic",
                "artists" => "avatar-default-symbolic",
                "playlists" | "mixes" => "view-list-symbolic",
                _ => "swell-heart-filled-symbolic",
            },
            _ => "folder-music-symbolic",
        },
        _ => "audio-x-generic-symbolic",
    }
}

/// CSS class of the scrolled window of a carousel: covers inside it load
/// only when their card is near the visible part.
pub const CAROUSEL_SCROLLER: &str = "carousel-scroller";

/// The download of one image for one widget (see [`load_when_shown`]).
struct ShownLoader {
    covers: Weak<CoverLoader>,
    image: Image,
    size: i32,
    set: Box<dyn Fn(&gdk::Texture)>,
    loaded: Cell<bool>,
    ticket: RefCell<Option<Ticket>>,
    /// Scroll positions (of the carousel, of the page) watched until the
    /// widget comes near the visible part.
    watch: RefCell<Vec<(gtk::Adjustment, [glib::SignalHandlerId; 2])>>,
}

impl ShownLoader {
    fn mapped(self: &Rc<Self>, widget: &gtk::Widget) {
        if self.loaded.get() {
            return;
        }
        let (carousel, page) = scrollers(widget);
        if carousel.is_none() && page.is_none() {
            return self.request(widget);
        }
        let check = {
            let this = Rc::downgrade(self);
            let widget = widget.downgrade();
            let carousel = carousel.as_ref().map(|s| s.downgrade());
            let page = page.as_ref().map(|s| s.downgrade());
            move |_: &gtk::Adjustment| {
                let (Some(this), Some(widget)) = (this.upgrade(), widget.upgrade()) else {
                    return;
                };
                let carousel = carousel.as_ref().and_then(|s| s.upgrade());
                let page = page.as_ref().and_then(|s| s.upgrade());
                if near(&widget, carousel.as_ref(), page.as_ref()) {
                    this.unwatch();
                    this.request(&widget);
                }
            }
        };
        self.unwatch();
        let adjustments = carousel
            .iter()
            .map(|s| s.hadjustment())
            .chain(page.iter().map(|s| s.vadjustment()));
        for adjustment in adjustments {
            let ids = [
                adjustment.connect_value_changed(check.clone()),
                adjustment.connect_changed(check.clone()),
            ];
            self.watch.borrow_mut().push((adjustment, ids));
        }
        // Already laid out (shown again): decide now. Otherwise the first
        // frame after mapping has the allocation, even when no adjustment
        // changes afterwards (a page drawn from the cache in one go).
        let first = self.watch.borrow().first().map(|(a, _)| a.clone());
        if let Some(adjustment) = first {
            check(&adjustment);
            // Ticks run before layout: wait for the first frame that has
            // allocated the widget.
            widget.add_tick_callback(move |widget, _| {
                if widget.width() <= 0 {
                    return glib::ControlFlow::Continue;
                }
                check(&adjustment);
                glib::ControlFlow::Break
            });
        }
    }

    fn request(self: &Rc<Self>, widget: &gtk::Widget) {
        if self.loaded.get() || self.ticket.borrow().is_some() {
            return;
        }
        let Some(covers) = self.covers.upgrade() else {
            return;
        };
        let pixels = super::device_pixels(widget, self.size);
        let this = Rc::downgrade(self);
        let ticket = covers.load(&self.image, pixels, 0, move |texture| {
            if let Some(this) = this.upgrade() {
                this.loaded.set(true);
                (this.set)(texture);
            }
        });
        if !self.loaded.get() {
            self.ticket.replace(ticket);
        }
    }

    /// Hidden (another page on top, or the card scrolled away with its
    /// page): stop downloading for it; mapping it again asks again.
    fn unmapped(&self) {
        self.unwatch();
        self.ticket.take();
    }

    fn unwatch(&self) {
        for (adjustment, ids) in self.watch.take() {
            for id in ids {
                adjustment.disconnect(id);
            }
        }
    }
}

/// The carousel (horizontal) and page (vertical) scrolled windows around
/// `widget`, if any.
fn scrollers(widget: &gtk::Widget) -> (Option<gtk::ScrolledWindow>, Option<gtk::ScrolledWindow>) {
    let mut carousel = None;
    let mut at = widget.ancestor(gtk::ScrolledWindow::static_type());
    while let Some(scrolled) = at.and_downcast::<gtk::ScrolledWindow>() {
        if scrolled.has_css_class(CAROUSEL_SCROLLER) {
            carousel.get_or_insert(scrolled.clone());
        } else if scrolled.vscrollbar_policy() != gtk::PolicyType::Never {
            return (carousel, Some(scrolled));
        }
        at = scrolled
            .parent()
            .and_then(|parent| parent.ancestor(gtk::ScrolledWindow::static_type()));
    }
    (carousel, None)
}

/// Whether `widget` is visible, or about to be (within half a screen), in
/// `carousel` (across) and `page` (down).
fn near(
    widget: &gtk::Widget,
    carousel: Option<&gtk::ScrolledWindow>,
    page: Option<&gtk::ScrolledWindow>,
) -> bool {
    if widget.width() <= 0 {
        return false;
    }
    let within = |scrolled: &gtk::ScrolledWindow, across: bool| {
        let Some(point) = widget.compute_point(scrolled, &gtk::graphene::Point::new(0.0, 0.0))
        else {
            return false;
        };
        let (at, size, room) = if across {
            (point.x(), widget.width(), scrolled.width())
        } else {
            (point.y(), widget.height(), scrolled.height())
        };
        let (at, size, room) = (f64::from(at), f64::from(size), f64::from(room));
        room > 0.0 && at + size >= -room / 2.0 && at <= room * 1.5
    };
    carousel.is_none_or(|s| within(s, true)) && page.is_none_or(|s| within(s, false))
}

/// Loads `image` into `set` while `widget` is mapped and on screen or
/// about to be (scrolled pages and carousels build more than they show).
/// Unmapping cancels a download in progress; once loaded, the image stays.
pub fn load_when_shown(
    ctx: &Rc<Ctx>,
    widget: &impl IsA<gtk::Widget>,
    image: Image,
    size: i32,
    set: impl Fn(&gdk::Texture) + 'static,
) {
    let loader = Rc::new(ShownLoader {
        covers: Rc::downgrade(&ctx.covers),
        image,
        size,
        set: Box::new(set),
        loaded: Cell::new(false),
        ticket: RefCell::default(),
        watch: RefCell::default(),
    });
    let widget = widget.upcast_ref::<gtk::Widget>();
    // The widget's handlers own the loader; it refers to the widget weakly.
    widget.connect_map({
        let loader = loader.clone();
        move |widget| loader.mapped(widget)
    });
    widget.connect_unmap({
        let loader = loader.clone();
        move |_| loader.unmapped()
    });
    widget.connect_destroy(move |_| loader.unmapped());
}

/// A square cover with a placeholder icon while it loads. `large` gives it
/// the 12 px radius of cards and headers.
pub fn cover(
    ctx: &Rc<Ctx>,
    image: Option<Image>,
    size: i32,
    placeholder: &str,
    large: bool,
) -> gtk::Widget {
    Cover::new(ctx, image, size, placeholder, large)
        .widget()
        .clone()
}

/// A cover whose displayed size can change (page headers shrink in narrow
/// windows); it is downloaded once, for the size it was created with.
pub struct Cover {
    frame: gtk::Overlay,
    icon: gtk::Image,
    picture: gtk::Image,
}

impl Cover {
    pub fn new(
        ctx: &Rc<Ctx>,
        image: Option<Image>,
        size: i32,
        placeholder: &str,
        large: bool,
    ) -> Self {
        let frame = gtk::Overlay::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["cover", "cover-placeholder"])
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        if large {
            frame.add_css_class("large");
        }
        let icon = gtk::Image::builder()
            .icon_name(placeholder)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["dimmed"])
            .build();
        frame.set_child(Some(&icon));
        let picture = gtk::Image::new();
        frame.add_overlay(&picture);
        if let Some(image) = image {
            let weak = picture.downgrade();
            load_when_shown(ctx, &frame, image, size, move |texture| {
                if let Some(picture) = weak.upgrade() {
                    picture.set_paintable(Some(texture));
                }
            });
        }
        let cover = Self {
            frame,
            icon,
            picture,
        };
        cover.set_size(size);
        cover
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.frame.upcast_ref()
    }

    pub fn set_size(&self, size: i32) {
        self.frame.set_size_request(size, size);
        self.icon.set_pixel_size((size / 4).clamp(16, 64));
        self.picture.set_pixel_size(size);
    }

    /// A handle that does not keep the cover's widgets alive (for closures
    /// stored in them).
    fn downgrade(&self) -> WeakCover {
        WeakCover {
            frame: self.frame.downgrade(),
            icon: self.icon.downgrade(),
            picture: self.picture.downgrade(),
        }
    }
}

struct WeakCover {
    frame: glib::WeakRef<gtk::Overlay>,
    icon: glib::WeakRef<gtk::Image>,
    picture: glib::WeakRef<gtk::Image>,
}

impl WeakCover {
    fn upgrade(&self) -> Option<Cover> {
        Some(Cover {
            frame: self.frame.upgrade()?,
            icon: self.icon.upgrade()?,
            picture: self.picture.upgrade()?,
        })
    }
}

/// A round artist picture with initials while it loads.
pub fn avatar(ctx: &Rc<Ctx>, image: Option<Image>, name: &str, size: i32) -> adw::Avatar {
    let avatar = adw::Avatar::new(size, Some(name), true);
    avatar.set_halign(gtk::Align::Center);
    avatar.set_valign(gtk::Align::Start);
    if let Some(image) = image {
        let weak = avatar.downgrade();
        load_when_shown(ctx, &avatar, image, size, move |texture| {
            if let Some(avatar) = weak.upgrade() {
                avatar.set_custom_image(Some(texture));
            }
        });
    }
    avatar
}

/// Title, subtitle and accessible name of a card.
fn describe(item: &Item) -> (String, String, String) {
    let year = |date: &Option<String>| date.as_deref().and_then(|d| d.get(..4)).map(str::to_owned);
    match item {
        Item::Track(track) => {
            let artists = track
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            (
                track.title.clone(),
                artists.clone(),
                tr("Track: {title}, by {artists}")
                    .replace("{title}", &track.title)
                    .replace("{artists}", &artists),
            )
        }
        Item::Album(album) => {
            let artist = album
                .artists
                .first()
                .map(|a| a.name.clone())
                .unwrap_or_default();
            let subtitle = match year(&album.release_date) {
                Some(year) if !artist.is_empty() => format!("{artist} · {year}"),
                Some(year) => year,
                None => artist.clone(),
            };
            (
                album.title.clone(),
                subtitle,
                tr("Album: {title}, by {artist}")
                    .replace("{title}", &album.title)
                    .replace("{artist}", &artist),
            )
        }
        Item::Artist(artist) => (
            artist.name.clone(),
            tr("Artist").into(),
            tr("Artist: {name}").replace("{name}", &artist.name),
        ),
        Item::Playlist(playlist) => {
            let subtitle = match playlist.creator.name.as_deref().filter(|n| !n.is_empty()) {
                Some(owner) => tr("by {name}").replace("{name}", owner),
                None if playlist.number_of_tracks > 0 => format::counted(
                    playlist.number_of_tracks as usize,
                    "{n} track",
                    "{n} tracks",
                ),
                None => tr("Playlist").into(),
            };
            (
                playlist.title.clone(),
                subtitle.clone(),
                tr("Playlist: {title}, {details}")
                    .replace("{title}", &playlist.title)
                    .replace("{details}", &subtitle),
            )
        }
        Item::Mix(mix) => (
            mix.title.clone(),
            mix.subtitle.clone(),
            tr("Mix: {title}, {details}")
                .replace("{title}", &mix.title)
                .replace("{details}", &mix.subtitle),
        ),
        Item::Link(link) => (
            link.title.clone(),
            link.subtitle.clone().unwrap_or_default(),
            link.title.clone(),
        ),
    }
}

/// Opens the menu of `subject` from right clicks, long presses, the Menu
/// key and Shift+F10 on `widget`.
pub fn attach_menu(widget: &impl IsA<gtk::Widget>, menu: &Rc<ItemMenu>, subject: Subject) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    let subject = Rc::new(subject);
    let open = {
        let menu = Rc::downgrade(menu);
        let widget = widget.downgrade();
        let subject = subject.clone();
        move || {
            if let (Some(menu), Some(widget)) = (menu.upgrade(), widget.upgrade()) {
                menu.popup_at((*subject).clone(), &widget);
            }
        }
    };
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    click.connect_pressed({
        let open = open.clone();
        move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open();
        }
    });
    widget.add_controller(click);
    let press = gtk::GestureLongPress::builder().touch_only(true).build();
    press.connect_pressed({
        let open = open.clone();
        move |gesture, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            open();
        }
    });
    widget.add_controller(press);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let modifiers = modifiers & gtk::accelerator_get_default_mod_mask();
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && modifiers == gdk::ModifierType::SHIFT_MASK)
        {
            open();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    widget.add_controller(keys);
}

/// One card. Build it with [`MediaCard::new`] and put
/// [`MediaCard::widget`] in a carousel or grid.
pub struct MediaCard {
    widget: gtk::Overlay,
}

impl MediaCard {
    /// A card for `item`; `menu` (one per carousel/grid) gives it a context
    /// menu.
    pub fn new(ctx: &Rc<Ctx>, item: &Item, menu: Option<&Rc<ItemMenu>>) -> Self {
        let size = CARD_SIZE;
        let (title, subtitle, accessible) = describe(item);
        let round = matches!(item, Item::Artist(_));

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        // The art, and how to resize it (narrow windows show 144 px cards).
        let (art, resize): (gtk::Widget, Box<dyn Fn(i32)>) = if round {
            let avatar = avatar(ctx, item_image(item), &title, size);
            let weak = avatar.downgrade();
            let resize = move |size| {
                if let Some(avatar) = weak.upgrade() {
                    avatar.set_size(size);
                }
            };
            (avatar.upcast(), Box::new(resize))
        } else {
            let cover = Cover::new(ctx, item_image(item), size, placeholder_icon(item), true);
            // At the card's start, where its title starts.
            cover.widget().set_halign(gtk::Align::Start);
            let weak = cover.downgrade();
            let resize = move |size| {
                if let Some(cover) = weak.upgrade() {
                    cover.set_size(size);
                }
            };
            (cover.widget().clone(), Box::new(resize))
        };
        content.append(&art);
        let xalign = if round { 0.5 } else { 0.0 };
        let title_label = gtk::Label::builder()
            .label(&title)
            .xalign(xalign)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .max_width_chars(1)
            .tooltip_text(&title)
            .css_classes(["heading"])
            .build();
        content.append(&title_label);
        if !subtitle.is_empty() {
            content.append(
                &gtk::Label::builder()
                    .label(&subtitle)
                    .xalign(xalign)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .single_line_mode(true)
                    .max_width_chars(1)
                    .css_classes(["caption", "dimmed"])
                    .build(),
            );
        }

        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat", "media-card-button"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&accessible)]);
        let widget = gtk::Overlay::builder()
            .child(&button)
            .valign(gtk::Align::Start)
            .css_classes(["media-card"])
            .build();

        let weak_ctx = Rc::downgrade(ctx);
        let open = item.clone();
        button.connect_clicked(move |_| {
            if let Some(ctx) = weak_ctx.upgrade() {
                super::section::activate_item(&ctx, &open);
            }
        });

        let mut play_button = None;
        if let Some(subject) = Subject::from_item(item) {
            if !matches!(subject, Subject::Track { .. }) {
                let play = gtk::Button::builder()
                    .icon_name("media-playback-start-symbolic")
                    .width_request(PLAY_SIZE)
                    .height_request(PLAY_SIZE)
                    .halign(gtk::Align::Start)
                    .valign(gtk::Align::Start)
                    .css_classes(["circular", "suggested-action", "media-card-play"])
                    .build();
                super::label_button(&play, &tr("Play {title}").replace("{title}", &title));
                let weak_ctx = Rc::downgrade(ctx);
                let to_play = subject.clone();
                play.connect_clicked(move |_| {
                    if let Some(ctx) = weak_ctx.upgrade() {
                        menus::play(&ctx, &to_play, PlayMode::Play);
                    }
                });
                widget.add_overlay(&play);
                play_button = Some(play);
            }
            add_drag_source(&button, &subject, &art);
            if let Some(menu) = menu {
                attach_menu(&button, menu, subject);
            }
        }
        // Card, art and the play button over the art's corner follow the
        // window width.
        let layout = {
            let card = widget.downgrade();
            let play = play_button.map(|play| play.downgrade());
            move |narrow: bool| {
                let size = if narrow { NARROW_CARD_SIZE } else { CARD_SIZE };
                resize(size);
                if let Some(card) = card.upgrade() {
                    card.set_width_request(size + 2 * CARD_PADDING);
                }
                if let Some(play) = play.as_ref().and_then(|play| play.upgrade()) {
                    let at = CARD_PADDING + size - PLAY_SIZE - 6;
                    play.set_margin_start(at);
                    play.set_margin_top(at);
                }
            }
        };
        layout(false);
        super::follow_narrow(&widget, layout);
        Self { widget }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.widget.upcast_ref()
    }
}

/// Dragging a card offers its TIDAL link (`text/uri-list` and text).
fn add_drag_source(widget: &impl IsA<gtk::Widget>, subject: &Subject, art: &gtk::Widget) {
    let url = subject.share_url();
    let source = gtk::DragSource::builder()
        .actions(gdk::DragAction::COPY)
        .build();
    let art = art.downgrade();
    source.connect_prepare(move |source, _, _| {
        if let Some(art) = art.upgrade() {
            source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&art))), 0, 0);
        }
        Some(gdk::ContentProvider::new_union(&[
            gdk::ContentProvider::for_bytes(
                super::track_list::TRACK_URI_MIME,
                &glib::Bytes::from_owned(format!("{url}\r\n")),
            ),
            gdk::ContentProvider::for_value(&url.to_value()),
        ]))
    });
    widget.add_controller(source);
}
