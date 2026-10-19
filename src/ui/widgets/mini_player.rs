//! The narrow-layout mini player (docs/design.md §4.10): a thin progress
//! line over the cover, title, artists, play/pause and next. Tapping it
//! (or Enter/Space with focus) opens the now-playing panel on Playing.
//!
//! Hidden until something is playing. The shell keeps the returned `Rc`
//! as long as the widget is shown.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gdk, glib};

use super::super::context::{Ctx, PanelTab};
use super::np_common::{Cover, Subscription, Ticker, subscribe};
use super::np_controls::{PlayButton, TICK};
use crate::i18n::tr;
use crate::playback::{Event, PlayState};

const COVER_SIZE: i32 = 42;
const BUTTON_SIZE: i32 = 44;

pub struct MiniPlayer {
    pub widget: gtk::Widget,
    ctx: Weak<Ctx>,
    area: gtk::Box,
    progress: gtk::ProgressBar,
    cover: Cover,
    title: gtk::Label,
    artists: gtk::Label,
    play: PlayButton,
    next: gtk::Button,
    ticker: RefCell<Option<Ticker>>,
    _events: Subscription,
}

impl MiniPlayer {
    pub fn new(ctx: &Rc<Ctx>) -> Rc<Self> {
        let playback = &ctx.playback;
        let progress = gtk::ProgressBar::builder()
            .css_classes(["mini-progress"])
            .accessible_role(gtk::AccessibleRole::Presentation)
            .build();
        let cover = Cover::new(COVER_SIZE);
        let label = |classes: &[&str]| {
            gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .single_line_mode(true)
                .css_classes(classes.to_vec())
                .build()
        };
        let title = label(&["heading"]);
        let artists = label(&["caption", "dimmed"]);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&title);
        text.append(&artists);
        let play = PlayButton::new(playback, &["circular", "flat"]);
        play.set_size(BUTTON_SIZE);
        let next = gtk::Button::builder()
            .icon_name("media-skip-forward-symbolic")
            .valign(gtk::Align::Center)
            .width_request(BUTTON_SIZE)
            .height_request(BUTTON_SIZE)
            .css_classes(["circular", "flat"])
            .tooltip_text(tr("Next Track"))
            .build();
        next.update_property(&[gtk::accessible::Property::Label(tr("Next Track"))]);

        let area = gtk::Box::builder()
            .spacing(12)
            .height_request(60)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .focusable(true)
            .accessible_role(gtk::AccessibleRole::Button)
            .build();
        area.append(&cover.image);
        area.append(&text);
        area.append(&play.button);
        area.append(&next);

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["mini-player"])
            .visible(false)
            .build();
        widget.append(&progress);
        widget.append(&area);

        let mini = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget: widget.upcast(),
            ctx: Rc::downgrade(ctx),
            area,
            progress,
            cover,
            title,
            artists,
            play,
            next,
            ticker: RefCell::default(),
            _events: subscribe(playback, weak, |mini, event| match event {
                Event::Track(_) => mini.show_track(),
                Event::State(state) => mini.show_state(*state),
                Event::Seeked(position) => mini.show_seeked(*position),
                _ => {}
            }),
        });

        let weak = Rc::downgrade(&mini);
        mini.ticker
            .replace(Some(Ticker::new(&mini.progress, playback, TICK, {
                let weak = weak.clone();
                move || {
                    if let Some(mini) = weak.upgrade() {
                        mini.update_progress();
                    }
                }
            })));
        mini.next.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(ctx) = weak.upgrade().and_then(|mini| mini.ctx.upgrade()) {
                    ctx.playback.next();
                }
            }
        });
        // Buttons claim their clicks, so this only sees taps elsewhere.
        let click = gtk::GestureClick::new();
        click.connect_released({
            let weak = weak.clone();
            move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(mini) = weak.upgrade() {
                    mini.open();
                }
            }
        });
        mini.area.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |controller, key, _, _| {
            let on_area = controller.widget().is_some_and(|w| w.has_focus());
            if on_area
                && matches!(
                    key,
                    gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space | gdk::Key::ISO_Enter
                )
                && let Some(mini) = weak.upgrade()
            {
                mini.open();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        mini.area.add_controller(keys);
        mini.show_track();
        mini
    }

    fn open(&self) {
        if let Some(ctx) = self.ctx.upgrade() {
            ctx.show_panel(Some(PanelTab::Playing));
        }
    }

    fn show_track(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let track = ctx.playback.current();
        self.widget.set_visible(track.is_some());
        self.show_state(ctx.playback.state());
        let Some(track) = track else { return };
        self.cover.set(&ctx.covers, track.cover.as_deref());
        self.title.set_label(&track.title);
        let artists = track.artist_names();
        self.artists.set_label(&artists);
        self.area
            .update_property(&[gtk::accessible::Property::Label(
                &tr("Now Playing: {title} by {artists}. Open player")
                    .replace("{title}", &track.title)
                    .replace("{artists}", &artists),
            )]);
        self.progress.set_fraction(0.0);
    }

    fn show_state(&self, state: PlayState) {
        let has_track = self
            .ctx
            .upgrade()
            .is_some_and(|ctx| ctx.playback.current().is_some());
        self.play.set_state(state, has_track);
        self.next.set_sensitive(has_track);
    }

    fn update_progress(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.set_progress(ctx.playback.position(), ctx.playback.duration());
    }

    /// The engine may still report the old position right after a seek.
    fn show_seeked(&self, position: f64) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        self.set_progress(Some(position), ctx.playback.duration());
    }

    fn set_progress(&self, position: Option<f64>, duration: Option<f64>) {
        let fraction = match (position, duration) {
            (Some(position), Some(duration)) if duration > 0.0 => position / duration,
            _ => 0.0,
        };
        self.progress.set_fraction(fraction.clamp(0.0, 1.0));
    }
}
