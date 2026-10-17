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
//!
//! What the shell has to do (once per signed-in session):
//! - [`Playback::attach`] with the application, after creating it: enables
//!   track notifications, GTK's suspend inhibition, MPRIS Raise/Quit
//!   (override those with [`Playback::set_app_hooks`]) and saving the
//!   session at shutdown;
//! - [`Playback::restore_session`] once the player bar is subscribed: the
//!   last queue comes back paused (when the preference is on);
//! - [`Playback::save_session`] before signing out (quitting is covered).
//!
//! MPRIS (`org.mpris.MediaPlayer2.SwellPlayer`) and the logind sleep lock
//! while playing work by themselves.
//!
//! Debugging: `SWELL_PLAY=<track id>[,<track id>…][@<seconds>]` plays those
//! tracks at startup, starting at `seconds` (negative: from the end);
//! `SWELL_PLAY=restore` restores the saved queue and plays it.

pub mod engine;
pub mod format;
mod mpris;
mod notify;
pub mod queue;
mod session;
mod sleep;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::glib;

use crate::api::Api;
use crate::model::TrackInfo;
use crate::runtime;
use crate::settings::{self, Normalization, QualityPreference, Repeat};
use crate::tidal::pages::Target;

pub use engine::Stream;
use engine::{Engine, EngineEvent, NextStream};
#[allow(unused_imports)] // Public API for the UI.
pub use format::{Codec, FormatDetails};
use queue::Queue;
pub use sleep::SleepTimer;

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
    /// The current track changed (or playback ended: `None`). With gapless
    /// playback this comes when the next track actually starts.
    Track(Option<TrackInfo>),
    State(PlayState),
    /// What is up next changed.
    Queue,
    /// Shuffle, repeat or volume changed.
    Options,
    /// The format actually being played, e.g. "24-bit/96 kHz FLAC"; see
    /// [`Playback::format_details`] for the same as data. Also sent when
    /// only the details changed (normalisation gain).
    Format(Option<String>),
    /// Something the user should hear about (a toast).
    Error(String),
    /// The sleep timer was set, cleared or fired, or the whole minutes it
    /// has left changed: see [`Playback::sleep_timer`] and
    /// [`Playback::sleep_timer_remaining`].
    SleepTimer,
    /// The position jumped (a seek), in seconds.
    Seeked(f64),
}

type Subscriber = Rc<dyn Fn(&Event)>;

/// Resolve the next track's stream this long before the current one ends
/// (demuxers finish reading, and playbin3 asks for the next URI, up to
/// ~30 s before the end).
const PREFETCH_BEFORE_END: f64 = 60.0;
/// Signed stream URLs expire; older prepared streams are resolved again.
const PREPARED_MAX_AGE: Duration = Duration::from_secs(20 * 60);
/// A track that played this long since (re)loading counts as healthy.
const HEALTHY_AFTER: f64 = 5.0;
/// Consecutive tracks that failed before playback stops altogether.
const MAX_FAILED_TRACKS: u32 = 3;
const SAVE_DELAY: Duration = Duration::from_secs(2);
const POSITION_SAVE_INTERVAL: Duration = Duration::from_secs(10);

/// A stream resolved ahead for gapless playback.
struct Prepared {
    stream: Rc<Stream>,
    at: Instant,
}

impl Prepared {
    fn is_fresh(&self) -> bool {
        self.at.elapsed() < PREPARED_MAX_AGE
    }
}

pub struct Playback {
    api: Arc<Api>,
    settings: Rc<settings::Store>,
    engine: Option<Engine>,
    weak_self: Weak<Self>,
    queue: RefCell<Queue>,
    state: Cell<PlayState>,
    format: RefCell<Option<String>>,
    details: RefCell<Option<FormatDetails>>,
    /// Bumped for every track load, so a late stream answer for a track
    /// the user already skipped is dropped.
    generation: Cell<u64>,
    subscribers: RefCell<Vec<(u64, Subscriber)>>,
    next_subscriber: Cell<u64>,
    requested_at: Cell<Option<Instant>>,

    /// Play (true) or stay paused once the stream is ready.
    want_play: Cell<bool>,
    /// The engine has the current track's stream.
    loaded: Cell<bool>,
    /// A stream lookup for the current track is in flight.
    resolving: Cell<bool>,
    /// The current track is known (restored, or stopped by the sleep timer)
    /// but not loaded: playing loads it at `resume_at`.
    unloaded: Cell<bool>,
    resume_at: Cell<Option<f64>>,
    /// The stream of the current track.
    current_stream: RefCell<Option<Rc<Stream>>>,
    /// The track the last load was for (to count failures per track).
    loaded_track: Cell<Option<u64>>,
    last_position: Cell<f64>,
    /// The engine's duration of the current track, as last known.
    engine_duration: Cell<Option<f64>>,
    healthy_at: Cell<f64>,
    /// Failed attempts at the current track.
    failures: Cell<u32>,
    /// Tracks skipped in a row after failing.
    failed_tracks: Cell<u32>,
    log_output: Cell<bool>,

    prepared: RefCell<Option<Prepared>>,
    /// Track whose stream is being resolved ahead.
    preparing: Cell<Option<u64>>,
    prepare_generation: Cell<u64>,
    /// Do not keep retrying a track whose prefetch failed.
    prefetch_failed: Cell<Option<u64>>,

    normalization: Cell<Normalization>,
    quality: Cell<QualityPreference>,
    gapless: Cell<bool>,
    applied_volume: Cell<Option<(f64, bool)>>,

    sleep: RefCell<Option<sleep::Armed>>,
    desktop: notify::Desktop,
    mpris: Option<mpris::Mpris>,
    save_pending: Cell<bool>,
    position_saved_at: Cell<Instant>,
    settings_listener: Cell<Option<u64>>,
    tick: RefCell<Option<glib::SourceId>>,
}

impl Playback {
    pub fn new(api: Arc<Api>, settings: Rc<settings::Store>) -> Rc<Self> {
        let current = settings.get();
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let this = Rc::new_cyclic(|weak: &Weak<Self>| {
            let engine_weak = weak.clone();
            let engine = Engine::new(move |event| {
                if let Some(this) = engine_weak.upgrade() {
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
                weak_self: weak.clone(),
                queue: RefCell::new(queue),
                state: Cell::new(PlayState::Stopped),
                format: RefCell::default(),
                details: RefCell::default(),
                generation: Cell::new(0),
                subscribers: RefCell::default(),
                next_subscriber: Cell::new(1),
                requested_at: Cell::new(None),
                want_play: Cell::new(false),
                loaded: Cell::new(false),
                resolving: Cell::new(false),
                unloaded: Cell::new(false),
                resume_at: Cell::new(None),
                current_stream: RefCell::default(),
                loaded_track: Cell::new(None),
                last_position: Cell::new(0.0),
                engine_duration: Cell::new(None),
                healthy_at: Cell::new(HEALTHY_AFTER),
                failures: Cell::new(0),
                failed_tracks: Cell::new(0),
                log_output: Cell::new(false),
                prepared: RefCell::default(),
                preparing: Cell::new(None),
                prepare_generation: Cell::new(0),
                prefetch_failed: Cell::new(None),
                normalization: Cell::new(current.normalization),
                quality: Cell::new(current.quality),
                gapless: Cell::new(current.gapless),
                applied_volume: Cell::new(None),
                sleep: RefCell::default(),
                desktop: notify::Desktop::default(),
                mpris: Some(mpris::Mpris::start(commands)),
                save_pending: Cell::new(false),
                position_saved_at: Cell::new(Instant::now()),
                settings_listener: Cell::new(None),
                tick: RefCell::default(),
            }
        });
        this.apply_volume();

        let weak = Rc::downgrade(&this);
        let listener = settings.connect_changed(move |settings| {
            if let Some(this) = weak.upgrade() {
                this.on_settings_changed(settings);
            }
        });
        this.settings_listener.set(Some(listener));

        let weak = Rc::downgrade(&this);
        glib::spawn_future_local(async move {
            while let Some(command) = receiver.recv().await {
                let Some(this) = weak.upgrade() else { break };
                this.on_command(command);
            }
        });

        let weak = Rc::downgrade(&this);
        let tick = glib::timeout_add_local(Duration::from_secs(1), move || match weak.upgrade() {
            Some(this) => {
                this.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        this.tick.replace(Some(tick));
        this.sync_mpris();
        this.debug_autoplay();
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
        self.subscribers
            .borrow_mut()
            .retain(|(other, _)| *other != id);
    }

    fn emit(&self, event: Event) {
        self.integrate(&event);
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

    fn this(&self) -> Option<Rc<Self>> {
        self.weak_self.upgrade()
    }

    // ---- Desktop integration ----

    /// Connects playback to the application: suspend is inhibited while
    /// playing, a notification shows new tracks while the window is not
    /// active (if enabled in the preferences), MPRIS Raise/Quit present
    /// the active window / quit the application, and the session is saved
    /// when the application shuts down.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn attach(&self, app: &adw::Application) {
        let weak = self.weak_self.clone();
        self.desktop.attach(app, move || {
            if let Some(this) = weak.upgrade() {
                this.save_session();
            }
        });
        self.desktop
            .set_playing(self.state.get() == PlayState::Playing);
        self.sync_mpris();
    }

    /// Replaces what MPRIS Raise and Quit do (by default: present the
    /// active window, quit the application).
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn set_app_hooks(&self, raise: impl Fn() + 'static, quit: impl Fn() + 'static) {
        self.desktop.set_hooks(Rc::new(raise), Rc::new(quit));
        self.sync_mpris();
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
        self.load_current(true, None, true);
    }

    pub fn play_next(&self, tracks: Vec<TrackInfo>) {
        self.queue.borrow_mut().play_next(tracks);
        self.queue_changed();
    }

    pub fn add_to_queue(&self, tracks: Vec<TrackInfo>) {
        self.queue.borrow_mut().add_to_queue(tracks);
        self.queue_changed();
    }

    // ---- Transport ----

    pub fn toggle(self: &Rc<Self>) {
        match self.state.get() {
            PlayState::Playing | PlayState::Loading => self.pause(),
            PlayState::Paused | PlayState::Stopped => self.play(),
        }
    }

    pub fn play(self: &Rc<Self>) {
        if self.unloaded.get() {
            let start = self.resume_at.get();
            return self.load_current(true, start, false);
        }
        match self.state.get() {
            PlayState::Paused if self.loaded.get() => {
                self.want_play.set(true);
                if let Some(engine) = &self.engine {
                    engine.resume();
                }
            }
            PlayState::Paused if self.resolving.get() => {
                self.want_play.set(true);
                self.set_state(PlayState::Loading);
            }
            PlayState::Paused | PlayState::Stopped if self.queue.borrow().current().is_some() => {
                let start =
                    (self.state.get() == PlayState::Paused).then(|| self.last_position.get());
                self.load_current(true, start, self.state.get() == PlayState::Stopped);
            }
            _ => {}
        }
    }

    pub fn pause(&self) {
        self.want_play.set(false);
        if self.loaded.get() {
            if let Some(engine) = &self.engine {
                engine.pause();
            }
        } else if self.state.get() == PlayState::Loading {
            self.set_state(PlayState::Paused);
        }
    }

    pub fn stop(&self) {
        self.generation.set(self.generation.get() + 1);
        if let Some(engine) = &self.engine {
            engine.stop();
        }
        self.loaded.set(false);
        self.resolving.set(false);
        self.unloaded.set(false);
        self.want_play.set(false);
        self.drop_prepared();
        self.set_state(PlayState::Stopped);
    }

    pub fn next(self: &Rc<Self>) {
        let advanced = self.queue.borrow_mut().advance(true);
        self.emit(Event::Queue);
        if advanced {
            self.load_current(true, None, true);
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
            self.load_current(true, None, true);
        } else {
            self.seek(0.0);
        }
    }

    /// Seconds. Seeking to the end goes to the next track.
    pub fn seek(&self, position: f64) {
        let position = position.max(0.0);
        if self.unloaded.get() {
            self.resume_at.set(Some(position));
            self.last_position.set(position);
            self.emit(Event::Seeked(position));
            return;
        }
        let Some(engine) = &self.engine else {
            return;
        };
        if !self.loaded.get() {
            return;
        }
        if let Some(duration) = self.duration().filter(|d| *d > 1.0)
            && position >= duration - 0.5
        {
            if let Some(this) = self.this() {
                this.next();
            }
            return;
        }
        if engine.handover_pending() {
            // The next track is already decoding behind the end of this
            // one: start this one over at the position instead.
            if let Some(stream) = self.current_stream.borrow().clone() {
                let gain = self.gain_for(&stream);
                engine.load(&stream.uri, self.want_play.get(), Some(position), gain);
                if let Some(this) = self.this() {
                    this.refresh_next();
                }
            }
        } else {
            engine.seek(position);
        }
        self.last_position.set(position);
        self.emit(Event::Seeked(position));
    }

    /// Seconds into the current track.
    pub fn position(&self) -> Option<f64> {
        if self.unloaded.get() {
            return self.resume_at.get();
        }
        self.engine.as_ref()?.position()
    }

    /// Length of the current track in seconds.
    pub fn duration(&self) -> Option<f64> {
        let engine = self
            .engine
            .as_ref()
            .filter(|_| self.loaded.get() && !self.unloaded.get());
        if let Some(engine) = engine {
            // Once the next track is handed over, playbin3 reports its
            // duration while the end of this one still plays.
            if engine.handover_pending() {
                if let Some(duration) = self.engine_duration.get() {
                    return Some(duration);
                }
            } else if let Some(duration) = engine.duration() {
                self.engine_duration.set(Some(duration));
                return Some(duration);
            }
        }
        self.current().map(|t| f64::from(t.duration))
    }

    // ---- Options ----

    /// Linear 0.0–1.0 (the slider position; the output follows a cubic,
    /// perceptual curve).
    pub fn volume(&self) -> f64 {
        self.settings.get().volume
    }

    pub fn set_volume(&self, volume: f64) {
        self.settings.update(|s| s.volume = volume.clamp(0.0, 1.0));
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
        let wanted = (settings.volume, settings.muted);
        if self.applied_volume.replace(Some(wanted)) == Some(wanted) {
            return;
        }
        if let Some(engine) = &self.engine {
            engine.set_volume(format::volume_curve(settings.volume));
            engine.set_muted(settings.muted);
        }
    }

    pub fn shuffle(&self) -> bool {
        self.queue.borrow().shuffle()
    }

    pub fn set_shuffle(&self, shuffle: bool) {
        self.queue.borrow_mut().set_shuffle(shuffle);
        self.settings.update(|s| s.shuffle = shuffle);
        self.emit(Event::Options);
        self.queue_changed();
    }

    pub fn repeat(&self) -> Repeat {
        self.queue.borrow().repeat()
    }

    pub fn set_repeat(&self, repeat: Repeat) {
        self.queue.borrow_mut().set_repeat(repeat);
        self.settings.update(|s| s.repeat = repeat);
        self.emit(Event::Options);
        if let Some(this) = self.this() {
            this.refresh_next();
        }
    }

    // ---- Sleep timer ----

    /// Starts (or with `None` cancels) the sleep timer.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn set_sleep_timer(&self, timer: Option<SleepTimer>) {
        let now = Instant::now();
        let armed = timer.map(|timer| {
            let mut armed = sleep::Armed::new(timer, now);
            armed.announced = armed.remaining(now).map(sleep::minutes_left);
            armed
        });
        self.sleep.replace(armed);
        if let Some(this) = self.this() {
            // "End of track" must not hand the next track over.
            this.refresh_next();
        }
        self.emit(Event::SleepTimer);
    }

    /// The running sleep timer, as it was set.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn sleep_timer(&self) -> Option<SleepTimer> {
        self.sleep.borrow().as_ref().map(|armed| armed.timer)
    }

    /// Time until the timer stops playback (for "end of track", what is
    /// left of the track).
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn sleep_timer_remaining(&self) -> Option<Duration> {
        let timer = self.sleep_timer()?;
        match timer {
            SleepTimer::After(_) => self
                .sleep
                .borrow()
                .as_ref()
                .and_then(|armed| armed.remaining(Instant::now())),
            SleepTimer::EndOfTrack => {
                let left = self.duration()? - self.position().unwrap_or(0.0);
                Some(Duration::from_secs_f64(left.max(0.0)))
            }
        }
    }

    fn check_sleep_timer(&self) {
        let now = Instant::now();
        let (fire, announce) = {
            let mut timer = self.sleep.borrow_mut();
            let Some(armed) = timer.as_mut() else {
                return;
            };
            match armed.remaining(now) {
                Some(left) if left.is_zero() => (true, false),
                Some(left) => {
                    let minutes = sleep::minutes_left(left);
                    (false, armed.announced.replace(minutes) != Some(minutes))
                }
                None => (false, false),
            }
        };
        if fire {
            log::info!("sleep timer: pausing");
            self.sleep.replace(None);
            self.pause();
            self.emit(Event::SleepTimer);
        } else if announce {
            self.emit(Event::SleepTimer);
        }
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

    /// The delivered format as data (codec, bit depth, sample rate,
    /// bitrate, TIDAL tier, applied ReplayGain), for the quality popover.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn format_details(&self) -> Option<FormatDetails> {
        self.details.borrow().clone()
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
            self.load_current(true, None, true);
        }
    }

    pub fn remove(&self, index: usize) {
        self.queue.borrow_mut().remove(index);
        self.queue_changed();
    }

    pub fn move_item(&self, from: usize, to: usize) {
        self.queue.borrow_mut().move_item(from, to);
        self.queue_changed();
    }

    pub fn clear_upcoming(&self) {
        self.queue.borrow_mut().clear_upcoming();
        self.queue_changed();
    }

    fn queue_changed(&self) {
        self.emit(Event::Queue);
        if let Some(this) = self.this() {
            this.refresh_next();
        }
    }

    // ---- Session ----

    /// Writes the queue and position to disk now. It is also saved by
    /// itself as things change, and at shutdown once attached.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn save_session(&self) {
        if !self.settings.get().restore_session {
            return;
        }
        if let Some(saved) = self.saved_state()
            && let Err(err) = session::write(&saved)
        {
            log::warn!("could not save the queue: {err:#}");
        }
    }

    /// Brings back the last saved queue, paused at the saved position,
    /// when the preference is on and nothing plays yet. The stream is only
    /// resolved when playback starts.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn restore_session(&self) {
        if !self.settings.get().restore_session {
            return;
        }
        let weak = self.weak_self.clone();
        glib::spawn_future_local(async move {
            let saved = runtime::spawn(session::read()).await;
            let Some(this) = weak.upgrade() else { return };
            match saved {
                Ok(Some(saved)) => this.apply_saved(saved),
                Ok(None) => {}
                Err(err) => log::warn!("could not read the saved queue: {err:#}"),
            }
        });
    }

    fn apply_saved(&self, saved: session::Saved) {
        if self.queue.borrow().current().is_some() || self.state.get() != PlayState::Stopped {
            return;
        }
        let queue = Queue::restore(saved.queue, saved.source.map(Source::from));
        let (shuffle, repeat) = (queue.shuffle(), queue.repeat());
        self.queue.replace(queue);
        self.settings.update(|s| {
            s.shuffle = shuffle;
            s.repeat = repeat;
        });
        self.unloaded.set(true);
        self.resume_at.set(Some(saved.position));
        self.last_position.set(saved.position);
        log::info!("restored the queue at {:.0} s", saved.position);
        self.emit(Event::Queue);
        self.emit(Event::Options);
        self.emit(Event::Track(self.current()));
        self.set_state(PlayState::Paused);
    }

    fn saved_state(&self) -> Option<session::Saved> {
        let queue = self.queue.borrow();
        queue.current()?;
        let position = self.position().unwrap_or(self.last_position.get());
        Some(session::Saved::new(
            queue.snapshot(),
            queue.source(),
            position,
        ))
    }

    fn schedule_save(&self) {
        if self.save_pending.replace(true) {
            return;
        }
        let weak = self.weak_self.clone();
        glib::timeout_add_local_once(SAVE_DELAY, move || {
            let Some(this) = weak.upgrade() else { return };
            this.save_pending.set(false);
            if !this.settings.get().restore_session {
                return;
            }
            if let Some(saved) = this.saved_state() {
                this.position_saved_at.set(Instant::now());
                runtime::runtime().spawn_blocking(move || {
                    if let Err(err) = session::write(&saved) {
                        log::warn!("could not save the queue: {err:#}");
                    }
                });
            }
        });
    }

    fn save_position(&self, position: f64) {
        self.position_saved_at.set(Instant::now());
        let Some(track) = self.current() else { return };
        if !self.settings.get().restore_session {
            return;
        }
        let saved = session::SavedPosition {
            track_id: track.id,
            position,
        };
        runtime::runtime().spawn_blocking(move || {
            if let Err(err) = session::write_position(&saved) {
                log::debug!("could not save the position: {err:#}");
            }
        });
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
        self.details.replace(None);
        self.current_stream.replace(None);
        self.emit(Event::Format(None));
    }

    /// What follows the event, besides telling subscribers.
    fn integrate(&self, event: &Event) {
        match event {
            Event::State(state) => self.desktop.set_playing(*state == PlayState::Playing),
            Event::Track(_) | Event::Queue | Event::Options => self.schedule_save(),
            Event::Seeked(position) => {
                if let Some(mpris) = &self.mpris {
                    mpris.seeked(*position);
                }
            }
            _ => {}
        }
        if !matches!(event, Event::Seeked(_) | Event::Error(_)) {
            self.sync_mpris();
        }
    }

    /// Announces a track that starts playing now.
    fn announce(&self, track: &TrackInfo) {
        self.emit(Event::Track(Some(track.clone())));
        if self.settings.get().notifications {
            self.desktop.track_changed(track);
        }
    }

    fn sync_mpris(&self) {
        let Some(mpris) = &self.mpris else { return };
        let queue = self.queue.borrow();
        let current = queue.current();
        let status = match self.state.get() {
            PlayState::Playing => mpris::Status::Playing,
            PlayState::Loading if self.want_play.get() => mpris::Status::Playing,
            PlayState::Loading | PlayState::Paused => mpris::Status::Paused,
            PlayState::Stopped => mpris::Status::Stopped,
        };
        let settings = self.settings.get();
        let snapshot = mpris::Snapshot {
            status,
            repeat: queue.repeat(),
            shuffle: queue.shuffle(),
            volume: settings.volume,
            track: current.map(|track| mpris::Metadata {
                track_id: track.id,
                title: track.title.clone(),
                artists: track.artists.iter().map(|a| a.name.clone()).collect(),
                album: track.album.clone(),
                length: f64::from(track.duration),
                art_url: track.cover.as_deref().map(|c| notify::cover_url(c, 640)),
                url: track.share_url(),
                track_number: track.track_number,
                disc_number: track.volume_number,
            }),
            position: self.position().unwrap_or(0.0),
            position_at: Some(Instant::now()),
            can_go_next: queue.can_advance(),
            can_go_previous: current.is_some(),
            can_play: current.is_some(),
            can_seek: current.is_some(),
            can_raise: self.desktop.can_raise(),
            can_quit: self.desktop.can_quit(),
        };
        drop(queue);
        mpris.update(snapshot);
    }

    fn on_command(self: &Rc<Self>, command: mpris::Command) {
        use mpris::Command;
        match command {
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::PlayPause => self.toggle(),
            Command::Stop => self.stop(),
            Command::Next => self.next(),
            Command::Previous => self.previous(),
            Command::Seek(offset) => {
                if let Some(position) = self.position() {
                    self.seek(position + offset);
                }
            }
            Command::SetPosition { track_id, position } => {
                let current = self.current().is_some_and(|t| t.id == track_id);
                if current && self.duration().is_none_or(|d| position <= d) {
                    self.seek(position);
                }
            }
            Command::Volume(volume) => self.set_volume(volume),
            Command::Shuffle(shuffle) => self.set_shuffle(shuffle),
            Command::Repeat(repeat) => self.set_repeat(repeat),
            Command::Raise => self.desktop.raise(),
            Command::Quit => self.desktop.quit(),
        }
    }

    fn on_settings_changed(self: &Rc<Self>, settings: &settings::Settings) {
        self.apply_volume();
        if self.normalization.replace(settings.normalization) != settings.normalization {
            self.apply_normalization();
        }
        if self.quality.replace(settings.quality) != settings.quality {
            // The current track keeps playing; the next one uses it.
            self.drop_prepared();
            self.refresh_next();
        }
        if self.gapless.replace(settings.gapless) != settings.gapless {
            self.refresh_next();
        }
    }

    fn quality(&self) -> crate::tidal::Quality {
        format::requested_quality(self.quality.get(), self.api.flow.max_quality())
    }

    fn gain_for(&self, stream: &Stream) -> Option<f64> {
        stream.replay_gain.factor(self.normalization.get())
    }

    /// Applies a normalisation change to what is playing: a gain change is
    /// live; adding or removing the filter restarts the pipeline at the
    /// same position.
    fn apply_normalization(self: &Rc<Self>) {
        let (Some(engine), Some(stream)) = (&self.engine, self.current_stream.borrow().clone())
        else {
            return;
        };
        let gain = self.gain_for(&stream);
        if let Some(details) = self.details.borrow_mut().as_mut() {
            details.replay_gain_db = gain.map(format::to_db);
        }
        if self.loaded.get() {
            if engine.has_filter() == gain.is_some() {
                if let Some(gain) = gain {
                    engine.set_gain(gain);
                }
            } else {
                let position = self.position().unwrap_or(self.last_position.get());
                engine.load(&stream.uri, self.want_play.get(), Some(position), gain);
            }
            self.refresh_next();
        }
        self.emit(Event::Format(self.format()));
    }

    fn drop_prepared(&self) {
        self.prepared.replace(None);
        self.preparing.set(None);
        self.prefetch_failed.set(None);
        self.prepare_generation
            .set(self.prepare_generation.get() + 1);
        if let Some(engine) = &self.engine
            && !engine.handover_pending()
        {
            engine.set_next(None);
        }
    }

    fn take_prepared(&self, track_id: u64) -> Option<Rc<Stream>> {
        let mut prepared = self.prepared.borrow_mut();
        let usable = prepared
            .as_ref()
            .is_some_and(|p| p.stream.track_id == track_id && p.is_fresh());
        if usable {
            prepared.take().map(|p| p.stream)
        } else {
            None
        }
    }

    /// The track to prepare for a gapless switch, if any.
    fn wanted_next(&self) -> Option<u64> {
        let end_of_track = self.sleep_timer() == Some(SleepTimer::EndOfTrack);
        if !self.gapless.get() || !self.loaded.get() || end_of_track {
            return None;
        }
        self.queue.borrow().peek_next().map(|t| t.id)
    }

    /// Keeps the stream handed to the engine for the gapless switch in line
    /// with the queue, and prepares one when it is time.
    fn refresh_next(self: &Rc<Self>) {
        let Some(engine) = &self.engine else { return };
        if engine.handover_pending() {
            // Too late to change; `NextStarted` sorts out a mismatch.
            return;
        }
        let wanted = self.wanted_next();
        let stale = self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| Some(p.stream.track_id) != wanted || !p.is_fresh());
        if stale {
            self.prepared.replace(None);
        }
        if self.preparing.get().is_some() && self.preparing.get() != wanted {
            self.preparing.set(None);
            self.prepare_generation
                .set(self.prepare_generation.get() + 1);
        }
        let next = self.prepared.borrow().as_ref().map(|p| NextStream {
            track_id: p.stream.track_id,
            uri: p.stream.uri.clone(),
            gain: self.gain_for(&p.stream),
        });
        engine.set_next(next);
        self.keep_manifests();
        self.maybe_prefetch();
    }

    fn keep_manifests(&self) {
        let ids: Vec<u64> = [
            self.current().map(|t| t.id),
            self.prepared.borrow().as_ref().map(|p| p.stream.track_id),
            self.preparing.get(),
        ]
        .into_iter()
        .flatten()
        .collect();
        engine::keep_manifests(&ids);
    }

    fn maybe_prefetch(self: &Rc<Self>) {
        let Some(next_id) = self.wanted_next() else {
            return;
        };
        let ready = self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| p.stream.track_id == next_id);
        if ready || self.preparing.get().is_some() || self.prefetch_failed.get() == Some(next_id) {
            return;
        }
        let Some(duration) = self.duration() else {
            return;
        };
        let position = self.position().unwrap_or(0.0);
        if duration - position > PREFETCH_BEFORE_END {
            return;
        }
        let generation = self.prepare_generation.get() + 1;
        self.prepare_generation.set(generation);
        self.preparing.set(Some(next_id));
        self.keep_manifests();
        let api = self.api.clone();
        let quality = self.quality();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result =
                runtime::spawn(async move { api.stream_with_quality(next_id, quality).await })
                    .await;
            if this.prepare_generation.get() != generation {
                return;
            }
            this.preparing.set(None);
            match result {
                Ok(stream) => {
                    log::info!(
                        "prepared track {next_id} ({}) for gapless playback",
                        stream.format
                    );
                    this.prepared.replace(Some(Prepared {
                        stream: Rc::new(stream),
                        at: Instant::now(),
                    }));
                    this.refresh_next();
                }
                Err(err) => {
                    log::warn!("could not prepare track {next_id}: {err:#}");
                    this.prefetch_failed.set(Some(next_id));
                }
            }
        });
    }

    /// Resolves the current track's stream and plays it from `start`.
    /// `announce`: the track is new (not a reload of the same one).
    fn load_current(self: &Rc<Self>, autoplay: bool, start: Option<f64>, announce: bool) {
        let skipped = self.queue.borrow_mut().skip_unavailable();
        if skipped > 0 {
            self.emit(Event::Queue);
            self.emit(Event::Error(if skipped == 1 {
                "Skipped a track that is not available".to_owned()
            } else {
                format!("Skipped {skipped} tracks that are not available")
            }));
        }
        let Some(track) = self.current() else {
            self.finish();
            return self.emit(Event::Track(None));
        };
        if self.loaded_track.replace(Some(track.id)) != Some(track.id) {
            self.failures.set(0);
            self.prefetch_failed.set(None);
        }
        self.unloaded.set(false);
        self.resume_at.set(None);
        self.want_play.set(autoplay);
        self.loaded.set(false);
        if announce || skipped > 0 {
            self.announce(&track);
        }
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

        if let Some(stream) = self.take_prepared(track.id) {
            return self.start_stream(stream, start);
        }
        self.keep_manifests();
        self.resolving.set(true);
        let this = self.clone();
        let api = self.api.clone();
        let quality = self.quality();
        glib::spawn_future_local(async move {
            let id = track.id;
            let stream =
                runtime::spawn(async move { api.stream_with_quality(id, quality).await }).await;
            if this.generation.get() != generation {
                return;
            }
            this.resolving.set(false);
            match stream {
                Ok(stream) => this.start_stream(Rc::new(stream), start),
                Err(err) => this.load_failed(&track, &format!("{err:#}"), start),
            }
        });
    }

    fn start_stream(self: &Rc<Self>, stream: Rc<Stream>, start: Option<f64>) {
        let Some(engine) = &self.engine else { return };
        log::info!("playing track {} as {}", stream.track_id, stream.format);
        let gain = self.gain_for(&stream);
        self.set_current_stream(stream.clone(), gain);
        self.loaded.set(true);
        self.log_output.set(true);
        let start = start.filter(|s| *s > 0.0);
        self.last_position.set(start.unwrap_or(0.0));
        self.healthy_at.set(start.unwrap_or(0.0) + HEALTHY_AFTER);
        engine.load(&stream.uri, self.want_play.get(), start, gain);
        self.refresh_next();
    }

    fn set_current_stream(&self, stream: Rc<Stream>, gain: Option<f64>) {
        let mut details = stream.details.clone();
        details.replay_gain_db = gain.map(format::to_db);
        self.details.replace(Some(details));
        self.format.replace(Some(stream.format.clone()));
        self.engine_duration.set(None);
        let format = stream.format.clone();
        self.current_stream.replace(Some(stream));
        self.emit(Event::Format(Some(format)));
    }

    /// The stream could not be resolved: retry once, then skip.
    fn load_failed(self: &Rc<Self>, track: &TrackInfo, error: &str, start: Option<f64>) {
        log::warn!("could not play track {}: {error}", track.id);
        let failures = self.failures.get() + 1;
        self.failures.set(failures);
        if failures >= 2 {
            return self.skip_failed(track);
        }
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(this) = weak.upgrade()
                && this.generation.get() == generation
            {
                this.load_current(this.want_play.get(), start, false);
            }
        });
    }

    /// Gives up on a track that failed twice.
    fn skip_failed(self: &Rc<Self>, track: &TrackInfo) {
        self.failures.set(0);
        let failed = self.failed_tracks.get() + 1;
        self.failed_tracks.set(failed);
        if failed >= MAX_FAILED_TRACKS {
            self.failed_tracks.set(0);
            self.stop();
            return self.emit(Event::Error(format!(
                "Could not play “{}”. Playback stopped: check the connection",
                track.title
            )));
        }
        self.emit(Event::Error(format!(
            "Could not play “{}”, skipped",
            track.title
        )));
        if self.queue.borrow_mut().advance(true) {
            self.emit(Event::Queue);
            self.load_current(true, None, true);
        } else {
            self.finish();
        }
    }

    fn tick(self: &Rc<Self>) {
        self.check_sleep_timer();
        if self.state.get() != PlayState::Playing {
            return;
        }
        let Some(position) = self.engine.as_ref().and_then(Engine::position) else {
            return;
        };
        self.last_position.set(position);
        if let Some(mpris) = &self.mpris {
            mpris.set_position(position);
        }
        if position >= self.healthy_at.get() {
            self.failures.set(0);
            self.failed_tracks.set(0);
        }
        self.maybe_prefetch();
        if self.position_saved_at.get().elapsed() >= POSITION_SAVE_INTERVAL {
            self.save_position(position);
        }
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
                if self.log_output.replace(false)
                    && let Some((format, rate)) =
                        self.engine.as_ref().and_then(Engine::output_format)
                {
                    log::info!("output: {format} at {rate} Hz");
                }
                self.set_state(PlayState::Playing);
            }
            EngineEvent::Paused => self.set_state(PlayState::Paused),
            EngineEvent::Buffering => self.set_state(PlayState::Loading),
            EngineEvent::Finished => {
                self.loaded.set(false);
                if self.sleep_timer() == Some(SleepTimer::EndOfTrack) {
                    return self.stop_at_end_of_track();
                }
                let advanced = self.queue.borrow_mut().advance(false);
                self.emit(Event::Queue);
                if advanced {
                    self.load_current(true, None, true);
                } else {
                    self.finish();
                }
            }
            EngineEvent::NextStarted(track_id) => self.next_started(track_id),
            EngineEvent::Error(message) => self.engine_failed(&message),
        }
    }

    /// The gapless switch happened: the queue follows.
    fn next_started(self: &Rc<Self>, track_id: u64) {
        let prepared = self.prepared.take();
        let advanced = self.queue.borrow_mut().advance(false);
        self.emit(Event::Queue);
        let current = self.current();
        let matches = advanced && current.as_ref().is_some_and(|t| t.id == track_id);
        let (Some(track), Some(prepared), true) = (current, prepared, matches) else {
            // The queue changed after the hand-over: play what it says.
            log::info!("gapless switch to {track_id} no longer matches the queue");
            if advanced {
                self.load_current(true, None, true);
            } else {
                self.finish();
            }
            return;
        };
        log::info!("gapless switch to track {track_id}");
        self.loaded_track.set(Some(track_id));
        self.failures.set(0);
        self.prefetch_failed.set(None);
        self.last_position.set(0.0);
        self.healthy_at.set(HEALTHY_AFTER);
        self.log_output.set(true);
        self.announce(&track);
        let gain = self.gain_for(&prepared.stream);
        self.set_current_stream(prepared.stream, gain);
        if self.sleep_timer() == Some(SleepTimer::EndOfTrack) {
            // Handed over before the timer was set: stop right here.
            self.sleep.replace(None);
            self.pause();
            self.seek(0.0);
            self.emit(Event::SleepTimer);
        }
        self.refresh_next();
    }

    /// The sleep timer's "end of track": the next track waits, paused.
    fn stop_at_end_of_track(self: &Rc<Self>) {
        self.sleep.replace(None);
        let advanced = self.queue.borrow_mut().advance(false);
        self.emit(Event::Queue);
        if advanced {
            self.stop();
            self.unloaded.set(true);
            self.resume_at.set(Some(0.0));
            self.last_position.set(0.0);
            self.emit(Event::Track(self.current()));
            self.set_state(PlayState::Paused);
        } else {
            self.finish();
        }
        self.emit(Event::SleepTimer);
    }

    /// A playback error: reload once at the same position (expired URLs),
    /// then skip.
    fn engine_failed(self: &Rc<Self>, message: &str) {
        self.loaded.set(false);
        let Some(track) = self.current() else {
            self.set_state(PlayState::Stopped);
            return self.emit(Event::Error(format!("Playback failed: {message}")));
        };
        let failures = self.failures.get() + 1;
        self.failures.set(failures);
        if failures >= 2 {
            return self.skip_failed(&track);
        }
        let position = self.last_position.get();
        log::info!(
            "reloading track {} at {position:.1} s after an error: {message}",
            track.id
        );
        // The prepared stream may be just as stale.
        if self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| p.stream.track_id == track.id)
        {
            self.prepared.replace(None);
        }
        self.load_current(self.want_play.get(), Some(position), false);
    }

    /// `SWELL_PLAY=<id>[,<id>…][@<seconds>]`: plays tracks at startup;
    /// `SWELL_PLAY=restore` restores the saved queue and plays it.
    fn debug_autoplay(self: &Rc<Self>) {
        let Ok(spec) = std::env::var("SWELL_PLAY") else {
            return;
        };
        if spec == "restore" {
            self.restore_session();
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(Duration::from_secs(2), move || {
                if let Some(this) = weak.upgrade() {
                    log::info!(
                        "SWELL_PLAY: restored {:?} at {:?}",
                        this.current().map(|t| t.id),
                        this.position()
                    );
                    this.play();
                }
            });
            return;
        }
        let (ids, at) = match spec.split_once('@') {
            Some((ids, at)) => (ids.to_owned(), at.parse::<f64>().ok()),
            None => (spec, None),
        };
        let ids: Vec<u64> = ids
            .split(',')
            .filter_map(|id| id.trim().parse().ok())
            .collect();
        let this = self.clone();
        let api = self.api.clone();
        glib::spawn_future_local(async move {
            let tracks = runtime::spawn(async move {
                let mut tracks = Vec::new();
                for id in ids {
                    tracks.push(TrackInfo::from(api.track(id).await?));
                }
                anyhow::Ok(tracks)
            })
            .await;
            let tracks = match tracks {
                Ok(tracks) => tracks,
                Err(err) => return log::warn!("SWELL_PLAY: {err:#}"),
            };
            let start = at.map(|at| {
                let duration = tracks.first().map_or(0.0, |t| f64::from(t.duration));
                if at < 0.0 {
                    (duration + at).max(0.0)
                } else {
                    at
                }
            });
            this.queue.borrow_mut().set_context(
                tracks,
                0,
                Source {
                    title: "SWELL_PLAY".to_owned(),
                    target: None,
                },
            );
            this.emit(Event::Queue);
            this.load_current(true, start, true);
        });
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        if let Some(tick) = self.tick.take() {
            tick.remove();
        }
        if let Some(listener) = self.settings_listener.take() {
            self.settings.disconnect(listener);
        }
        self.desktop.clear();
    }
}
