//! Playback controls shared by the player bar and the narrow now-playing
//! panel: transport buttons, the seek bar and the volume control.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib};

use super::np_common::{Subscription, Ticker, spoken_duration, subscribe};
use crate::i18n::tr;
use crate::model::format_duration;
use crate::playback::{Event, PlayState, Playback};
use crate::settings::Repeat;

/// How often the position is refreshed while playing.
pub(super) const TICK: Duration = Duration::from_millis(500);
/// The spinner only appears if loading takes longer than this.
const SPINNER_DELAY: Duration = Duration::from_millis(300);
/// After a seek, the engine may still report the old position briefly.
const SEEK_SETTLE: Duration = Duration::from_millis(800);
/// Volume change per scroll step or key press.
const VOLUME_STEP: f64 = 0.05;

/// Play/pause with a delayed loading spinner.
pub(super) struct PlayButton {
    pub button: gtk::Button,
    stack: gtk::Stack,
    image: gtk::Image,
    state: Cell<Option<PlayState>>,
    /// Bumped on every state change; a pending spinner checks it.
    generation: Rc<Cell<u32>>,
}

impl PlayButton {
    pub fn new(playback: &Rc<Playback>, classes: &[&str]) -> Self {
        let image = gtk::Image::from_icon_name("media-playback-start-symbolic");
        let spinner = adw::Spinner::builder()
            .width_request(16)
            .height_request(16)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&image, Some("icon"));
        stack.add_named(&spinner, Some("spinner"));
        let button = gtk::Button::builder()
            .child(&stack)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .tooltip_text(tr("Play"))
            .css_classes(classes.to_vec())
            .sensitive(false)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(tr("Play"))]);
        let weak = Rc::downgrade(playback);
        button.connect_clicked(move |_| {
            if let Some(playback) = weak.upgrade() {
                playback.toggle();
            }
        });
        Self {
            button,
            stack,
            image,
            state: Cell::new(None),
            generation: Rc::default(),
        }
    }

    pub fn set_size(&self, size: i32) {
        self.button.set_size_request(size, size);
    }

    pub fn set_state(&self, state: PlayState, has_track: bool) {
        self.button.set_sensitive(has_track);
        if self.state.replace(Some(state)) == Some(state) {
            return;
        }
        self.generation.set(self.generation.get().wrapping_add(1));
        let busy = matches!(state, PlayState::Playing | PlayState::Loading);
        let (icon, label) = if busy {
            ("media-playback-pause-symbolic", tr("Pause"))
        } else {
            ("media-playback-start-symbolic", tr("Play"))
        };
        self.image.set_icon_name(Some(icon));
        self.button.set_tooltip_text(Some(label));
        self.button
            .update_property(&[gtk::accessible::Property::Label(label)]);
        if state == PlayState::Loading {
            let generation = self.generation.clone();
            let expected = generation.get();
            let stack = self.stack.downgrade();
            let button = self.button.downgrade();
            glib::timeout_add_local_once(SPINNER_DELAY, move || {
                if generation.get() != expected {
                    return;
                }
                if let (Some(stack), Some(button)) = (stack.upgrade(), button.upgrade()) {
                    stack.set_visible_child_name("spinner");
                    button.update_property(&[gtk::accessible::Property::Label(tr("Loading"))]);
                }
            });
        } else {
            self.stack.set_visible_child_name("icon");
        }
    }
}

/// Shuffle, previous, play/pause, next, repeat.
pub(super) struct Transport {
    pub widget: gtk::Box,
    playback: Rc<Playback>,
    shuffle: gtk::ToggleButton,
    previous: gtk::Button,
    pub play: PlayButton,
    next: gtk::Button,
    repeat: gtk::ToggleButton,
    syncing: Cell<bool>,
    _events: Subscription,
}

impl Transport {
    pub fn new(playback: &Rc<Playback>) -> Rc<Self> {
        let toggle = |icon: &str, tooltip: &str| {
            let button = gtk::ToggleButton::builder()
                .icon_name(icon)
                .valign(gtk::Align::Center)
                .tooltip_text(tooltip)
                .css_classes(["circular", "flat"])
                .build();
            button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
            button
        };
        let button = |icon: &str, tooltip: &str| {
            let button = gtk::Button::builder()
                .icon_name(icon)
                .valign(gtk::Align::Center)
                .tooltip_text(tooltip)
                .css_classes(["circular", "flat"])
                .build();
            button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
            button
        };
        let shuffle = toggle("media-playlist-shuffle-symbolic", tr("Shuffle"));
        let previous = button("media-skip-backward-symbolic", tr("Previous Track"));
        let play = PlayButton::new(playback, &["circular", "suggested-action"]);
        let next = button("media-skip-forward-symbolic", tr("Next Track"));
        let repeat = toggle("media-playlist-repeat-symbolic", tr("Repeat: Off"));
        repeat.update_property(&[gtk::accessible::Property::Label(tr("Repeat"))]);

        let widget = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        widget.append(&shuffle);
        widget.append(&previous);
        widget.append(&play.button);
        widget.append(&next);
        widget.append(&repeat);

        let transport = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget,
            playback: playback.clone(),
            shuffle,
            previous,
            play,
            next,
            repeat,
            syncing: Cell::new(false),
            _events: subscribe(playback, weak, |transport, event| match event {
                Event::Track(_) | Event::State(_) | Event::Options | Event::Queue => {
                    transport.sync()
                }
                _ => {}
            }),
        });
        transport.set_large(false);

        let weak = Rc::downgrade(&transport);
        transport.shuffle.connect_toggled({
            let weak = weak.clone();
            move |button| {
                if let Some(transport) = weak.upgrade()
                    && !transport.syncing.get()
                    && transport.playback.shuffle() != button.is_active()
                {
                    transport.playback.set_shuffle(button.is_active());
                }
            }
        });
        transport.repeat.connect_clicked({
            let weak = weak.clone();
            move |_| {
                let Some(transport) = weak.upgrade() else {
                    return;
                };
                if transport.syncing.get() {
                    return;
                }
                let next = match transport.playback.repeat() {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
                transport.playback.set_repeat(next);
                transport.sync();
            }
        });
        transport.previous.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(transport) = weak.upgrade() {
                    transport.playback.previous();
                }
            }
        });
        transport.next.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(transport) = weak.upgrade() {
                    transport.playback.next();
                }
            }
        });
        // "Restart Track" once more than 3 s in, like `Playback::previous`.
        transport.previous.set_has_tooltip(true);
        transport
            .previous
            .connect_query_tooltip(move |_, _, _, _, tooltip| {
                let Some(transport) = weak.upgrade() else {
                    return false;
                };
                let restart = transport.playback.position().is_some_and(|p| p > 3.0);
                tooltip.set_text(Some(if restart {
                    tr("Restart Track")
                } else {
                    tr("Previous Track")
                }));
                true
            });
        // Never trust a state missed while hidden (e.g. a session restored
        // before the bar was shown): resync when shown.
        transport.widget.connect_map({
            let weak = Rc::downgrade(&transport);
            move |_| {
                if let Some(transport) = weak.upgrade() {
                    transport.sync();
                }
            }
        });
        transport.sync();
        transport
    }

    /// Narrow layout: 44 px buttons and a 56 px play button.
    pub fn set_large(&self, large: bool) {
        let (small, medium, play) = if large { (44, 44, 56) } else { (32, 36, 40) };
        self.shuffle.set_size_request(small, small);
        self.repeat.set_size_request(small, small);
        self.previous.set_size_request(medium, medium);
        self.next.set_size_request(medium, medium);
        self.play.set_size(play);
        self.widget.set_spacing(if large { 18 } else { 6 });
    }

    fn sync(&self) {
        // Playable whenever there is a current track, even before its
        // stream is resolved (a restored session).
        let has_track = self.playback.current().is_some();
        self.syncing.set(true);
        self.play.set_state(self.playback.state(), has_track);
        self.shuffle.set_active(self.playback.shuffle());
        let repeat = self.playback.repeat();
        self.repeat.set_active(repeat != Repeat::Off);
        self.syncing.set(false);
        let (icon, tooltip) = match repeat {
            Repeat::Off => ("media-playlist-repeat-symbolic", tr("Repeat: Off")),
            Repeat::All => ("media-playlist-repeat-symbolic", tr("Repeat: All")),
            Repeat::One => ("media-playlist-repeat-song-symbolic", tr("Repeat: One")),
        };
        self.repeat.set_icon_name(icon);
        self.repeat.set_tooltip_text(Some(tooltip));
        self.repeat
            .update_property(&[gtk::accessible::Property::Description(tooltip)]);
        for button in [&self.previous, &self.next] {
            button.set_sensitive(has_track);
        }
    }
}

/// Position, seek scale and duration. Dragging does not fight the tick:
/// the seek happens on release (keyboard changes seek at once).
pub(super) struct SeekBar {
    pub widget: gtk::Box,
    playback: Rc<Playback>,
    position: gtk::Label,
    scale: gtk::Scale,
    duration: gtk::Label,
    pressed: Cell<bool>,
    pending: Cell<Option<f64>>,
    settle_until: Cell<Option<Instant>>,
    ticker: RefCell<Option<Ticker>>,
    _events: Subscription,
}

impl SeekBar {
    pub fn new(playback: &Rc<Playback>) -> Rc<Self> {
        let time_label = |xalign: f32| {
            gtk::Label::builder()
                .label("0:00")
                .xalign(xalign)
                .width_chars(5)
                .css_classes(["numeric", "caption", "dimmed"])
                .build()
        };
        let position = time_label(1.0);
        let duration = time_label(0.0);
        let adjustment = gtk::Adjustment::new(0.0, 0.0, 1.0, 5.0, 30.0, 0.0);
        let scale = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .adjustment(&adjustment)
            .draw_value(false)
            .hexpand(true)
            .sensitive(false)
            .build();
        scale.update_property(&[gtk::accessible::Property::Label(tr("Seek"))]);
        let widget = gtk::Box::builder().spacing(6).hexpand(true).build();
        widget.append(&position);
        widget.append(&scale);
        widget.append(&duration);

        let bar = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget,
            playback: playback.clone(),
            position,
            scale,
            duration,
            pressed: Cell::new(false),
            pending: Cell::new(None),
            settle_until: Cell::new(None),
            ticker: RefCell::default(),
            _events: subscribe(playback, weak, |bar, event| match event {
                Event::Track(track) => bar.reset(track.as_ref().map(|t| t.duration)),
                Event::State(_) => bar.update(),
                _ => {}
            }),
        });
        let weak = Rc::downgrade(&bar);
        bar.ticker
            .replace(Some(Ticker::new(&bar.widget, playback, TICK, {
                let weak = weak.clone();
                move || {
                    if let Some(bar) = weak.upgrade() {
                        bar.update();
                    }
                }
            })));

        // Track the pointer so a drag only seeks on release.
        let legacy = gtk::EventControllerLegacy::new();
        legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
        legacy.connect_event({
            let weak = weak.clone();
            move |_, event| {
                let Some(bar) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                match event.event_type() {
                    gdk::EventType::ButtonPress | gdk::EventType::TouchBegin => {
                        bar.pressed.set(true)
                    }
                    gdk::EventType::ButtonRelease
                    | gdk::EventType::TouchEnd
                    | gdk::EventType::TouchCancel => {
                        bar.pressed.set(false);
                        if let Some(position) = bar.pending.take() {
                            bar.seek(position);
                        }
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            }
        });
        bar.scale.add_controller(legacy);
        bar.scale.connect_change_value(move |scale, _, value| {
            let Some(bar) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let adjustment = scale.adjustment();
            let value = value.clamp(adjustment.lower(), adjustment.upper());
            bar.show_position(value, adjustment.upper());
            if bar.pressed.get() {
                bar.pending.set(Some(value));
            } else {
                bar.seek(value);
            }
            glib::Propagation::Proceed
        });
        bar.reset(playback.current().map(|t| t.duration));
        bar
    }

    fn seek(&self, position: f64) {
        self.settle_until.set(Some(Instant::now() + SEEK_SETTLE));
        self.scale.set_value(position);
        self.playback.seek(position);
    }

    fn reset(&self, duration: Option<u32>) {
        let duration = f64::from(duration.unwrap_or(0));
        self.pending.set(None);
        self.settle_until.set(None);
        self.scale.set_range(0.0, duration.max(1.0));
        self.scale.set_value(0.0);
        self.show_position(0.0, duration);
        self.update();
    }

    fn update(&self) {
        let state = self.playback.state();
        self.scale
            .set_sensitive(self.playback.current().is_some() && state != PlayState::Stopped);
        let duration = self.playback.duration().unwrap_or(0.0);
        if duration > 0.0 {
            self.scale.set_range(0.0, duration);
        }
        if self.pressed.get() || self.pending.get().is_some() {
            return;
        }
        if let Some(until) = self.settle_until.get() {
            if Instant::now() < until {
                return;
            }
            self.settle_until.set(None);
        }
        let position = match state {
            PlayState::Stopped => 0.0,
            _ => self.playback.position().unwrap_or(self.scale.value()),
        };
        self.scale.set_value(position);
        self.show_position(position, duration);
    }

    fn show_position(&self, position: f64, duration: f64) {
        let (position, duration) = (position.max(0.0) as u64, duration.max(0.0) as u64);
        self.position.set_label(&format_duration(position));
        self.duration.set_label(&format_duration(duration));
        self.scale
            .update_property(&[gtk::accessible::Property::ValueText(
                &tr("{position} of {duration}")
                    .replace("{position}", &spoken_duration(position))
                    .replace("{duration}", &spoken_duration(duration)),
            )]);
    }
}

/// Mute button and volume scale; compact: one button with a popover.
pub(super) struct Volume {
    pub widget: gtk::Box,
    playback: Rc<Playback>,
    mute: gtk::Button,
    scale: gtk::Scale,
    menu: gtk::MenuButton,
    popover_mute: gtk::Button,
    popover_scale: gtk::Scale,
    syncing: Cell<bool>,
    _events: Subscription,
}

impl Volume {
    pub fn new(playback: &Rc<Playback>) -> Rc<Self> {
        let mute = gtk::Button::builder()
            .icon_name("audio-volume-high-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, VOLUME_STEP);
        scale.set_width_request(100);
        scale.set_draw_value(false);
        scale.set_valign(gtk::Align::Center);
        scale.update_property(&[gtk::accessible::Property::Label(tr("Volume"))]);

        let popover_scale =
            gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 1.0, VOLUME_STEP);
        popover_scale.set_inverted(true);
        popover_scale.set_draw_value(false);
        popover_scale.set_height_request(140);
        popover_scale.update_property(&[gtk::accessible::Property::Label(tr("Volume"))]);
        let popover_mute = gtk::Button::builder()
            .icon_name("audio-volume-high-symbolic")
            .css_classes(["flat", "circular"])
            .build();
        let popover_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        popover_box.append(&popover_scale);
        popover_box.append(&popover_mute);
        let menu = gtk::MenuButton::builder()
            .icon_name("audio-volume-high-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .tooltip_text(tr("Volume"))
            .popover(&gtk::Popover::builder().child(&popover_box).build())
            .visible(false)
            .build();
        menu.update_property(&[gtk::accessible::Property::Label(tr("Volume"))]);

        let widget = gtk::Box::builder()
            .spacing(0)
            .valign(gtk::Align::Center)
            .build();
        widget.append(&mute);
        widget.append(&scale);
        widget.append(&menu);

        let volume = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            widget,
            playback: playback.clone(),
            mute,
            scale,
            menu,
            popover_mute,
            popover_scale,
            syncing: Cell::new(false),
            _events: subscribe(playback, weak, |volume, event| {
                if matches!(event, Event::Options) {
                    volume.sync();
                }
            }),
        });
        let weak = Rc::downgrade(&volume);
        for scale in [&volume.scale, &volume.popover_scale] {
            let weak = weak.clone();
            scale.connect_value_changed(move |scale| {
                let Some(volume) = weak.upgrade() else { return };
                if volume.syncing.get() {
                    return;
                }
                if volume.playback.is_muted() && scale.value() > 0.0 {
                    volume.playback.set_muted(false);
                }
                volume.playback.set_volume(scale.value());
            });
        }
        for button in [&volume.mute, &volume.popover_mute] {
            let weak = weak.clone();
            button.connect_clicked(move |_| {
                if let Some(volume) = weak.upgrade() {
                    volume.playback.set_muted(!volume.playback.is_muted());
                }
            });
        }
        // Scrolling over the buttons changes the volume too.
        for target in [
            volume.mute.upcast_ref::<gtk::Widget>(),
            volume.menu.upcast_ref(),
        ] {
            let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            let weak = weak.clone();
            scroll.connect_scroll(move |_, _, dy| {
                if let Some(volume) = weak.upgrade() {
                    let value = (volume.playback.volume() - dy * VOLUME_STEP).clamp(0.0, 1.0);
                    if volume.playback.is_muted() {
                        volume.playback.set_muted(false);
                    }
                    volume.playback.set_volume(value);
                }
                glib::Propagation::Stop
            });
            target.add_controller(scroll);
        }
        volume.sync();
        volume
    }

    /// Breakpoint B: one button with a popover instead of the inline scale.
    pub fn set_compact(&self, compact: bool) {
        self.mute.set_visible(!compact);
        self.scale.set_visible(!compact);
        self.menu.set_visible(compact);
    }

    fn sync(&self) {
        let (volume, muted) = (self.playback.volume(), self.playback.is_muted());
        self.syncing.set(true);
        self.scale.set_value(volume);
        self.popover_scale.set_value(volume);
        self.syncing.set(false);
        let icon = if muted || volume <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if volume < 0.34 {
            "audio-volume-low-symbolic"
        } else if volume < 0.67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        let action = if muted { tr("Unmute") } else { tr("Mute") };
        for button in [&self.mute, &self.popover_mute] {
            button.set_icon_name(icon);
            button.set_tooltip_text(Some(action));
            button.update_property(&[gtk::accessible::Property::Label(action)]);
        }
        self.menu.set_icon_name(icon);
        let text = if muted {
            tr("Muted").to_owned()
        } else {
            format!("{} %", (volume * 100.0).round())
        };
        for scale in [&self.scale, &self.popover_scale] {
            scale.update_property(&[gtk::accessible::Property::ValueText(&text)]);
        }
    }
}
