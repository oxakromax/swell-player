//! The now-playing panel: Playing, Lyrics and Queue tabs (docs/design.md
//! §3.13). The shell places it (docked or overlaid side panel, or the
//! narrow bottom sheet), calls [`NowPlayingPanel::show_tab`] when it opens
//! it and [`NowPlayingPanel::set_narrow`] at breakpoint C. Switching tabs
//! inside the panel goes through `ctx.show_panel` so the shell (and the
//! player bar toggles) stay in step.
//!
//! The shell keeps the returned `Rc` as long as the widget is shown.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;

use super::super::context::{Ctx, PanelTab};
use super::super::nav::Route;
use super::lyrics_view::LyricsView;
use super::np_common::{
    self, Cover, FormatBadge, Heart, Subscription, album_markup, artists_markup, device_pixels,
    explicit_badge, icons, link_label, quality_badge, set_quality_badge, subscribe,
};
use super::np_controls::{SeekBar, Transport, Volume};
use super::queue_view::QueueView;
use crate::covers::{Image, Ticket};
use crate::playback::Event;

/// Largest cover size (logical pixels).
const COVER_MAX: i32 = 480;

fn tab_name(tab: PanelTab) -> &'static str {
    match tab {
        PanelTab::Playing => "playing",
        PanelTab::Lyrics => "lyrics",
        PanelTab::Queue => "queue",
    }
}

fn tab_from_name(name: &str) -> Option<PanelTab> {
    match name {
        "playing" => Some(PanelTab::Playing),
        "lyrics" => Some(PanelTab::Lyrics),
        "queue" => Some(PanelTab::Queue),
        _ => None,
    }
}

pub struct NowPlayingPanel {
    pub widget: gtk::Widget,
    ctx: Weak<Ctx>,
    stack: adw::ViewStack,
    header: adw::HeaderBar,
    close: gtk::Button,
    playing: gtk::Stack,
    picture: gtk::Picture,
    picture_ticket: RefCell<Option<Ticket>>,
    picture_cover: RefCell<Option<String>>,
    title: gtk::Label,
    explicit: gtk::Label,
    artists: gtk::Label,
    album: gtk::Label,
    quality: gtk::Label,
    format: FormatBadge,
    controls: gtk::Box,
    transport: Rc<Transport>,
    _seek: Rc<SeekBar>,
    volume: Rc<Volume>,
    heart: Rc<Heart>,
    up_next: gtk::Box,
    up_next_cover: Cover,
    up_next_label: gtk::Label,
    lyrics: Rc<LyricsView>,
    _queue: Rc<QueueView>,
    syncing: Cell<bool>,
    _events: Subscription,
}

impl NowPlayingPanel {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let playback = &ctx.playback;

        // ---- Playing tab ----
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["cover", "large", "card"])
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        let placeholder = gtk::Image::builder()
            .icon_name(icons::track())
            .pixel_size(96)
            .css_classes(["dimmed"])
            .build();
        let cover_overlay = gtk::Overlay::builder().child(&placeholder).build();
        cover_overlay.add_overlay(&picture);
        let cover_frame = gtk::AspectFrame::builder()
            .ratio(1.0)
            .obey_child(false)
            .child(&cover_overlay)
            .build();

        let title = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .hexpand(true)
            .css_classes(["title-2"])
            .build();
        let explicit = explicit_badge();
        explicit.set_valign(gtk::Align::Start);
        explicit.set_margin_top(6);
        let title_row = gtk::Box::builder().spacing(6).build();
        title_row.append(&title);
        title_row.append(&explicit);
        let artists = link_label(ctx, &["title-4"]);
        let album = link_label(ctx, &["dimmed"]);
        let quality = quality_badge();
        let format = FormatBadge::new();
        let badges = gtk::Box::builder().spacing(6).build();
        badges.append(&quality);
        badges.append(&format.widget);

        // Narrow only: seek and transport.
        let seek = SeekBar::new(playback);
        let transport = Transport::new(playback);
        let controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .visible(false)
            .build();
        controls.append(&seek.widget);
        controls.append(&transport.widget);

        let heart = Heart::new(ctx);
        let radio_content = adw::ButtonContent::builder()
            .icon_name(icons::radio())
            .label("Radio")
            .build();
        let radio = gtk::Button::builder()
            .child(&radio_content)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Track Radio")
            .build();
        let volume = Volume::new(playback);
        volume.set_compact(true);
        volume.widget.set_visible(false);
        let more = np_common::track_menu_button(ctx);
        for button in [heart.button.upcast_ref::<gtk::Widget>(), more.upcast_ref()] {
            button.set_size_request(36, 36);
        }
        let actions = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        actions.append(&heart.button);
        actions.append(&radio);
        actions.append(&volume.widget);
        actions.append(&more);

        let up_next_cover = Cover::new(40);
        let up_next_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .build();
        let up_next_row = gtk::Box::builder().spacing(12).build();
        up_next_row.append(&up_next_cover.image);
        up_next_row.append(&up_next_label);
        up_next_row.append(&gtk::Image::from_icon_name("go-next-symbolic"));
        let up_next_button = gtk::Button::builder()
            .child(&up_next_row)
            .css_classes(["flat"])
            .tooltip_text("Show Queue")
            .build();
        let up_next = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        up_next.append(
            &gtk::Label::builder()
                .label("UP NEXT")
                .xalign(0.0)
                .css_classes(["caption-heading", "dimmed"])
                .build(),
        );
        up_next.append(&up_next_button);

        let track_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        let text_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        text_box.append(&title_row);
        text_box.append(&artists);
        text_box.append(&album);
        track_box.append(&cover_frame);
        track_box.append(&text_box);
        track_box.append(&badges);
        track_box.append(&controls);
        track_box.append(&actions);
        track_box.append(&up_next);
        let clamp = adw::Clamp::builder()
            .maximum_size(COVER_MAX + 36)
            .child(&track_box)
            .build();
        let playing = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        playing.add_named(
            &adw::StatusPage::builder()
                .icon_name(icons::track())
                .title("Nothing Playing")
                .description("Pick something from your collection.")
                .css_classes(["compact"])
                .build(),
            Some("empty"),
        );
        playing.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&clamp)
                .build(),
            Some("track"),
        );

        // ---- Tabs ----
        let lyrics = LyricsView::new(ctx);
        let queue = QueueView::new(ctx);
        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(&playing, Some("playing"), "Playing", icons::track());
        stack.add_titled_with_icon(&lyrics.widget, Some("lyrics"), "Lyrics", icons::lyrics());
        stack.add_titled_with_icon(&queue.widget, Some("queue"), "Queue", icons::queue());
        let switcher = adw::InlineViewSwitcher::builder()
            .stack(&stack)
            .display_mode(adw::InlineViewSwitcherDisplayMode::Labels)
            .build();
        let close = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Close")
            .visible(false)
            .build();
        close.update_property(&[gtk::accessible::Property::Label("Close")]);
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        header.pack_end(&close);
        let toolbar = adw::ToolbarView::builder()
            .content(&stack)
            .css_classes(["now-playing"])
            .build();
        toolbar.add_top_bar(&header);

        let panel = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget: toolbar.upcast(),
            ctx: Rc::downgrade(ctx),
            stack,
            header,
            close,
            playing,
            picture,
            picture_ticket: RefCell::default(),
            picture_cover: RefCell::default(),
            title,
            explicit,
            artists,
            album,
            quality,
            format,
            controls,
            transport,
            _seek: seek,
            volume,
            heart,
            up_next,
            up_next_cover,
            up_next_label,
            lyrics,
            _queue: queue,
            syncing: Cell::new(false),
            _events: subscribe(playback, weak, |panel, event| match event {
                Event::Track(_) => panel.show_track(None),
                Event::Format(format) => panel.show_track(format.as_deref()),
                Event::Queue => panel.show_up_next(),
                _ => {}
            }),
        });

        let weak = Rc::downgrade(&panel);
        panel.stack.connect_visible_child_name_notify({
            let weak = weak.clone();
            move |stack| {
                let Some(panel) = weak.upgrade() else { return };
                if panel.syncing.get() {
                    return;
                }
                let tab = stack
                    .visible_child_name()
                    .and_then(|name| tab_from_name(&name));
                if let (Some(ctx), Some(tab)) = (panel.ctx.upgrade(), tab)
                    && ctx.panel().is_some()
                    && ctx.panel() != Some(tab)
                {
                    ctx.show_panel(Some(tab));
                }
            }
        });
        panel.close.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(ctx) = weak.upgrade().and_then(|panel| panel.ctx.upgrade()) {
                    ctx.show_panel(None);
                }
            }
        });
        radio.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(ctx) = weak.upgrade().and_then(|panel| panel.ctx.upgrade())
                    && let Some(track) = ctx.playback.current()
                {
                    ctx.navigate(Route::TrackRadio(track.id));
                }
            }
        });
        up_next_button.connect_clicked(move |_| {
            let Some(panel) = weak.upgrade() else { return };
            match panel.ctx.upgrade() {
                Some(ctx) if ctx.panel().is_some() => ctx.show_panel(Some(PanelTab::Queue)),
                _ => panel.show_tab(PanelTab::Queue),
            }
        });
        panel.show_track(ctx.playback.format().as_deref());
        panel
    }

    /// Shows `tab` (the shell has already made the panel visible).
    pub fn show_tab(&self, tab: PanelTab) {
        self.syncing.set(true);
        self.stack.set_visible_child_name(tab_name(tab));
        self.syncing.set(false);
    }

    /// Narrow layout (bottom sheet): transport and volume inside the
    /// Playing tab, a close button, no window buttons.
    pub fn set_narrow(&self, narrow: bool) {
        self.controls.set_visible(narrow);
        self.volume.widget.set_visible(narrow);
        self.transport.set_large(narrow);
        self.close.set_visible(narrow);
        self.header.set_show_start_title_buttons(!narrow);
        self.header.set_show_end_title_buttons(!narrow);
        self.lyrics.set_narrow(narrow);
        let size = if narrow { 44 } else { 36 };
        self.heart.button.set_size_request(size, size);
    }

    fn show_track(&self, format: Option<&str>) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let Some(track) = ctx.playback.current() else {
            self.playing.set_visible_child_name("empty");
            self.heart.set_track(None);
            self.format.set(&ctx, None, None);
            self.set_picture(&ctx, None);
            self.show_up_next();
            return;
        };
        self.playing.set_visible_child_name("track");
        self.set_picture(&ctx, track.cover.as_deref());
        self.title.set_label(&track.title);
        self.title.set_tooltip_text(Some(&track.title));
        self.explicit.set_visible(track.explicit);
        self.artists.set_markup(&artists_markup(&track));
        self.album.set_markup(&album_markup(&track, &track.album));
        self.album.set_visible(!track.album.is_empty());
        set_quality_badge(&self.quality, track.hires);
        self.format.set(&ctx, Some(&track), format);
        self.heart.set_track(Some(track.id));
        self.show_up_next();
    }

    fn set_picture(&self, ctx: &Rc<Ctx>, cover: Option<&str>) {
        if self.picture_cover.borrow().as_deref() == cover && cover.is_some() {
            return;
        }
        self.picture_ticket.take();
        self.picture_cover.replace(cover.map(str::to_owned));
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        let Some(cover) = cover else { return };
        let picture = self.picture.downgrade();
        let ticket = ctx.covers.load(
            &Image::Cover(cover.to_owned()),
            device_pixels(&self.picture, COVER_MAX),
            0,
            move |texture| {
                if let Some(picture) = picture.upgrade() {
                    picture.set_paintable(Some(texture));
                }
            },
        );
        self.picture_ticket.replace(ticket);
    }

    fn show_up_next(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let next = ctx.playback.upcoming().into_iter().next();
        self.up_next.set_visible(next.is_some());
        let Some(next) = next else { return };
        self.up_next_cover.set(&ctx.covers, next.cover.as_deref());
        let text = format!("{} · {}", next.title, next.artist_names());
        self.up_next_label.set_label(&text);
        self.up_next_label.set_tooltip_text(Some(&text));
    }
}
