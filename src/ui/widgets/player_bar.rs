//! The player bar at the bottom of the wide and regular layouts
//! (docs/design.md §4.9): the current track, transport and seek in the
//! centre, format, panel toggles, volume and a menu at the end.
//!
//! Hidden until something is playing. The shell keeps the returned `Rc`
//! as long as the widget is shown; dropping it disconnects everything.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use adw::prelude::*;

use super::super::context::{Ctx, PanelTab};
use super::np_common::{
    Cover, FormatBadge, Heart, PlayerActions, SleepIndicator, Subscription, album_markup,
    artists_markup, explicit_badge, icons, link_label, subscribe, track_menu_button,
};
use super::np_controls::{SeekBar, Transport, Volume};
use crate::playback::Event;

const COVER_SIZE: i32 = 56;

pub struct PlayerBar {
    pub widget: gtk::Widget,
    ctx: Weak<Ctx>,
    cover: Cover,
    title: gtk::Label,
    explicit: gtk::Label,
    artists: gtk::Label,
    heart: Rc<Heart>,
    _transport: Rc<Transport>,
    _seek: Rc<SeekBar>,
    format: Rc<FormatBadge>,
    _sleep: Rc<SleepIndicator>,
    _actions: Rc<PlayerActions>,
    lyrics: gtk::ToggleButton,
    queue: gtk::ToggleButton,
    volume: Rc<Volume>,
    syncing: Cell<bool>,
    _events: Subscription,
}

impl PlayerBar {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let playback = &ctx.playback;

        // Start: cover, title and artists, heart.
        let cover = Cover::new(COVER_SIZE);
        let cover_button = gtk::Button::builder()
            .child(&cover.image)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "player-cover"])
            .tooltip_text("Show Now Playing")
            .build();
        cover_button.update_property(&[gtk::accessible::Property::Label("Show Now Playing")]);
        let title = link_label(ctx, &["heading"]);
        let explicit = explicit_badge();
        let title_row = gtk::Box::builder().spacing(6).build();
        title_row.append(&title);
        title_row.append(&explicit);
        let artists = link_label(ctx, &["dimmed"]);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&title_row);
        text.append(&artists);
        let heart = Heart::new(ctx);
        let start = gtk::Box::builder().spacing(12).width_request(240).build();
        start.append(&cover_button);
        start.append(&text);
        start.append(&heart.button);

        // Centre: transport over the seek bar.
        let transport = Transport::new(playback);
        let seek = SeekBar::new(playback);
        let center_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .width_request(320)
            .build();
        center_box.append(&transport.widget);
        center_box.append(&seek.widget);
        let center = adw::Clamp::builder()
            .maximum_size(640)
            .tightening_threshold(640)
            .child(&center_box)
            .hexpand(true)
            .margin_start(12)
            .margin_end(12)
            .build();

        // End: format, sleep timer, lyrics, queue, volume, more.
        let format = FormatBadge::new(ctx);
        let sleep = SleepIndicator::new(playback);
        let panel_toggle = |icon: &str, tooltip: &str| {
            let button = gtk::ToggleButton::builder()
                .icon_name(icon)
                .valign(gtk::Align::Center)
                .tooltip_text(tooltip)
                .css_classes(["flat", "circular"])
                .build();
            button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
            button
        };
        let lyrics = panel_toggle(icons::lyrics(), "Lyrics");
        let queue = panel_toggle(icons::queue(), "Queue");
        let volume = Volume::new(playback);
        let more = track_menu_button();
        let end = gtk::Box::builder()
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();
        end.append(&format.widget);
        end.append(&sleep.widget);
        end.append(&lyrics);
        end.append(&queue);
        end.append(&volume.widget);
        end.append(&more);

        let bar_box = gtk::CenterBox::builder()
            .start_widget(&start)
            .center_widget(&center)
            .end_widget(&end)
            .height_request(78)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["player-bar"])
            .visible(false)
            .build();
        bar_box.update_property(&[gtk::accessible::Property::Label("Player")]);
        let actions = PlayerActions::new(ctx);
        bar_box.insert_action_group("player", Some(&actions.group));

        let bar = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget: bar_box.upcast(),
            ctx: Rc::downgrade(ctx),
            cover,
            title,
            explicit,
            artists,
            heart,
            _transport: transport,
            _seek: seek,
            format,
            _sleep: sleep,
            _actions: actions,
            lyrics,
            queue,
            volume,
            syncing: Cell::new(false),
            _events: subscribe(playback, weak, |bar, event| {
                if let Event::Track(_) = event {
                    bar.show_track();
                }
            }),
        });

        let weak = Rc::downgrade(&bar);
        cover_button.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(ctx) = weak.upgrade().and_then(|bar| bar.ctx.upgrade()) {
                    ctx.toggle_panel(PanelTab::Playing);
                }
            }
        });
        for (button, tab) in [
            (&bar.lyrics, PanelTab::Lyrics),
            (&bar.queue, PanelTab::Queue),
        ] {
            let weak = weak.clone();
            button.connect_clicked(move |_| {
                let Some(bar) = weak.upgrade() else { return };
                if bar.syncing.get() {
                    return;
                }
                if let Some(ctx) = bar.ctx.upgrade() {
                    ctx.toggle_panel(tab);
                    // Reflect what the shell actually did (also when the
                    // shell has no panel and ignored the request).
                    bar.sync_panel(ctx.panel());
                }
            });
        }
        ctx.connect_panel_changed(move |tab| {
            if let Some(bar) = weak.upgrade() {
                bar.sync_panel(tab);
            }
        });
        bar.sync_panel(ctx.panel());
        bar.show_track();
        bar
    }

    /// Breakpoint B: volume in a popover, short format badge.
    pub fn set_compact(&self, compact: bool) {
        self.volume.set_compact(compact);
        self.format.set_compact(compact);
    }

    fn sync_panel(&self, tab: Option<PanelTab>) {
        self.syncing.set(true);
        self.lyrics.set_active(tab == Some(PanelTab::Lyrics));
        self.queue.set_active(tab == Some(PanelTab::Queue));
        self.syncing.set(false);
    }

    fn show_track(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let track = ctx.playback.current();
        self.widget.set_visible(track.is_some());
        let Some(track) = track else {
            self.heart.set_track(None);
            return;
        };
        self.cover.set(&ctx.covers, track.cover.as_deref());
        self.title.set_markup(&album_markup(&track, &track.title));
        self.title.set_tooltip_text(Some(&track.title));
        self.explicit.set_visible(track.explicit);
        self.artists.set_markup(&artists_markup(&track));
        self.artists.set_tooltip_text(Some(&track.artist_names()));
        self.heart.set_track(Some(track.id));
        self.widget
            .update_property(&[gtk::accessible::Property::Description(&format!(
                "{} by {}",
                track.title,
                track.artist_names()
            ))]);
    }
}
