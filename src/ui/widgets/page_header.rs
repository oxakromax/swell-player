//! Page headers (docs/design.md §4.4): artwork, a type overtitle, the
//! title, metadata, badges and the action row.
//!
//! | Variant | Artwork | Actions |
//! |---|---|---|
//! | Album | cover 240/180 | Play, Shuffle, ♥, Copy link, ⋯ |
//! | Playlist | cover 240/180 | Play, Shuffle, ♥ (or Edit when own), ⋯ |
//! | Artist | avatar 200/144 | Play, Shuffle, Follow, Radio, ⋯ |
//! | Mix | cover 240/180 | Play, Shuffle, ♥, Copy link, ⋯ |
//!
//! Wide: artwork beside the text. Compact (narrow page): stacked and
//! centred, smaller artwork, `title-2`.
//!
//! The action row stays on one line: when the text column gets too slim
//! for it, Shuffle loses its label, then Copy link goes (the menu has it);
//! only then does it wrap, the icon buttons moving down together.
//!
//! Titles: pages with this header (and the Home/Explore feeds, with their
//! large in-content title) keep the header bar's title hidden until the
//! in-content title scrolls away; list and utility pages (Collection
//! sub-views, Recently Played, View All, Search) have no in-content title
//! and show theirs, with a count subtitle, in the header bar.
//!
//! In short windows (the window's `short` class, ≤ 640 px high) the
//! artwork shrinks to 120 px so the list stays in view.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::heart_button::HeartButton;
use super::media_card::Cover;
use super::quality_badge::{self, QualityBadge};
use crate::covers::Image;
use crate::i18n::tr;
use crate::tidal::models::{Album, Artist, ArtistRef, Mix, Playlist};
use crate::ui::context::Ctx;
use crate::ui::dialogs;
use crate::ui::format;
use crate::ui::menus::{ItemMenu, Subject};
use crate::ui::nav::Route;

enum Art {
    Cover(Cover),
    Avatar(adw::Avatar),
}

impl Art {
    fn widget(&self) -> gtk::Widget {
        match self {
            Self::Cover(cover) => cover.widget().clone(),
            Self::Avatar(avatar) => avatar.clone().upcast(),
        }
    }

    /// 240/180 covers and 200/144 avatars (wide/compact, §1.5); 120 in
    /// short windows, where a large one would fill the screen.
    fn set_size(&self, compact: bool, short: bool) {
        match self {
            Self::Cover(cover) => cover.set_size(match (short, compact) {
                (true, _) => SHORT_ART,
                (false, true) => 180,
                (false, false) => 240,
            }),
            Self::Avatar(avatar) => avatar.set_size(match (short, compact) {
                (true, _) => SHORT_ART,
                (false, true) => 144,
                (false, false) => 200,
            }),
        }
    }
}

/// Artwork size in windows with the `short` class (≤ 640 px high).
const SHORT_ART: i32 = 120;

/// The artwork and what its size depends on.
struct ArtSize {
    art: Art,
    compact: Cell<bool>,
    short: Cell<bool>,
}

impl ArtSize {
    fn apply(&self) {
        self.art.set_size(self.compact.get(), self.short.get());
    }
}

/// Calls `f(has_class)` once `widget` is in a window, and whenever the
/// window gains or loses `class`.
fn follow_window_class(
    widget: &impl IsA<gtk::Widget>,
    class: &'static str,
    f: impl Fn(bool) + 'static,
) {
    let f = Rc::new(f);
    widget.connect_realize(move |widget| {
        let Some(window) = widget.root().and_downcast::<gtk::Window>() else {
            return;
        };
        f(window.has_css_class(class));
        let f = f.clone();
        let handler =
            window.connect_css_classes_notify(move |window| f(window.has_css_class(class)));
        let window = window.downgrade();
        let handler = RefCell::new(Some(handler));
        widget.connect_unrealize(move |_| {
            if let (Some(window), Some(handler)) = (window.upgrade(), handler.take()) {
                window.disconnect(handler);
            }
        });
    });
}

/// How much the action row has to give up to stay on one line.
struct ActionFit {
    /// The page is compact (narrow layout).
    compact: Cell<bool>,
    /// Natural width of the whole row, and of the row without Shuffle's
    /// label (0 until measured).
    full_width: Cell<i32>,
    without_label_width: Cell<i32>,
    /// The text column is too slim for Shuffle's label.
    tight: Cell<bool>,
    /// ...and for the Copy link button.
    tighter: Cell<bool>,
    shuffle_content: adw::ButtonContent,
    /// Hidden when compact or tighter (the menu has "Copy Link" too).
    copy_link: RefCell<Option<gtk::Button>>,
}

impl ActionFit {
    /// The row got `width` px.
    fn allocated(&self, width: i32) {
        if self.full_width.get() == 0 {
            return;
        }
        let tight = width < self.full_width.get();
        let tighter = width < self.without_label_width.get();
        if self.tight.replace(tight) != tight || self.tighter.replace(tighter) != tighter {
            self.update();
        }
    }

    fn update(&self) {
        let compact = self.compact.get();
        let label = !(compact || self.tight.get());
        self.shuffle_content
            .set_label(if label { tr("Shuffle") } else { "" });
        if let Some(copy) = self.copy_link.borrow().as_ref() {
            copy.set_visible(!(compact || self.tighter.get()));
        }
    }
}

mod imp {
    use std::cell::RefCell;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    type OnAllocate = Box<dyn Fn(i32)>;

    /// A single-child container that reports its width when allocated.
    #[derive(Default)]
    pub struct FitBin {
        pub on_allocate: RefCell<Option<OnAllocate>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FitBin {
        const NAME: &'static str = "SwellFitBin";
        type Type = super::FitBin;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for FitBin {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for FitBin {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.obj().first_child() {
                Some(child) => child.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(on_allocate) = self.on_allocate.borrow().as_ref() {
                on_allocate(width);
            }
            if let Some(child) = self.obj().first_child() {
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    /// A single-child container that reports its width when allocated.
    pub struct FitBin(ObjectSubclass<imp::FitBin>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FitBin {
    fn new(child: &impl IsA<gtk::Widget>, on_allocate: impl Fn(i32) + 'static) -> Self {
        let bin: Self = glib::Object::new();
        child.set_parent(&bin);
        bin.imp().on_allocate.replace(Some(Box::new(on_allocate)));
        bin
    }
}

pub struct PageHeader {
    widget: gtk::Box,
    art: Rc<ArtSize>,
    text: gtk::Box,
    labels: Vec<gtk::Label>,
    /// Labels inside detail boxes ("Read More" excerpts).
    details: Vec<gtk::Label>,
    title: gtk::Label,
    badges: gtk::Box,
    actions: adw::WrapBox,
    /// The icon buttons after Play and Shuffle: they wrap as one.
    extras: gtk::Box,
    /// Holds the action row and fits it to one line.
    actions_bin: FitBin,
    play: gtk::Button,
    shuffle: gtk::Button,
    fit: Rc<ActionFit>,
    menu: Rc<ItemMenu>,
}

impl PageHeader {
    fn build(ctx: &Rc<Ctx>, art: Art, overtitle: &str, title: &str) -> Self {
        let widget = gtk::Box::builder()
            .spacing(24)
            .margin_top(24)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        widget.append(&art.widget());
        let art = Rc::new(ArtSize {
            art,
            compact: Cell::new(false),
            short: Cell::new(false),
        });
        follow_window_class(&widget, "short", {
            let art = art.clone();
            move |short| {
                if art.short.replace(short) != short {
                    art.apply();
                }
            }
        });
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        widget.append(&text);

        let overtitle = gtk::Label::builder()
            .label(overtitle.to_uppercase())
            .xalign(0.0)
            .css_classes(["caption-heading", "dimmed"])
            .build();
        let title = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["title-1"])
            .accessible_role(gtk::AccessibleRole::Heading)
            .build();
        title.set_selectable(false);
        text.append(&overtitle);
        text.append(&title);

        let badges = gtk::Box::builder().spacing(6).visible(false).build();
        let actions = adw::WrapBox::builder()
            .child_spacing(12)
            .line_spacing(12)
            .margin_top(12)
            .build();
        let play = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("media-playback-start-symbolic")
                    .label(tr("Play"))
                    .build(),
            )
            .css_classes(["pill", "suggested-action"])
            .build();
        let shuffle_content = adw::ButtonContent::builder()
            .icon_name("media-playlist-shuffle-symbolic")
            .label(tr("Shuffle"))
            .build();
        let shuffle = gtk::Button::builder()
            .child(&shuffle_content)
            .tooltip_text(tr("Shuffle"))
            .css_classes(["pill"])
            .build();
        shuffle.update_property(&[gtk::accessible::Property::Label(tr("Shuffle"))]);
        actions.append(&play);
        actions.append(&shuffle);
        let extras = gtk::Box::builder().spacing(12).build();
        actions.append(&extras);
        let fit = Rc::new(ActionFit {
            compact: Cell::new(false),
            full_width: Cell::new(0),
            without_label_width: Cell::new(0),
            tight: Cell::new(false),
            tighter: Cell::new(false),
            shuffle_content,
            copy_link: RefCell::default(),
        });
        let actions_bin = FitBin::new(&actions, {
            let fit = Rc::downgrade(&fit);
            move |width| {
                if let Some(fit) = fit.upgrade() {
                    fit.allocated(width);
                }
            }
        });

        let menu = ItemMenu::new(ctx, &widget);
        Self {
            widget,
            art,
            text,
            labels: vec![overtitle],
            details: Vec::new(),
            title,
            badges,
            actions,
            extras,
            actions_bin,
            play,
            shuffle,
            fit,
            menu,
        }
    }

    /// Adds the badges row and the action row after the text lines added
    /// so far.
    fn finish(&self) {
        self.text.append(&self.badges);
        self.text.append(&self.actions_bin);
    }

    /// Once the row is complete: the widths where it stops fitting, with
    /// and without Shuffle's label.
    fn fit_actions(&self) {
        let natural = |widget: &adw::WrapBox| widget.measure(gtk::Orientation::Horizontal, -1).1;
        self.fit.full_width.set(natural(&self.actions));
        self.fit.shuffle_content.set_label("");
        let without_label = natural(&self.actions);
        self.fit.update();
        // Without Copy link there is nothing more to give up.
        self.fit
            .without_label_width
            .set(if self.fit.copy_link.borrow().is_some() {
                without_label
            } else {
                0
            });
    }

    fn add_line(&mut self, label: gtk::Label) {
        self.text.append(&label);
        self.labels.push(label);
    }

    fn add_dimmed(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dimmed"])
            .build();
        self.add_line(label);
    }

    /// Three dimmed lines of `text` and, only when they cut it short, a
    /// "Read More" button that opens all of it (as the artist biography
    /// does).
    fn add_read_more(&mut self, ctx: &Rc<Ctx>, text: &str, title: &str, subtitle: &str) {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(80)
            .css_classes(["dimmed"])
            .build();
        label.set_tooltip_text(Some(text));
        let more = gtk::Button::builder()
            .label(tr("Read More"))
            .halign(gtk::Align::Start)
            .visible(false)
            .css_classes(["flat"])
            .build();
        let weak = Rc::downgrade(ctx);
        let (text, title, subtitle) = (text.to_owned(), title.to_owned(), subtitle.to_owned());
        more.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                dialogs::credits::show_text(&ctx, &title, &subtitle, &text);
            }
        });
        // Whether the text is cut depends on the width: check after each
        // allocation (outside it: changing visibility queues a resize).
        let pending = Rc::new(Cell::new(false));
        let bin = FitBin::new(&label, {
            let label = label.downgrade();
            let more = more.downgrade();
            move |_| {
                if pending.replace(true) {
                    return;
                }
                let (label, more, pending) = (label.clone(), more.clone(), pending.clone());
                glib::idle_add_local_once(move || {
                    pending.set(false);
                    if let (Some(label), Some(more)) = (label.upgrade(), more.upgrade()) {
                        let cut = label.layout().is_ellipsized();
                        if more.is_visible() != cut {
                            more.set_visible(cut);
                        }
                    }
                });
            }
        });
        let detail = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        detail.append(&bin);
        detail.append(&more);
        self.text.append(&detail);
        self.details.push(label);
    }

    fn add_badges(&self, kinds: &[quality_badge::BadgeKind]) {
        for kind in kinds {
            self.badges.append(&QualityBadge::new(*kind));
        }
        self.badges.set_visible(!kinds.is_empty());
    }

    fn add_heart(&self, ctx: &Rc<Ctx>, subject: &Subject) {
        let heart = HeartButton::new(ctx);
        heart.set_item(Some(subject.favorite_item()));
        self.extras.append(heart.widget());
    }

    fn add_copy_link(&self, ctx: &Rc<Ctx>, url: String) {
        let button = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        super::label_button(&button, tr("Copy Link"));
        let ctx = Rc::downgrade(ctx);
        button.connect_clicked(move |_| {
            if let Some(ctx) = ctx.upgrade() {
                ctx.copy(&url, tr("Link copied"));
            }
        });
        self.extras.append(&button);
        self.fit.copy_link.replace(Some(button));
    }

    fn add_more(&self, subject: Subject) {
        let button = gtk::Button::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        super::label_button(&button, tr("More Options"));
        let menu = Rc::downgrade(&self.menu);
        button.connect_clicked(move |button| {
            if let Some(menu) = menu.upgrade() {
                menu.popup_at(subject.clone(), button);
            }
        });
        self.extras.append(&button);
    }

    /// Album: cover, ALBUM/EP/SINGLE, title, artist links, year · tracks,
    /// duration, badges; Play, Shuffle, ♥, Copy link, ⋯.
    pub fn album(ctx: &Rc<Ctx>, album: &Album) -> Self {
        let cover = Cover::new(
            ctx,
            album.cover.clone().map(Image::Cover),
            240,
            "audio-x-generic-symbolic",
            true,
        );
        let kind = match album.kind.as_deref() {
            Some("EP") => tr("EP"),
            Some("SINGLE") => tr("Single"),
            _ => tr("Album"),
        };
        let title = match album.version.as_deref().filter(|v| !v.is_empty()) {
            Some(version) if !album.title.contains(version) => {
                format!("{} ({version})", album.title)
            }
            _ => album.title.clone(),
        };
        let mut header = Self::build(ctx, Art::Cover(cover), kind, &title);
        header.add_line(artist_links(ctx, &album.artists));
        let year = album.release_date.as_deref().and_then(|d| d.get(..4));
        header.add_dimmed(&format::join([
            year.unwrap_or_default().to_owned(),
            format::tracks_summary(album.number_of_tracks as usize, album.duration.into()),
        ]));
        header.add_badges(&quality_badge::album_badges(album));
        header.finish();
        let subject = Subject::Album(Rc::new(album.clone()));
        header.add_heart(ctx, &subject);
        header.add_copy_link(ctx, subject.share_url());
        header.add_more(subject);
        header.fit_actions();
        header
    }

    /// Playlist: cover, PLAYLIST, title, by owner · tracks, duration ·
    /// Public/Private (own), description; Play, Shuffle, ♥ or Edit, ⋯.
    pub fn playlist(ctx: &Rc<Ctx>, playlist: &Playlist) -> Self {
        let image = playlist.square_image.clone().map(Image::Playlist);
        let cover = Cover::new(ctx, image, 240, "view-list-symbolic", true);
        let mut header = Self::build(ctx, Art::Cover(cover), tr("Playlist"), &playlist.title);
        let own = ctx.library.is_own_playlist(playlist);
        let mut meta = Vec::new();
        if let Some(owner) = playlist.creator.name.as_deref().filter(|n| !n.is_empty()) {
            meta.push(tr("by {name}").replace("{name}", owner));
        } else if !own && playlist.creator.id == Some(0) {
            meta.push(tr("by TIDAL").into());
        }
        meta.push(format::tracks_summary(
            playlist.number_of_tracks as usize,
            playlist.duration.into(),
        ));
        if own {
            meta.push(
                if playlist.public_playlist {
                    tr("Public")
                } else {
                    tr("Private")
                }
                .into(),
            );
        }
        header.add_dimmed(&format::join(meta));
        let description = playlist.description.trim();
        if !description.is_empty() {
            header.add_read_more(ctx, description, tr("Description"), &playlist.title);
        }
        header.finish();
        let subject = Subject::Playlist(Rc::new(playlist.clone()));
        if own {
            let edit = gtk::Button::builder()
                .icon_name("edit-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["circular", "flat"])
                .build();
            super::label_button(&edit, tr("Edit Details"));
            let weak = Rc::downgrade(ctx);
            let playlist = playlist.clone();
            edit.connect_clicked(move |_| {
                if let Some(ctx) = weak.upgrade() {
                    dialogs::playlist::edit_playlist(&ctx, &playlist);
                }
            });
            header.extras.append(&edit);
        } else {
            header.add_heart(ctx, &subject);
        }
        header.add_more(subject);
        header.fit_actions();
        header
    }

    /// Artist: avatar, ARTIST, name; Play, Shuffle, Follow, Radio, ⋯.
    pub fn artist(ctx: &Rc<Ctx>, artist: &Artist) -> Self {
        let image = artist.picture.clone().map(Image::Artist).or_else(|| {
            artist
                .selected_album_cover_fallback
                .clone()
                .map(Image::Cover)
        });
        let avatar = super::media_card::avatar(ctx, image, &artist.name, 200);
        let header = Self::build(ctx, Art::Avatar(avatar), tr("Artist"), &artist.name);
        header.finish();
        let subject = Subject::Artist(Rc::new(artist.clone()));
        let follow = HeartButton::follow(ctx);
        follow.set_item(Some(subject.favorite_item()));
        header.extras.append(follow.widget());
        let radio = gtk::Button::builder()
            .icon_name(super::icon_name(
                "swell-radio-symbolic",
                "media-playlist-repeat-symbolic",
            ))
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        super::label_button(&radio, tr("Artist Radio"));
        let weak = Rc::downgrade(ctx);
        let id = artist.id;
        radio.connect_clicked(move |_| {
            if let Some(ctx) = weak.upgrade() {
                ctx.navigate(Route::ArtistRadio(id));
            }
        });
        header.extras.append(&radio);
        header.add_more(subject);
        header.fit_actions();
        header
    }

    /// Mix / radio: cover, MIX / TRACK RADIO / ARTIST RADIO, title,
    /// subtitle; Play, Shuffle, ♥, Copy link, ⋯.
    pub fn mix(ctx: &Rc<Ctx>, mix: &Mix, overtitle: &str) -> Self {
        let cover = Cover::new(
            ctx,
            mix.image.clone().map(Image::Url),
            240,
            "view-list-symbolic",
            true,
        );
        let mut header = Self::build(ctx, Art::Cover(cover), overtitle, &mix.title);
        header.add_dimmed(&mix.subtitle);
        header.finish();
        let subject = Subject::Mix(Rc::new(mix.clone()));
        header.add_heart(ctx, &subject);
        header.add_copy_link(ctx, subject.share_url());
        header.add_more(subject);
        header.fit_actions();
        header
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.widget.upcast_ref()
    }

    pub fn connect_play(&self, f: impl Fn() + 'static) {
        self.play.connect_clicked(move |_| f());
    }

    pub fn connect_shuffle(&self, f: impl Fn() + 'static) {
        self.shuffle.connect_clicked(move |_| f());
    }

    /// Play and Shuffle are insensitive until there is something to play.
    pub fn set_playable(&self, playable: bool) {
        self.play.set_sensitive(playable);
        self.shuffle.set_sensitive(playable);
    }

    /// Adds a widget under the text (e.g. a bio excerpt), before the badges
    /// and actions.
    pub fn add_detail(&self, widget: &impl IsA<gtk::Widget>) {
        self.text
            .insert_child_after(widget, self.badges.prev_sibling().as_ref());
    }

    /// The menu of the header's "⋯" button.
    pub fn menu(&self) -> &Rc<ItemMenu> {
        &self.menu
    }

    /// Stacked and centred (narrow pages) or side by side.
    pub fn set_compact(&self, compact: bool) {
        self.art.compact.set(compact);
        self.art.apply();
        self.widget.set_orientation(if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        self.widget.set_spacing(if compact { 12 } else { 24 });
        let (xalign, justify, align) = if compact {
            (0.5, gtk::Justification::Center, gtk::Align::Center)
        } else {
            (0.0, gtk::Justification::Left, gtk::Align::Start)
        };
        for label in self.labels.iter().chain(&self.details).chain([&self.title]) {
            label.set_xalign(xalign);
            label.set_justify(justify);
        }
        if compact {
            self.title.remove_css_class("title-1");
            self.title.add_css_class("title-2");
        } else {
            self.title.remove_css_class("title-2");
            self.title.add_css_class("title-1");
        }
        self.badges.set_halign(align);
        self.actions_bin.set_halign(align);
        self.actions.set_align(xalign);
        self.actions.set_child_spacing(if compact { 6 } else { 12 });
        self.extras.set_spacing(if compact { 6 } else { 12 });
        // Details are a label or a box of labels and buttons.
        let mut detail = self.badges.prev_sibling();
        while let Some(widget) = detail.filter(|w| {
            self.labels
                .iter()
                .all(|l| l.upcast_ref::<gtk::Widget>() != w)
                && w != self.title.upcast_ref::<gtk::Widget>()
        }) {
            let mut child = Some(widget.clone());
            if widget.first_child().is_some() && !widget.is::<gtk::Label>() {
                child = widget.first_child();
            }
            while let Some(part) = child {
                if let Some(label) = part.downcast_ref::<gtk::Label>() {
                    label.set_xalign(xalign);
                    label.set_justify(justify);
                } else {
                    part.set_halign(align);
                }
                child = if part == widget {
                    None
                } else {
                    part.next_sibling()
                };
            }
            detail = widget.prev_sibling();
        }
        // Narrow: "(▶ Play) (⤮) ♥ ⋯".
        self.fit.compact.set(compact);
        self.fit.update();
    }
}

/// "Artist A, Artist B" as links that open the artists.
pub fn artist_links(ctx: &Rc<Ctx>, artists: &[ArtistRef]) -> gtk::Label {
    let markup = artists
        .iter()
        .map(|artist| {
            format!(
                "<a href=\"swell:artist/{}\">{}</a>",
                artist.id,
                glib::markup_escape_text(&artist.name)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let label = gtk::Label::builder()
        .use_markup(true)
        .label(&markup)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["text-link"])
        .build();
    let ctx = Rc::downgrade(ctx);
    label.connect_activate_link(move |_, uri| {
        if let (Some(ctx), Some(id)) = (
            ctx.upgrade(),
            uri.strip_prefix("swell:artist/")
                .and_then(|id| id.parse().ok()),
        ) {
            ctx.navigate(Route::Artist(id));
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    label
}
