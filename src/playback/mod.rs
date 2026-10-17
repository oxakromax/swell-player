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
//! Failures: when the network is to blame (no connection, timeouts, 5xx),
//! the current track and position stay, the state goes to
//! [`PlayState::Loading`] and loading is retried with backoff (1, 2, 4… 30 s,
//! or as soon as GIO reports connectivity), with one
//! "Connection lost — retrying" [`Event::Error`]. Stream lookups that TIDAL
//! answers with a server error (5xx/429) are retried like that only a few
//! times ([`failure::MAX_SERVER_ERRORS`]), then the track counts as failed.
//! Only failures of the track itself count towards skipping it (after two
//! attempts) and towards stopping (after [`MAX_FAILED_TRACKS`] tracks in a
//! row).
//!
//! Debugging: `SWELL_PLAY=<track id>[,<track id>…][@<seconds>]` plays those
//! tracks at startup, starting at `seconds` (negative: from the end);
//! `SWELL_PLAY=restore` restores the saved queue and plays it. Neither saves
//! the session. `SWELL_DEBUG_STREAM_FAIL=<n>[@<skip>]` (see `api.rs`) and
//! `SWELL_DEBUG_ENGINE_FAIL=<seconds>[@<status>]` (a dropped connection, or
//! an HTTP `status` such as 403, that many seconds into the track) simulate
//! network failures.

use crate::i18n::{tr, trn};

pub mod engine;
mod failure;
pub mod format;
mod mpris;
mod notify;
pub mod queue;
mod rundir;
mod session;
mod sleep;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::gio::prelude::*;
use gtk::{gio, glib};

use crate::api::Api;
use crate::model::TrackInfo;
use crate::runtime;
use crate::settings::{self, Normalization, QualityPreference, Repeat};
use crate::tidal::pages::Target;

pub use engine::Stream;
use engine::{Engine, EngineEvent, NextStream};
use failure::{Failure, HandoverBlame};
#[allow(unused_imports)] // Public API for the UI.
pub use format::{Codec, FormatDetails};
use queue::Queue;
pub use queue::QueueSnapshot;
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
/// Network-type engine errors on one track, while TIDAL itself answers,
/// before the track is blamed instead.
const MAX_NETWORK_STRIKES: u32 = 4;
/// A prefetch that failed for lack of network is tried again after this.
const PREFETCH_RETRY: Duration = Duration::from_secs(15);
const SAVE_DELAY: Duration = Duration::from_secs(2);
const POSITION_SAVE_INTERVAL: Duration = Duration::from_secs(10);

/// A stream resolved ahead for gapless playback.
struct Prepared {
    stream: Rc<Stream>,
    at: Instant,
}

impl Prepared {
    fn is_fresh(&self) -> bool {
        is_fresh(self.at)
    }
}

/// Whether a stream resolved at `at` can still be played (its signed URLs
/// have not expired).
fn is_fresh(at: Instant) -> bool {
    at.elapsed() < PREPARED_MAX_AGE
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
    /// When `current_stream` was resolved.
    current_resolved: Cell<Option<Instant>>,
    /// The track the last load was for (to count failures per track).
    loaded_track: Cell<Option<u64>>,
    last_position: Cell<f64>,
    /// The engine's duration of the current track, as last known.
    engine_duration: Cell<Option<f64>>,
    healthy_at: Cell<f64>,
    /// Failed attempts at the current track.
    failures: Cell<u32>,
    /// Stream lookups for the current track that TIDAL answered with a
    /// server error, in a row.
    server_errors: Cell<u32>,
    /// Tracks skipped in a row after failing.
    failed_tracks: Cell<u32>,
    log_output: Cell<bool>,

    prepared: RefCell<Option<Prepared>>,
    /// Track whose stream is being resolved ahead.
    preparing: Cell<Option<u64>>,
    prepare_generation: Cell<u64>,
    /// Do not keep retrying a track whose prefetch failed.
    prefetch_failed: Cell<Option<u64>>,
    /// No prefetch before this (it failed for lack of network).
    prefetch_retry_at: Cell<Option<Instant>>,

    /// Where the load in progress starts (seeking while it resolves moves
    /// it).
    load_start: Cell<Option<f64>>,
    /// Waiting to retry the current track after a network failure.
    net_waiting: Cell<bool>,
    net_attempts: Cell<u32>,
    /// "Connection lost" was said for this outage.
    net_reported: Cell<bool>,
    /// Network-type engine errors on the current track since it was healthy.
    net_strikes: Cell<u32>,
    net_timer: RefCell<Option<glib::SourceId>>,
    network: RefCell<Option<(gio::NetworkMonitor, glib::SignalHandlerId)>>,

    normalization: Cell<Normalization>,
    quality: Cell<QualityPreference>,
    gapless: Cell<bool>,
    applied_volume: Cell<Option<(f64, bool)>>,

    sleep: RefCell<Option<sleep::Armed>>,
    /// Set for good after sign-out (and for debug runs): nothing writes the
    /// session files any more.
    saving_off: Arc<AtomicBool>,
    forgotten: Cell<bool>,
    /// `SWELL_DEBUG_ENGINE_FAIL`: seconds into the track, HTTP status.
    debug_engine_fail: Cell<Option<(f64, u32)>>,
    desktop: notify::Desktop,
    mpris: Option<mpris::Mpris>,
    save_pending: Cell<bool>,
    position_saved_at: Cell<Instant>,
    /// Output device from the settings (display name).
    output_device: RefCell<Option<String>>,
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
                current_resolved: Cell::new(None),
                loaded_track: Cell::new(None),
                last_position: Cell::new(0.0),
                engine_duration: Cell::new(None),
                healthy_at: Cell::new(HEALTHY_AFTER),
                failures: Cell::new(0),
                server_errors: Cell::new(0),
                failed_tracks: Cell::new(0),
                log_output: Cell::new(false),
                prepared: RefCell::default(),
                preparing: Cell::new(None),
                prepare_generation: Cell::new(0),
                prefetch_failed: Cell::new(None),
                prefetch_retry_at: Cell::new(None),
                load_start: Cell::new(None),
                net_waiting: Cell::new(false),
                net_attempts: Cell::new(0),
                net_reported: Cell::new(false),
                net_strikes: Cell::new(0),
                net_timer: RefCell::default(),
                network: RefCell::default(),
                normalization: Cell::new(current.normalization),
                quality: Cell::new(current.quality),
                gapless: Cell::new(current.gapless),
                applied_volume: Cell::new(None),
                sleep: RefCell::default(),
                saving_off: Arc::new(AtomicBool::new(false)),
                forgotten: Cell::new(false),
                debug_engine_fail: Cell::new(
                    std::env::var("SWELL_DEBUG_ENGINE_FAIL")
                        .ok()
                        .and_then(|spec| {
                            let (at, status) = spec.split_once('@').unwrap_or((&spec, "0"));
                            Some((at.parse().ok()?, status.parse().ok()?))
                        }),
                ),
                desktop: notify::Desktop::default(),
                mpris: Some(mpris::Mpris::start(commands)),
                save_pending: Cell::new(false),
                position_saved_at: Cell::new(Instant::now()),
                settings_listener: Cell::new(None),
                output_device: RefCell::new(current.output_device.clone()),
                tick: RefCell::default(),
            }
        });
        this.apply_volume();
        if let Some(engine) = &this.engine {
            engine.set_output_device(this.output_device.borrow().clone());
        }

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

        // Retry at once when the network comes back.
        let monitor = gio::NetworkMonitor::default();
        let weak = Rc::downgrade(&this);
        let handler = monitor.connect_network_changed(move |_, available| {
            if let Some(this) = weak.upgrade()
                && available
                && this.net_waiting.get()
            {
                log::info!("network is back: retrying now");
                this.retry_now();
            }
        });
        this.network.replace(Some((monitor, handler)));
        runtime::runtime().spawn_blocking(rundir::prune_stale);

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
            rundir::remove_own();
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

    /// Pauses if playback is wanted (playing, or loading to play),
    /// otherwise plays.
    pub fn toggle(self: &Rc<Self>) {
        if self.want_play.get() && self.state.get() != PlayState::Stopped {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn play(self: &Rc<Self>) {
        if self.unloaded.get() {
            let start = self.resume_at.get();
            return self.load_current(true, start, false);
        }
        if self.current().is_none() {
            return;
        }
        let state = self.state.get();
        if self.loaded.get() {
            self.want_play.set(true);
            if let Some(engine) = &self.engine {
                engine.resume();
                if state != PlayState::Playing && engine.is_waiting() {
                    self.set_state(PlayState::Loading);
                }
            }
            // A prepared stream may have expired during a long pause.
            self.refresh_next();
        } else if self.resolving.get() {
            self.want_play.set(true);
            self.set_state(PlayState::Loading);
        } else if self.net_waiting.get() {
            self.want_play.set(true);
            self.set_state(PlayState::Loading);
            self.retry_now();
        } else {
            let start = (state != PlayState::Stopped).then(|| self.last_position.get());
            self.load_current(true, start, state == PlayState::Stopped);
        }
        self.sync_mpris();
    }

    /// Pauses, whatever is going on: a stream still resolving or buffering
    /// stays paused once ready.
    pub fn pause(&self) {
        self.want_play.set(false);
        if self.loaded.get()
            && let Some(engine) = &self.engine
        {
            engine.pause();
        }
        if matches!(self.state.get(), PlayState::Loading | PlayState::Playing) {
            self.set_state(PlayState::Paused);
        }
        self.sync_mpris();
    }

    pub fn stop(&self) {
        self.generation.set(self.generation.get() + 1);
        if let Some(engine) = &self.engine {
            engine.stop();
        }
        self.cancel_network_retry();
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
            // Still resolving or waiting for the network: start there.
            if self.current().is_some() && (self.resolving.get() || self.net_waiting.get()) {
                self.load_start.set(Some(position));
                self.last_position.set(position);
                self.emit(Event::Seeked(position));
            }
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
            if let Some(this) = self.this() {
                this.reload_current(position);
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
        if !self.loaded.get() && (self.resolving.get() || self.net_waiting.get()) {
            return Some(self.load_start.get().unwrap_or(0.0));
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
    /// `upcoming()[0]` (what "Up next" shows), without copying the queue.
    pub fn next_upcoming(&self) -> Option<TrackInfo> {
        self.queue.borrow().first_upcoming().cloned()
    }

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

    /// The queue as it is now, for [`Self::restore_queue`] (e.g. to undo
    /// [`Self::clear_upcoming`]).
    pub fn snapshot_queue(&self) -> QueueSnapshot {
        self.queue.borrow().snapshot()
    }

    /// Puts back what was upcoming in `snapshot`: the context in its play
    /// order and the up-next list, as they were. What plays now keeps
    /// playing. If another context was started since, the snapshot's
    /// upcoming tracks are queued after it instead.
    pub fn restore_queue(&self, snapshot: QueueSnapshot) {
        // Same context: what is left of it is a prefix of the snapshot's.
        let same_context = {
            let played = self.queue.borrow().context_in_order();
            played.len() <= snapshot.order.len()
                && played
                    .iter()
                    .zip(snapshot.order.iter().map(|&i| snapshot.context.get(i)))
                    .all(|(a, b)| b.is_some_and(|b| a.id == b.id))
        };
        if same_context {
            self.queue.borrow_mut().restore_upcoming(snapshot);
        } else {
            let upcoming: Vec<TrackInfo> = snapshot
                .up_next
                .iter()
                .cloned()
                .chain(
                    snapshot
                        .order
                        .iter()
                        .skip(snapshot.next)
                        .filter_map(|&i| snapshot.context.get(i).cloned()),
                )
                .collect();
            self.queue.borrow_mut().add_to_queue(upcoming);
        }
        self.queue_changed();
    }

    fn queue_changed(&self) {
        self.emit(Event::Queue);
        if let Some(this) = self.this() {
            this.refresh_next();
        }
    }

    // ---- Session ----

    /// Stops playback, empties the queue and forgets the saved one
    /// (sign-out). From then on this `Playback` never writes the session
    /// files again, not even at shutdown.
    pub fn forget_session(&self) {
        self.saving_off.store(true, Ordering::SeqCst);
        self.forgotten.set(true);
        self.stop();
        let mut queue = Queue::default();
        queue.set_shuffle(self.shuffle());
        queue.set_repeat(self.repeat());
        self.queue.replace(queue);
        self.format.replace(None);
        self.details.replace(None);
        self.current_stream.replace(None);
        self.current_resolved.set(None);
        self.desktop.clear();
        session::clear();
        // Manifests (signed URLs) and covers of this session.
        runtime::runtime().spawn_blocking(rundir::remove_own);
        self.emit(Event::Queue);
        self.emit(Event::Track(None));
        self.emit(Event::Format(None));
    }

    /// Whether the session may be written now.
    fn may_save(&self) -> bool {
        !self.saving_off.load(Ordering::SeqCst) && self.settings.get().restore_session
    }

    /// Writes the queue and position to disk now. It is also saved by
    /// itself as things change, and at shutdown once attached.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn save_session(&self) {
        if !self.may_save() {
            return;
        }
        if let Some(saved) = self.saved_state()
            && let Err(err) = session::write(&saved, &self.saving_off)
        {
            log::warn!("could not save the queue: {err:#}");
        }
    }

    /// Brings back the last saved queue, paused at the saved position,
    /// when the preference is on and nothing plays yet. The stream is only
    /// resolved when playback starts.
    #[allow(dead_code)] // Public API for the shell and the UI.
    pub fn restore_session(&self) {
        if self.forgotten.get() || !self.settings.get().restore_session {
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
        if self.forgotten.get()
            || self.queue.borrow().current().is_some()
            || self.state.get() != PlayState::Stopped
        {
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
            if !this.may_save() {
                return;
            }
            if let Some(saved) = this.saved_state() {
                this.position_saved_at.set(Instant::now());
                let off = this.saving_off.clone();
                runtime::runtime().spawn_blocking(move || {
                    if let Err(err) = session::write(&saved, &off) {
                        log::warn!("could not save the queue: {err:#}");
                    }
                });
            }
        });
    }

    fn save_position(&self, position: f64) {
        self.position_saved_at.set(Instant::now());
        let Some(track) = self.current() else { return };
        if !self.may_save() {
            return;
        }
        let saved = session::SavedPosition {
            track_id: track.id,
            position,
        };
        let off = self.saving_off.clone();
        runtime::runtime().spawn_blocking(move || {
            if let Err(err) = session::write_position(&saved, &off) {
                log::debug!("could not save the position: {err:#}");
            }
        });
    }

    // ---- Internals ----

    fn set_state(&self, state: PlayState) {
        if self.state.replace(state) != state {
            log::debug!("state: {state:?}");
            self.emit(Event::State(state));
        }
    }

    fn finish(&self) {
        self.stop();
        self.format.replace(None);
        self.details.replace(None);
        self.current_stream.replace(None);
        self.current_resolved.set(None);
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
            // The application's `open` handler routes links (pages, or
            // playing a track).
            Command::OpenUri(uri) => {
                if let Some(app) = gio::Application::default() {
                    app.open(&[gio::File::for_uri(&uri)], "");
                }
            }
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
        if *self.output_device.borrow() != settings.output_device {
            self.output_device.replace(settings.output_device.clone());
            self.apply_output_device();
        }
    }

    /// Moves playback to the chosen output: the pipeline is rebuilt at the
    /// same position.
    fn apply_output_device(self: &Rc<Self>) {
        let Some(engine) = &self.engine else { return };
        engine.set_output_device(self.output_device.borrow().clone());
        if !self.loaded.get() || self.current_stream.borrow().is_none() {
            return;
        }
        let position = self.position().unwrap_or(self.last_position.get());
        self.reload_current(position);
    }

    /// Starts the current track over at `position` (a new pipeline): with
    /// its stream if that is still fresh, otherwise with a newly resolved
    /// one (signed URLs expire).
    fn reload_current(self: &Rc<Self>, position: f64) {
        let Some(engine) = &self.engine else { return };
        let current = self.current().map(|t| t.id);
        let stream = self
            .current_stream
            .borrow()
            .clone()
            .filter(|s| Some(s.track_id) == current)
            .filter(|_| self.current_resolved.get().is_some_and(is_fresh));
        let Some(stream) = stream else {
            log::info!("the current stream may have expired: resolving it again");
            return self.load_current(self.want_play.get(), Some(position), false);
        };
        let gain = self.gain_for(&stream);
        self.loaded.set(true);
        self.last_position.set(position);
        if self.want_play.get() {
            self.set_state(PlayState::Loading);
        }
        engine.load(&stream.uri, self.want_play.get(), Some(position), gain);
        self.refresh_next();
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
                // The next track may be handed over already, its gain
                // waiting for it to start.
                if engine.handover_pending()
                    && let Some(prepared) = self.prepared.borrow().as_ref()
                {
                    engine.set_pending_gain(self.gain_for(&prepared.stream));
                }
            } else {
                let position = self.position().unwrap_or(self.last_position.get());
                engine.load(&stream.uri, self.want_play.get(), Some(position), gain);
            }
            self.refresh_next();
        }
        self.emit(Event::Format(self.format()));
    }

    /// Forgets the stream prepared for the next track, unless it is
    /// already handed over (then it plays as it is: dropping it would make
    /// the switch restart the next track).
    fn drop_prepared(&self) {
        self.preparing.set(None);
        self.prefetch_failed.set(None);
        self.prefetch_retry_at.set(None);
        self.prepare_generation
            .set(self.prepare_generation.get() + 1);
        // Withdrawn from the engine first: if the streaming thread took it
        // already, it is what plays next and stays.
        if self
            .engine
            .as_ref()
            .is_none_or(|engine| engine.set_next(None))
        {
            self.prepared.replace(None);
        }
    }

    fn take_prepared(&self, track_id: u64) -> Option<Prepared> {
        let mut prepared = self.prepared.borrow_mut();
        let usable = prepared
            .as_ref()
            .is_some_and(|p| p.stream.track_id == track_id && p.is_fresh());
        if usable { prepared.take() } else { None }
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
    ///
    /// Invariant: `prepared` holds whatever the engine may hand over (its
    /// `next`) or has handed over. The engine's `next` is replaced in one
    /// step that fails once `about-to-finish` took it, and only after that
    /// step succeeded is a stale `prepared` dropped; so the stream about to
    /// play is never dropped or replaced (which would restart that track
    /// from the beginning).
    fn refresh_next(self: &Rc<Self>) {
        let Some(engine) = &self.engine else { return };
        let wanted = self.wanted_next();
        let usable = self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| Some(p.stream.track_id) == wanted && p.is_fresh());
        let next = self
            .prepared
            .borrow()
            .as_ref()
            .filter(|_| usable)
            .map(|p| NextStream {
                track_id: p.stream.track_id,
                uri: p.stream.uri.clone(),
                gain: self.gain_for(&p.stream),
            });
        if !engine.set_next(next) {
            // Handed over already: too late to change; `NextStarted` sorts
            // out a mismatch.
            return;
        }
        if !usable {
            self.prepared.replace(None);
        }
        if self.preparing.get().is_some() && self.preparing.get() != wanted {
            self.preparing.set(None);
            self.prepare_generation
                .set(self.prepare_generation.get() + 1);
        }
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
        // What plays next is decided (see `refresh_next`).
        if self.engine.as_ref().is_some_and(Engine::handover_pending) {
            return;
        }
        let ready = self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| p.stream.track_id == next_id && p.is_fresh());
        if ready || self.preparing.get().is_some() || self.prefetch_failed.get() == Some(next_id) {
            return;
        }
        if self
            .prefetch_retry_at
            .get()
            .is_some_and(|at| Instant::now() < at)
        {
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
            if this.engine.as_ref().is_some_and(Engine::handover_pending) {
                return;
            }
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
                    let kind = failure::classify(&err);
                    log::warn!(
                        "could not prepare track {next_id} ({kind:?}): {}",
                        failure::redact(&format!("{err:#}"))
                    );
                    if kind == Failure::Network {
                        this.prefetch_retry_at
                            .set(Some(Instant::now() + PREFETCH_RETRY));
                    } else {
                        this.prefetch_failed.set(Some(next_id));
                    }
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
            self.emit(Event::Error(
                trn(
                    "Skipped a track that is not available",
                    "Skipped {n} tracks that are not available",
                    skipped as u64,
                )
                .replace("{n}", &skipped.to_string()),
            ));
        }
        let Some(track) = self.current() else {
            self.finish();
            return self.emit(Event::Track(None));
        };
        if self.loaded_track.replace(Some(track.id)) != Some(track.id) {
            self.failures.set(0);
            self.server_errors.set(0);
            self.net_strikes.set(0);
            self.prefetch_failed.set(None);
            self.prefetch_retry_at.set(None);
        }
        self.cancel_network_retry();
        self.unloaded.set(false);
        self.resume_at.set(None);
        self.load_start.set(start);
        self.want_play.set(autoplay);
        self.loaded.set(false);
        if announce || skipped > 0 {
            self.announce(&track);
        }
        let Some(engine) = &self.engine else {
            return self.emit(Event::Error(
                tr("Playback needs GStreamer's playbin3").to_owned(),
            ));
        };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.requested_at.set(Some(Instant::now()));
        engine.stop();
        self.set_state(if autoplay {
            PlayState::Loading
        } else {
            PlayState::Paused
        });

        if let Some(prepared) = self.take_prepared(track.id) {
            return self.start_stream(prepared.stream, prepared.at, start);
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
                Ok(stream) => {
                    this.server_errors.set(0);
                    this.start_stream(Rc::new(stream), Instant::now(), this.load_start.get());
                }
                Err(err) => {
                    let detail = failure::redact(&format!("{err:#}"));
                    let classified = failure::classify(&err);
                    let (kind, count) = failure::after_lookup(
                        classified,
                        failure::server_answered(&err),
                        this.server_errors.get(),
                    );
                    this.server_errors.set(count);
                    if kind != classified {
                        // TIDAL answers, but only with errors: give up on it.
                        log::warn!(
                            "TIDAL keeps failing to give track {} ({detail}): skipping it",
                            track.id
                        );
                        this.loaded.set(false);
                        return this.skip_failed(&track);
                    }
                    this.failed(kind, &track, &detail, Duration::from_secs(1));
                }
            }
        });
    }

    /// Plays `stream` (resolved at `resolved`) from `start`.
    fn start_stream(self: &Rc<Self>, stream: Rc<Stream>, resolved: Instant, start: Option<f64>) {
        let Some(engine) = &self.engine else { return };
        log::info!("playing track {} as {}", stream.track_id, stream.format);
        let gain = self.gain_for(&stream);
        self.set_current_stream(stream.clone(), resolved, gain);
        self.loaded.set(true);
        self.log_output.set(true);
        let start = start.filter(|s| *s > 0.0);
        self.last_position.set(start.unwrap_or(0.0));
        self.healthy_at.set(start.unwrap_or(0.0) + HEALTHY_AFTER);
        engine.load(&stream.uri, self.want_play.get(), start, gain);
        self.refresh_next();
    }

    fn set_current_stream(&self, stream: Rc<Stream>, resolved: Instant, gain: Option<f64>) {
        let mut details = stream.details.clone();
        details.replay_gain_db = gain.map(format::to_db);
        self.details.replace(Some(details));
        self.format.replace(Some(stream.format.clone()));
        self.engine_duration.set(None);
        let format = stream.format.clone();
        self.current_stream.replace(Some(stream));
        self.current_resolved.set(Some(resolved));
        self.emit(Event::Format(Some(format)));
    }

    /// Loading or playing the current track failed (`detail` is already
    /// redacted). The load target is in `load_start`.
    fn failed(self: &Rc<Self>, kind: Failure, track: &TrackInfo, detail: &str, retry_in: Duration) {
        self.loaded.set(false);
        self.resolving.set(false);
        match kind {
            Failure::Network => self.network_failed(track, detail),
            Failure::Track => self.track_failed(track, detail, retry_in),
            Failure::Session => {
                log::warn!("could not play track {}: {detail}", track.id);
                self.stop();
                self.emit(Event::Error(
                    tr("The TIDAL session has expired: sign in again to play").to_owned(),
                ));
            }
        }
    }

    /// The network failed: keep the track and position, retry with backoff
    /// (sooner if the network comes back), and say so once.
    fn network_failed(self: &Rc<Self>, track: &TrackInfo, detail: &str) {
        let attempt = self.net_attempts.get() + 1;
        self.net_attempts.set(attempt);
        let wait = failure::backoff(attempt);
        log::warn!(
            "network trouble playing track {} (retry {attempt} in {} s): {detail}",
            track.id,
            wait.as_secs()
        );
        self.cancel_network_retry();
        self.net_waiting.set(true);
        self.set_state(if self.want_play.get() {
            PlayState::Loading
        } else {
            PlayState::Paused
        });
        if !self.net_reported.replace(true) {
            self.emit(Event::Error(tr("Connection lost — retrying").to_owned()));
        }
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(wait, move || {
            let Some(this) = weak.upgrade() else { return };
            // It fired: nothing to remove any more.
            let _fired = this.net_timer.take();
            if this.generation.get() == generation {
                this.retry_now();
            }
        });
        self.net_timer.replace(Some(timer));
    }

    /// Retries the track waiting for the network, now.
    fn retry_now(self: &Rc<Self>) {
        if !self.net_waiting.get() {
            return;
        }
        self.cancel_network_retry();
        self.load_current(self.want_play.get(), self.load_start.get(), false);
    }

    fn cancel_network_retry(&self) {
        self.net_waiting.set(false);
        if let Some(timer) = self.net_timer.take() {
            timer.remove();
        }
    }

    /// The track itself failed: retry once (after `retry_in`), then skip.
    fn track_failed(self: &Rc<Self>, track: &TrackInfo, detail: &str, retry_in: Duration) {
        log::warn!("could not play track {}: {detail}", track.id);
        let failures = self.failures.get() + 1;
        self.failures.set(failures);
        if failures >= 2 {
            return self.skip_failed(track);
        }
        let start = self.load_start.get();
        if retry_in.is_zero() {
            return self.load_current(self.want_play.get(), start, false);
        }
        self.set_state(if self.want_play.get() {
            PlayState::Loading
        } else {
            PlayState::Paused
        });
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(retry_in, move || {
            if let Some(this) = weak.upgrade()
                && this.generation.get() == generation
            {
                this.load_current(this.want_play.get(), this.load_start.get(), false);
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
            return self.emit(Event::Error(
                tr("Could not play “{title}”. Playback stopped after {n} tracks failed")
                    .replace("{title}", &track.title)
                    .replace("{n}", &MAX_FAILED_TRACKS.to_string()),
            ));
        }
        self.emit(Event::Error(
            tr("Could not play “{title}”, skipped").replace("{title}", &track.title),
        ));
        if self.queue.borrow_mut().advance(true) {
            self.emit(Event::Queue);
            self.load_current(true, None, true);
        } else {
            self.finish();
        }
    }

    fn tick(self: &Rc<Self>) {
        self.check_sleep_timer();
        // Signed URLs expire: a prepared stream that sat too long is
        // resolved again before the switch needs it.
        let stale = self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| !p.is_fresh());
        if stale {
            self.refresh_next();
        }
        // A missed engine message must not leave the UI loading while
        // audio plays.
        if self.state.get() == PlayState::Loading
            && self.loaded.get()
            && self.want_play.get()
            && self.engine.as_ref().is_some_and(Engine::is_playing)
        {
            log::debug!("playing, though no engine message said so");
            self.set_state(PlayState::Playing);
        }
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
            self.net_strikes.set(0);
        }
        if let Some((at, status)) = self.debug_engine_fail.get()
            && position >= at
            && let Some(engine) = &self.engine
        {
            log::info!("SWELL_DEBUG_ENGINE_FAIL: failing with HTTP {status} at {position:.1} s");
            self.debug_engine_fail.set(None);
            engine.simulate_network_error(status);
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
                self.stream_works();
                if !self.want_play.get() {
                    // Paused while it started: the pause is on its way.
                    log::debug!("engine playing, though paused");
                    return;
                }
                if self.log_output.replace(false)
                    && let Some((format, rate)) =
                        self.engine.as_ref().and_then(Engine::output_format)
                {
                    log::info!("output: {format} at {rate} Hz");
                }
                self.set_state(PlayState::Playing);
            }
            EngineEvent::Paused => {
                self.stream_works();
                self.set_state(PlayState::Paused);
            }
            EngineEvent::Buffering => {
                if self.want_play.get() {
                    self.set_state(PlayState::Loading);
                }
            }
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
            EngineEvent::Error {
                message,
                kind,
                from_sink,
            } => self.engine_failed(&message, kind, from_sink),
        }
    }

    /// Data flows again: the outage (if any) is over.
    fn stream_works(&self) {
        if self.net_reported.replace(false) {
            log::info!("connection back");
        }
        self.net_attempts.set(0);
    }

    /// The gapless switch happened: the queue follows.
    fn next_started(self: &Rc<Self>, track_id: u64) {
        let prepared = self
            .prepared
            .take()
            .filter(|p| p.stream.track_id == track_id);
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
        self.net_strikes.set(0);
        self.prefetch_failed.set(None);
        self.prefetch_retry_at.set(None);
        self.last_position.set(0.0);
        self.healthy_at.set(HEALTHY_AFTER);
        self.log_output.set(true);
        self.announce(&track);
        let gain = self.gain_for(&prepared.stream);
        self.set_current_stream(prepared.stream, prepared.at, gain);
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

    /// A playback error. The network's fault: retry the same track at the
    /// same position until it works. The track's: reload once at the same
    /// position (expired URLs), then skip. During the gapless handover, see
    /// [`failure::blame_handover`].
    fn engine_failed(self: &Rc<Self>, message: &str, kind: Failure, from_sink: bool) {
        let handed_over = self.engine.as_ref().is_some_and(Engine::handover_pending);
        self.loaded.set(false);
        let Some(track) = self.current() else {
            self.set_state(PlayState::Stopped);
            return self.emit(Event::Error(
                tr("Playback failed: {error}").replace("{error}", message),
            ));
        };
        if handed_over && failure::blame_handover(kind, from_sink) == HandoverBlame::Next {
            // The next track's stream failed (the end of this one was
            // already decoded): its prepared stream is to blame, likely
            // expired. Finish this track without it; the next one is
            // loaded afresh when it ends.
            let next = self.prepared.take().map(|p| p.stream.track_id);
            log::info!(
                "the stream handed over after track {} failed ({message}): finishing this one first",
                track.id
            );
            if next.is_some() {
                self.prefetch_failed.set(next);
            }
            return self.reload_current(self.last_position.get());
        }
        // Otherwise (also during the handover: the network, the audio
        // output) it is the current track's; the prepared next one stays.
        // The prepared stream may be just as stale.
        if self
            .prepared
            .borrow()
            .as_ref()
            .is_some_and(|p| p.stream.track_id == track.id)
        {
            self.prepared.replace(None);
        }
        self.load_start.set(Some(self.last_position.get()));
        let kind = if kind == Failure::Network {
            let strikes = self.net_strikes.get() + 1;
            self.net_strikes.set(strikes);
            if strikes > MAX_NETWORK_STRIKES {
                log::info!(
                    "track {} keeps failing while TIDAL answers: blaming it",
                    track.id
                );
                self.net_strikes.set(0);
                Failure::Track
            } else {
                Failure::Network
            }
        } else {
            kind
        };
        self.failed(kind, &track, message, Duration::ZERO);
    }

    /// `SWELL_PLAY=<id>[,<id>…][@<seconds>]`: plays tracks at startup;
    /// `SWELL_PLAY=restore` restores the saved queue and plays it.
    fn debug_autoplay(self: &Rc<Self>) {
        let Ok(spec) = std::env::var("SWELL_PLAY") else {
            return;
        };
        // A debug run must not overwrite the user's saved queue.
        self.saving_off.store(true, Ordering::SeqCst);
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
                Err(err) => {
                    return log::warn!("SWELL_PLAY: {}", failure::redact(&format!("{err:#}")));
                }
            };
            let Some(first) = tracks.first() else { return };
            let source = Source {
                title: if first.album.is_empty() {
                    first.title.clone()
                } else {
                    first.album.clone()
                },
                target: (first.album_id != 0).then_some(Target::Album(first.album_id)),
            };
            let start = at.map(|at| {
                let duration = tracks.first().map_or(0.0, |t| f64::from(t.duration));
                if at < 0.0 {
                    (duration + at).max(0.0)
                } else {
                    at
                }
            });
            this.queue.borrow_mut().set_context(tracks, 0, source);
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
        if let Some((monitor, handler)) = self.network.take() {
            monitor.disconnect(handler);
        }
        if let Some(timer) = self.net_timer.take() {
            timer.remove();
        }
        self.desktop.clear();
        rundir::remove_own();
    }
}
