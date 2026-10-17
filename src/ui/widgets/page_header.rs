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

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::heart_button::HeartButton;
use super::media_card::Cover;
use super::quality_badge::{self, QualityBadge};
use crate::covers::Image;
use crate::model::format_total;
use crate::tidal::models::{Album, Artist, ArtistRef, Mix, Playlist};
use crate::ui::context::Ctx;
use crate::ui::dialogs;
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

    fn set_compact(&self, compact: bool) {
        match self {
            Self::Cover(cover) => cover.set_size(if compact { 180 } else { 240 }),
            Self::Avatar(avatar) => avatar.set_size(if compact { 144 } else { 200 }),
        }
    }
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
            .set_label(if label { "Shuffle" } else { "" });
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

    /// A single-child container that reports its width when allocated.
    #[derive(Default)]
    pub struct FitBin {
        pub on_allocate: RefCell<Option<Box<dyn Fn(i32)>>>,
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
    art: Art,
    text: gtk::Box,
    labels: Vec<gtk::Label>,
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
                    .label("Play")
                    .build(),
            )
            .css_classes(["pill", "suggested-action"])
            .build();
        let shuffle_content = adw::ButtonContent::builder()
            .icon_name("media-playlist-shuffle-symbolic")
            .label("Shuffle")
            .build();
        let shuffle = gtk::Button::builder()
            .child(&shuffle_content)
            .tooltip_text("Shuffle")
            .css_classes(["pill"])
            .build();
        shuffle.update_property(&[gtk::accessible::Property::Label("Shuffle")]);
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
        self.fit.without_label_width.set(if self.fit.copy_link.borrow().is_some() {
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
        super::label_button(&button, "Copy Link");
        let ctx = Rc::downgrade(ctx);
        button.connect_clicked(move |_| {
            if let Some(ctx) = ctx.upgrade() {
                ctx.copy(&url, "Link");
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
        super::label_button(&button, "More Options");
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
            Some("EP") => "EP",
            Some("SINGLE") => "Single",
            _ => "Album",
        };
        let title = match album.version.as_deref().filter(|v| !v.is_empty()) {
            Some(version) if !album.title.contains(version) => {
                format!("{} ({version})", album.title)
            }
            _ => album.title.clone(),
        };
        let mut header = Self::build(ctx, Art::Cover(cover), kind, &title);
        header.add_line(artist_links(ctx, &album.artists));
        let mut meta = Vec::new();
        if let Some(year) = album.release_date.as_deref().and_then(|d| d.get(..4)) {
            meta.push(year.to_owned());
        }
        let tracks = match album.number_of_tracks {
            0 => String::new(),
            1 => "1 track".into(),
            n => format!("{n} tracks"),
        };
        meta.push(match (tracks.is_empty(), album.duration) {
            (_, 0) => tracks,
            (true, seconds) => format_total(seconds.into()),
            (false, seconds) => format!("{tracks}, {}", format_total(seconds.into())),
        });
        meta.retain(|m| !m.is_empty());
        header.add_dimmed(&meta.join(" · "));
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
        let mut header = Self::build(ctx, Art::Cover(cover), "Playlist", &playlist.title);
        let own = ctx.library.is_own_playlist(playlist);
        let mut meta = Vec::new();
        if let Some(owner) = playlist.creator.name.as_deref().filter(|n| !n.is_empty()) {
            meta.push(format!("by {owner}"));
        } else if !own && playlist.creator.id == Some(0) {
            meta.push("by TIDAL".into());
        }
        let tracks = match playlist.number_of_tracks {
            1 => "1 track".to_owned(),
            n => format!("{n} tracks"),
        };
        meta.push(if playlist.duration > 0 {
            format!("{tracks}, {}", format_total(playlist.duration.into()))
        } else {
            tracks
        });
        if own {
            meta.push(
                if playlist.public_playlist {
                    "Public"
                } else {
                    "Private"
                }
                .into(),
            );
        }
        header.add_dimmed(&meta.join(" · "));
        if !playlist.description.is_empty() {
            let description = gtk::Label::builder()
                .label(&playlist.description)
                .xalign(0.0)
                .wrap(true)
                .lines(3)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dimmed"])
                .build();
            header.add_line(description);
        }
        header.finish();
        let subject = Subject::Playlist(Rc::new(playlist.clone()));
        if own {
            let edit = gtk::Button::builder()
                .icon_name("edit-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["circular", "flat"])
                .build();
            super::label_button(&edit, "Edit Details");
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
        let header = Self::build(ctx, Art::Avatar(avatar), "Artist", &artist.name);
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
        super::label_button(&radio, "Artist Radio");
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
        self.art.set_compact(compact);
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
        for label in self.labels.iter().chain([&self.title]) {
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
        .css_classes(["heading"])
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
