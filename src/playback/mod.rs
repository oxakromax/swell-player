//! Everything about playing music: the queue, the GStreamer engine and the
//! glue that resolves TIDAL streams.
//!
//! [`Playback`] is the one object the UI talks to (main thread only). It
//! owns the queue and the engine and tells subscribers what changed through
//! [`Event`]s. The UI never touches GStreamer directly.
//!
//! Queue model (as in most players):
//! - the *context*: the list playback started from (album, playlist,
//!   favourites...), in play order (shuffled or not);
//! - the *up next* list: tracks added with "Play next" / "Add to queue",
//!   played before the context continues.
//!
//! [`Playback::upcoming`] is both, in the order they will play; indices in
//! the queue-editing methods refer to that list.

pub mod engine;
pub mod queue;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gtk::glib;

use crate::api::Api;
use crate::model::TrackInfo;
use crate::runtime;
use crate::settings::{self, Repeat};
use crate::tidal::pages::Target;

pub use engine::Stream;
use engine::{Engine, EngineEvent};
use queue::Queue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    /// Resolving the stream or buffering.
    Loading,
    Playing,
    Paused,
}

/// Where the current context came from, for "Playing from …".
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub title: String,
    pub target: Option<Target>,
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The current track changed (or playback ended: `None`).
    Track(Option<TrackInfo>),
    State(PlayState),
    /// What is up next changed.
    Queue,
    /// Shuffle, repeat or volume changed.
    Options,
    /// The format actually being played, e.g. "24-bit/96 kHz FLAC".
    Format(Option<String>),
    /// Something the user should hear about (a toast).
    Error(String),
}

type Subscriber = Rc<dyn Fn(&Event)>;

pub struct Playback {
    api: Arc<Api>,
    settings: Rc<settings::Store>,
    engine: Option<Engine>,
    queue: RefCell<Queue>,
    state: Cell<PlayState>,
    format: RefCell<Option<String>>,
    /// Bumped for every track load, so a late stream answer for a track
    /// the user already skipped is dropped.
    generation: Cell<u64>,
    subscribers: RefCell<Vec<(u64, Subscriber)>>,
    next_subscriber: Cell<u64>,
    requested_at: Cell<Option<Instant>>,
}

impl Playback {
    pub fn new(api: Arc<Api>, settings: Rc<settings::Store>) -> Rc<Self> {
        let current = settings.get();
        let this = Rc::new_cyclic(|weak: &std::rc::Weak<Self>| {
            let weak = weak.clone();
            let engine = Engine::new(move |event| {
                if let Some(this) = weak.upgrade() {
                    this.on_engine_event(event);
                }
            });
            let engine = match engine {
                Ok(engine) => Some(engine),
                Err(err) => {
                    log::warn!("playback unavailable: {err:#}");
                    None
                }
            };
            let mut queue = Queue::default();
            queue.set_shuffle(current.shuffle);
            queue.set_repeat(current.repeat);
            Self {
                api,
                settings: settings.clone(),
                engine,
                queue: RefCell::new(queue),
                state: Cell::new(PlayState::Stopped),
                format: RefCell::default(),
                generation: Cell::new(0),
                subscribers: RefCell::default(),
                next_subscriber: Cell::new(1),
                requested_at: Cell::new(None),
            }
        });
        this.apply_volume();
        this
    }

    /// Whether GStreamer (playbin3) is available at all.
    pub fn is_available(&self) -> bool {
        self.engine.is_some()
    }

    pub fn subscribe(&self, subscriber: impl Fn(&Event) + 'static) -> u64 {
        let id = self.next_subscriber.get();
        self.next_subscriber.set(id + 1);
        self.subscribers
            .borrow_mut()
            .push((id, Rc::new(subscriber)));
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        self.subscribers.borrow_mut().retain(|(other, _)| *other != id);
    }

    fn emit(&self, event: Event) {
        // Clone the list so a subscriber may (un)subscribe while handling.
        let subscribers: Vec<Subscriber> = self
            .subscribers
            .borrow()
            .iter()
            .map(|(_, s)| s.clone())
            .collect();
        for subscriber in subscribers {
            subscriber(&event);
        }
    }

    // ---- Starting playback ----

    /// Replaces the context with `tracks` and plays `tracks[start]`. With
    /// shuffle on, `start` plays first and the rest is shuffled.
    pub fn play_tracks(self: &Rc<Self>, tracks: Vec<TrackInfo>, start: usize, source: Source) {
        if tracks.is_empty() {
            return;
        }
        self.queue.borrow_mut().set_context(tracks, start, source);
        self.emit(Event::Queue);
        self.load_current(true);
    }

    pub fn play_next(&self, tracks: Vec<TrackInfo>) {
        self.queue.borrow_mut().play_next(tracks);
        self.emit(Event::Queue);
    }

    pub fn add_to_queue(&self, tracks: Vec<TrackInfo>) {
        self.queue.borrow_mut().add_to_queue(tracks);
        self.emit(Event::Queue);
    }

    // ---- Transport ----

    pub fn toggle(self: &Rc<Self>) {
        match self.state.get() {
            PlayState::Playing | PlayState::Loading => self.pause(),
            PlayState::Paused => self.play(),
            PlayState::Stopped => {
                if self.queue.borrow().current().is_some() {
                    self.load_current(true);
                }
            }
        }
    }

    pub fn play(self: &Rc<Self>) {
        match self.state.get() {
            PlayState::Paused => {
                if let Some(engine) = &self.engine {
                    engine.resume();
                }
            }
            PlayState::Stopped if self.queue.borrow().current().is_some() => {
                self.load_current(true)
            }
            _ => {}
        }
    }

    pub fn pause(&self) {
        if let Some(engine) = &self.engine {
            engine.pause();
        }
    }

    pub fn stop(&self) {
        self.generation.set(self.generation.get() + 1);
        if let Some(engine) = &self.engine {
            engine.stop();
        }
        self.set_state(PlayState::Stopped);
    }

    pub fn next(self: &Rc<Self>) {
        let advanced = self.queue.borrow_mut().advance(true);
        self.emit(Event::Queue);
        if advanced {
            self.load_current(true);
        } else {
            self.finish();
        }
    }

    /// Restarts the track if more than 3 s in, otherwise goes back one.
    pub fn previous(self: &Rc<Self>) {
        if self.position().is_some_and(|p| p > 3.0) {
            self.seek(0.0);
            return;
        }
        if self.queue.borrow_mut().go_back() {
            self.emit(Event::Queue);
            self.load_current(true);
        } else {
            self.seek(0.0);
        }
    }

    /// Seconds.
    pub fn seek(&self, position: f64) {
        if let Some(engine) = &self.engine {
            engine.seek(position.max(0.0));
        }
    }

    /// Seconds into the current track.
    pub fn position(&self) -> Option<f64> {
        self.engine.as_ref()?.position()
    }

    /// Length of the current track in seconds.
    pub fn duration(&self) -> Option<f64> {
        self.engine
            .as_ref()
            .and_then(Engine::duration)
            .or_else(|| self.current().map(|t| f64::from(t.duration)))
    }

    // ---- Options ----

    /// Linear 0.0–1.0.
    pub fn volume(&self) -> f64 {
        self.settings.get().volume
    }

    pub fn set_volume(&self, volume: f64) {
        self.settings
            .update(|s| s.volume = volume.clamp(0.0, 1.0));
        self.apply_volume();
        self.emit(Event::Options);
    }

    pub fn is_muted(&self) -> bool {
        self.settings.get().muted
    }

    pub fn set_muted(&self, muted: bool) {
        self.settings.update(|s| s.muted = muted);
        self.apply_volume();
        self.emit(Event::Options);
    }

    fn apply_volume(&self) {
        let settings = self.settings.get();
        if let Some(engine) = &self.engine {
            engine.set_volume(if settings.muted { 0.0 } else { settings.volume });
        }
    }

    pub fn shuffle(&self) -> bool {
        self.queue.borrow().shuffle()
    }

    pub fn set_shuffle(&self, shuffle: bool) {
        self.queue.borrow_mut().set_shuffle(shuffle);
        self.settings.update(|s| s.shuffle = shuffle);
        self.emit(Event::Options);
        self.emit(Event::Queue);
    }

    pub fn repeat(&self) -> Repeat {
        self.queue.borrow().repeat()
    }

    pub fn set_repeat(&self, repeat: Repeat) {
        self.queue.borrow_mut().set_repeat(repeat);
        self.settings.update(|s| s.repeat = repeat);
        self.emit(Event::Options);
    }

    // ---- What is playing ----

    pub fn current(&self) -> Option<TrackInfo> {
        self.queue.borrow().current().cloned()
    }

    pub fn state(&self) -> PlayState {
        self.state.get()
    }

    pub fn format(&self) -> Option<String> {
        self.format.borrow().clone()
    }

    pub fn source(&self) -> Option<Source> {
        self.queue.borrow().source().cloned()
    }

    /// What will play after the current track, in order.
    pub fn upcoming(&self) -> Vec<TrackInfo> {
        self.queue.borrow().upcoming()
    }

    /// Number of up-next tracks (the first entries of [`Self::upcoming`]).
    pub fn up_next_len(&self) -> usize {
        self.queue.borrow().up_next_len()
    }

    /// Recently played, oldest first.
    pub fn history(&self) -> Vec<TrackInfo> {
        self.queue.borrow().history()
    }

    // ---- Editing the queue (indices into `upcoming`) ----

    pub fn jump_to(self: &Rc<Self>, index: usize) {
        if self.queue.borrow_mut().jump_to(index) {
            self.emit(Event::Queue);
            self.load_current(true);
        }
    }

    pub fn remove(&self, index: usize) {
        self.queue.borrow_mut().remove(index);
        self.emit(Event::Queue);
    }

    pub fn move_item(&self, from: usize, to: usize) {
        self.queue.borrow_mut().move_item(from, to);
        self.emit(Event::Queue);
    }

    pub fn clear_upcoming(&self) {
        self.queue.borrow_mut().clear_upcoming();
        self.emit(Event::Queue);
    }

    // ---- Internals ----

    fn set_state(&self, state: PlayState) {
        if self.state.replace(state) != state {
            self.emit(Event::State(state));
        }
    }

    fn finish(&self) {
        self.stop();
        self.format.replace(None);
        self.emit(Event::Format(None));
    }

    /// Resolves the current track's stream and plays it.
    fn load_current(self: &Rc<Self>, autoplay: bool) {
        let Some(track) = self.current() else {
            self.finish();
            return self.emit(Event::Track(None));
        };
        self.emit(Event::Track(Some(track.clone())));
        let Some(engine) = &self.engine else {
            return self.emit(Event::Error(
                "Playback needs GStreamer's playbin3".to_owned(),
            ));
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.requested_at.set(Some(Instant::now()));
        engine.stop();
        self.set_state(PlayState::Loading);

        let this = self.clone();
        let api = self.api.clone();
        glib::spawn_future_local(async move {
            let id = track.id;
            let stream = runtime::spawn(async move { api.stream(id).await }).await;
            if this.generation.get() != generation {
                return;
            }
            match stream {
                Ok(stream) => {
                    log::info!("playing track {id} as {}", stream.format);
                    this.format.replace(Some(stream.format.clone()));
                    this.emit(Event::Format(Some(stream.format.clone())));
                    if let Some(engine) = &this.engine {
                        engine.load(&stream.uri, autoplay);
                    }
                }
                Err(err) => {
                    log::warn!("could not play track {id}: {err:#}");
                    this.set_state(PlayState::Stopped);
                    this.emit(Event::Error(format!("Could not play “{}”", track.title)));
                }
            }
        });
    }

    fn on_engine_event(self: &Rc<Self>, event: EngineEvent) {
        match event {
            EngineEvent::Playing => {
                if let Some(at) = self.requested_at.take() {
                    log::info!(
                        "playback started {} ms after the track was chosen",
                        at.elapsed().as_millis()
                    );
                }
                self.set_state(PlayState::Playing);
            }
            EngineEvent::Paused => self.set_state(PlayState::Paused),
            EngineEvent::Buffering => self.set_state(PlayState::Loading),
            EngineEvent::Finished => self.next(),
            EngineEvent::Error(message) => {
                self.set_state(PlayState::Stopped);
                self.emit(Event::Error(format!("Playback failed: {message}")));
            }
        }
    }
}
