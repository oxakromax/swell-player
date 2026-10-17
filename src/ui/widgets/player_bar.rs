//! The bar at the bottom of the window: play/pause, the current track and a
//! progress bar you can drag to seek.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::model::{TrackInfo, format_duration};
use crate::playback::{Event, PlayState, Playback};

/// How often the position is refreshed while playing.
const TICK: Duration = Duration::from_millis(500);

pub struct PlayerBar {
    pub widget: gtk::Box,
    player: Rc<Playback>,
    play: gtk::Button,
    title: gtk::Label,
    subtitle: gtk::Label,
    position: gtk::Label,
    duration: gtk::Label,
    progress: gtk::Scale,
    tick: RefCell<Option<glib::SourceId>>,
}

impl PlayerBar {
    pub fn new(player: Rc<Playback>) -> Rc<Self> {
        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Play")
            .valign(gtk::Align::Center)
            .sensitive(false)
            .css_classes(["circular", "suggested-action"])
            .build();

        let title = ellipsized_label();
        title.add_css_class("heading");
        let subtitle = ellipsized_label();
        subtitle.add_css_class("dim-label");
        subtitle.add_css_class("caption");
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .width_request(260)
            .build();
        text.append(&title);
        text.append(&subtitle);

        let position = time_label();
        let duration = time_label();
        let progress = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .sensitive(false)
            .build();

        let widget = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        widget.append(&play);
        widget.append(&text);
        widget.append(&position);
        widget.append(&progress);
        widget.append(&duration);

        let bar = Rc::new(Self {
            widget,
            player,
            play,
            title,
            subtitle,
            position,
            duration,
            progress,
            tick: RefCell::default(),
        });

        bar.player.subscribe({
            let bar = Rc::downgrade(&bar);
            move |event| {
                let Some(bar) = bar.upgrade() else { return };
                match event {
                    Event::Track(Some(track)) => bar.show_loading(track),
                    Event::Track(None) => bar.set_state(PlayState::Stopped),
                    Event::State(state) => bar.set_state(*state),
                    Event::Format(Some(format)) => {
                        if let Some(track) = bar.player.current() {
                            bar.show_format(&track, format);
                        }
                    }
                    Event::Error(message) => bar.show_error(message),
                    _ => {}
                }
            }
        });
        bar.play.connect_clicked({
            let bar = Rc::downgrade(&bar);
            move |_| {
                if let Some(bar) = bar.upgrade() {
                    bar.player.toggle();
                }
            }
        });
        // `change-value` only fires on user input, not when the tick moves it.
        bar.progress.connect_change_value({
            let bar = Rc::downgrade(&bar);
            move |_, _, seconds| {
                if let Some(bar) = bar.upgrade() {
                    bar.player.seek(seconds.max(0.0));
                }
                glib::Propagation::Proceed
            }
        });
        bar
    }

    /// A track was chosen and its stream is being looked up.
    pub fn show_loading(&self, track: &TrackInfo) {
        self.widget.set_visible(true);
        self.title.set_label(&track.title);
        self.subtitle.set_label(&track.artist_names());
        self.progress
            .set_range(0.0, f64::from(track.duration.max(1)));
        self.progress.set_value(0.0);
        self.position.set_label(&format_duration(0));
        self.duration
            .set_label(&format_duration(track.duration.into()));
    }

    pub fn show_format(&self, track: &TrackInfo, format: &str) {
        self.subtitle
            .set_label(&format!("{} · {format}", track.artist_names()));
    }

    pub fn show_error(self: &Rc<Self>, message: &str) {
        self.subtitle.set_label(message);
    }

    pub fn set_state(self: &Rc<Self>, state: PlayState) {
        self.widget.set_visible(true);
        let playing = matches!(state, PlayState::Playing | PlayState::Loading);
        let (icon, tooltip) = if playing {
            ("media-playback-pause-symbolic", "Pause")
        } else {
            ("media-playback-start-symbolic", "Play")
        };
        self.play.set_icon_name(icon);
        self.play.set_tooltip_text(Some(tooltip));
        self.play.set_sensitive(self.player.current().is_some());
        self.progress.set_sensitive(state != PlayState::Stopped);

        if state == PlayState::Playing {
            self.update_position();
            if self.tick.borrow().is_none() {
                let bar = Rc::downgrade(self);
                let id = glib::timeout_add_local(TICK, move || match bar.upgrade() {
                    Some(bar) => {
                        bar.update_position();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                });
                self.tick.replace(Some(id));
            }
        } else if let Some(id) = self.tick.take() {
            id.remove();
        }
    }

    fn update_position(&self) {
        if let Some(duration) = self.player.duration() {
            self.progress.set_range(0.0, duration.max(1.0));
            self.duration.set_label(&format_duration(duration as u64));
        }
        if let Some(position) = self.player.position() {
            self.progress.set_value(position);
            self.position.set_label(&format_duration(position as u64));
        }
    }
}

fn ellipsized_label() -> gtk::Label {
    gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .build()
}

fn time_label() -> gtk::Label {
    gtk::Label::builder()
        .css_classes(["numeric", "dim-label"])
        .build()
}
