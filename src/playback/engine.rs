//! Audio playback with GStreamer's `playbin3`, on two decks.
//!
//! TIDAL hands out two kinds of stream:
//! - Hi-Res Lossless as an MPEG-DASH manifest (FLAC in fragmented MP4). The
//!   manifest is written to one file per track and played as a `file://`
//!   URI; `dashdemux2` then fetches the segments itself, streaming.
//! - Everything else as a "BTS" manifest: JSON with a direct URL to the file.
//!
//! Both go through the local proxy of `proxy.rs` when it runs, so the first
//! seconds prefetched for upcoming tracks are played from memory.
//!
//! Bit-perfect output: without normalisation there is no filter in the
//! pipeline, and the volume is the sink's stream volume (PipeWire/Pulse), so
//! the decoder's samples reach the sound server untouched in their native
//! format and rate (e.g. S24_32LE at 96 kHz); at 100 % the server leaves
//! them alone too. With normalisation, a `volume` element scales them.
//!
//! Gapless: the next track's URI waits in [`Engine::set_next`]; playbin3
//! asks for it on `about-to-finish` (a streaming thread, so it only takes a
//! lock), and the switch is reported when the new stream actually starts.
//!
//! # Two decks: fades and crossfades
//!
//! The engine owns two `playbin3` pipelines ("decks"), each with its own
//! audio sink; one is active (what the rest of Swell sees and controls),
//! the other is idle in `Null`, which costs next to nothing. The second
//! deck is used only while two tracks overlap:
//! - a crossfade (an automatic transition, see `transition.rs`): the next
//!   track is pre-rolled, paused, on the idle deck a few seconds early,
//!   started when the crossfade begins, and the two decks' volumes follow
//!   equal-power ramps until the old deck stops;
//! - a manual change (skip, jump, reload): the old deck fades out over
//!   [`fade::MANUAL_FADE`] while the new one already loads on the other,
//!   so the fade adds no latency.
//!
//! Pausing, resuming and seeking fade the active deck (out, act, back in),
//! which avoids the click of cutting a waveform mid-cycle.
//!
//! Why two pipelines and not one with an `audiomixer`: a mixer needs one
//! fixed format for all its inputs, so every track would be converted and
//! resampled all the time, and bit-perfect output (S24_32LE at 96 kHz going
//! straight to the sound server) would be gone. With two decks each track
//! keeps its native format from decoder to sound server; during the few
//! seconds of a crossfade the sound server mixes the two streams, as it
//! mixes any two applications. Each deck keeps all of playbin3's behaviour
//! (gapless `about-to-finish`, buffering, DASH, seeking, error reporting),
//! and the decks' volumes are the sinks' stream volumes (`soft-volume`
//! goes to PulseAudio/PipeWire), so fades never touch the samples either:
//! once a fade is over the output is bit-perfect again. CPU doubles only
//! while two tracks actually overlap.
//!
//! Ramps are driven by a main-loop timer (every 15 ms while one runs, every
//! 250 ms while a crossfade is waiting for its moment, none otherwise).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gst::prelude::*;
use gtk::glib;
use serde::Deserialize;

use super::expiry;
use super::fade::{self, Ramp, Shape};
use super::failure::{self, EngineError, Failure};
use super::format::{FormatDetails, ReplayGain};
use super::prefetch;
use super::proxy;
use super::transition;
use crate::tidal::models::PlaybackInfo;

/// What TIDAL returned for a track, resolved to something `playbin3` plays.
pub struct Stream {
    pub uri: String,
    /// Human-readable format, e.g. "24-bit/96 kHz FLAC".
    pub format: String,
    pub track_id: u64,
    /// The same format as data.
    pub details: FormatDetails,
    pub replay_gain: ReplayGain,
    /// How long after resolving it the stream can still be started (its
    /// signed URLs expire; see `expiry.rs`).
    pub lifetime: Duration,
    /// Its route through the local proxy, when it plays through it.
    pub route: Option<proxy::Route>,
}

impl Stream {
    /// Decodes the manifest of `info`, TIDAL's answer for `track_id` (the
    /// id asked for, which the answer's own id need not match), and routes
    /// it through the local proxy when that runs. DASH manifests are
    /// written to disk, so this runs on the I/O runtime.
    pub async fn from_playback_info(track_id: u64, info: PlaybackInfo) -> Result<Self> {
        if info.track_id != track_id {
            log::debug!("track {track_id} streams as track {}", info.track_id);
        }
        let manifest = BASE64
            .decode(info.manifest.as_bytes())
            .context("the playback manifest is not base64")?;
        let mime = info.manifest_mime_type.to_ascii_lowercase();
        let now = expiry::now_unix();

        // What the proxy keeps of the start, and the format it is kept
        // under on disk (with the media's identity, which the proxy adds).
        let start = |codecs: Option<&str>| {
            let details = FormatDetails::new(&info, codecs);
            proxy::Start {
                seconds: prefetch::PREFETCH_SECONDS,
                bytes_per_second: prefetch::bytes_per_second(&details),
                format: Some(media_format(&info, codecs)),
            }
        };

        let (uri, codecs, lifetime, route) = if mime.starts_with("application/dash+xml") {
            let mpd = String::from_utf8_lossy(&manifest).into_owned();
            let codecs = dash_codecs(&mpd);
            let urls = proxy::dash_urls(&mpd);
            let lifetime = expiry::lifetime(urls.iter().map(String::as_str), now);
            let start = start(codecs.as_deref());
            let routed =
                proxy::get().and_then(|proxy| match proxy.route_dash(track_id, &mpd, &start) {
                    Ok(routed) => Some(routed),
                    Err(err) => {
                        log::debug!("track {track_id} streams without the proxy: {err:#}");
                        None
                    }
                });
            let (mpd, route) = match routed {
                Some((mpd, route)) => (mpd, Some(route)),
                None => (mpd, None),
            };
            let tag = route.as_ref().map_or("direct", |r| &r.key[..8]);
            let path = write_manifest(track_id, tag, mpd.as_bytes()).await?;
            let uri = glib::filename_to_uri(&path, None)?.to_string();
            (uri, codecs, lifetime, route)
        } else if mime.starts_with("application/vnd.tidal.bts") {
            let bts: BtsManifest = serde_json::from_slice(&manifest)?;
            if !bts.encryption_type.eq_ignore_ascii_case("NONE") {
                bail!(
                    "encrypted streams ({}) are not supported",
                    bts.encryption_type
                );
            }
            let url = bts
                .urls
                .into_iter()
                .next()
                .context("the manifest lists no URL")?;
            let lifetime = expiry::lifetime([url.as_str()], now);
            let start = start(bts.codecs.as_deref());
            match proxy::get().and_then(|proxy| proxy.route_progressive(track_id, &url, &start)) {
                Some(route) => (route.uri.clone(), bts.codecs, lifetime, Some(route)),
                None => (url, bts.codecs, lifetime, None),
            }
        } else {
            bail!("unsupported manifest type {}", info.manifest_mime_type);
        };
        let details = FormatDetails::new(&info, codecs.as_deref());
        Ok(Self {
            uri,
            format: details.describe(),
            track_id,
            details,
            replay_gain: ReplayGain::new(&info),
            lifetime,
            route,
        })
    }

    /// Whether the stream, resolved at `resolved`, can still be started.
    pub fn is_fresh(&self, resolved: Instant) -> bool {
        resolved.elapsed() < self.lifetime
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BtsManifest {
    encryption_type: String,
    urls: Vec<String>,
    #[serde(default)]
    codecs: Option<String>,
}

/// The format of a stream as the disk cache keys it: what TIDAL says of
/// it, never a URL.
fn media_format(info: &PlaybackInfo, codecs: Option<&str>) -> String {
    format!(
        "{}/{}/{}/{}/{}",
        info.audio_quality.as_deref().unwrap_or("-"),
        info.audio_mode.as_deref().unwrap_or("-"),
        info.bit_depth.unwrap_or(0),
        info.sample_rate.unwrap_or(0),
        codecs.unwrap_or("-")
    )
}

/// The first `codecs="…"` of an MPD.
fn dash_codecs(mpd: &str) -> Option<String> {
    let start = mpd.find("codecs=\"")? + "codecs=\"".len();
    let end = mpd[start..].find('"')?;
    Some(mpd[start..start + end].to_owned())
}

/// Tracks whose manifests must survive pruning: the current, the next and
/// the prefetched ones.
static KEEP_MANIFESTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Sets which tracks' DASH manifests are in use (the current one, the one
/// prepared for gapless playback, prefetched ones); the others are deleted
/// when the next manifest is written.
pub fn keep_manifests(track_ids: &[u64]) {
    *KEEP_MANIFESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = track_ids.to_vec();
}

/// Tracks the prefetcher holds streams for (their manifests stay too).
static KEEP_PREFETCHED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Sets which tracks the prefetcher holds (or is resolving) streams for.
pub fn keep_prefetched_manifests(track_ids: &[u64]) {
    *KEEP_PREFETCHED
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = track_ids.to_vec();
}

/// Manifests carry signed segment URLs, so they go to this process's
/// private, in-memory runtime directory, one file per track.
/// `<track id>-<tag>.mpd`: each stream has its own (`tag` tells streams of
/// the same track apart).
async fn write_manifest(track_id: u64, tag: &str, manifest: &[u8]) -> Result<PathBuf> {
    let dir = tokio::task::spawn_blocking(super::rundir::create).await??;
    let path = dir.join(format!("{track_id}-{tag}.mpd"));
    let partial = super::rundir::temp_path(&path);
    let written = match tokio::fs::write(&partial, manifest).await {
        Ok(()) => tokio::fs::rename(&partial, &path).await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(err.into());
    }

    let mut keep = KEEP_MANIFESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    keep.extend(
        KEEP_PREFETCHED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter(),
    );
    let mut entries = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let old = entry.path();
        if old == path || old.extension().is_none_or(|ext| ext != "mpd") {
            continue;
        }
        let id = old
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.split('-').next())
            .and_then(|id| id.parse::<u64>().ok());
        if !id.is_some_and(|id| keep.contains(&id)) {
            let _ = tokio::fs::remove_file(old).await;
        }
    }
    Ok(path)
}

pub enum EngineEvent {
    Playing,
    Paused,
    /// Waiting for data (network stall or pre-roll).
    Buffering,
    /// The end of the last stream (nothing was handed over).
    Finished,
    /// Playback failed; `kind` says whether the network is to blame,
    /// `from_sink` whether the audio output posted it.
    Error {
        message: String,
        kind: Failure,
        from_sink: bool,
    },
    /// The track handed over with [`Engine::set_next`] started playing
    /// (gaplessly, or as a crossfade began).
    NextStarted(u64),
}

/// The next track, ready for a gapless switch or a crossfade.
#[derive(Clone, Debug, PartialEq)]
pub struct NextStream {
    pub track_id: u64,
    pub uri: String,
    /// ReplayGain factor; ignored when no normalisation filter is in place.
    pub gain: Option<f64>,
    /// Crossfade into it over this long instead of switching gaplessly.
    pub crossfade: Option<Duration>,
    /// If the crossfade cannot happen (its pre-roll failed), switch
    /// gaplessly instead (gapless playback is on).
    pub gapless_fallback: bool,
}

/// Pre-roll the next track this long before its crossfade starts.
const PREROLL_LEAD: f64 = 10.0;
/// Ramp timer while a fade runs.
const RAMP_TICK: Duration = Duration::from_millis(15);
/// Timer while a crossfade waits for its moment.
const WATCH_TICK: Duration = Duration::from_millis(250);

/// State shared with GStreamer's streaming threads, one per deck.
///
/// Invariant: once `about-to-finish` took `next` (`handed_over` is set),
/// the main thread can no longer change what plays next; [`Shared::offer`]
/// refuses then, in the same critical section, so the caller keeps the
/// stream it handed over (dropping it would restart the next track).
#[derive(Default)]
struct Shared {
    next: Option<NextStream>,
    /// Taken by `about-to-finish`, until its stream starts.
    handed_over: Option<u64>,
    /// Gain for the handed-over stream, applied when it reaches the filter.
    pending_gain: Option<f64>,
    /// The normalisation `volume` element, when installed.
    filter: Option<gst::Element>,
}

impl Shared {
    /// Sets the stream for the next gapless switch, unless one was handed
    /// over already (then nothing changes and this returns false).
    fn offer(&mut self, next: Option<NextStream>) -> bool {
        if self.handed_over.is_some() {
            return false;
        }
        self.next = next;
        true
    }

    /// `about-to-finish`: takes the waiting stream, if any. A stream meant
    /// for a crossfade stays: the engine starts it on the other deck.
    fn hand_over(&mut self) -> Option<NextStream> {
        if self.next.as_ref()?.crossfade.is_some() {
            return None;
        }
        let next = self.next.take()?;
        self.handed_over = Some(next.track_id);
        self.pending_gain = next.gain;
        Some(next)
    }

    fn reset(&mut self) {
        self.next = None;
        self.handed_over = None;
        self.pending_gain = None;
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A sink element for the `Audio/Sink` device called `name`.
fn audio_sink_for(name: &str) -> Option<gst::Element> {
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some("Audio/Sink"), None);
    if monitor.start().is_err() {
        return None;
    }
    let device = monitor
        .devices()
        .into_iter()
        .find(|device| device.display_name() == name);
    monitor.stop();
    device?.create_element(None).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// In `Null`, free.
    Idle,
    /// What plays (or is paused, or loads): the engine's API is about it.
    Active,
    /// Fading out (a manual change, the end of a crossfade), then stopped.
    Outgoing,
    /// The next track, pre-rolled paused for a crossfade.
    Preroll,
}

/// What a deck does once its fade is over.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    Nothing,
    Pause,
    Stop,
    Seek,
}

/// One `playbin3` and its state. Main thread only, except `shared`.
struct Deck {
    name: &'static str,
    playbin: gst::Element,
    shared: Arc<Mutex<Shared>>,
    role: Cell<Role>,
    /// What the user wants: playing or paused.
    target_playing: Cell<bool>,
    buffering: Cell<bool>,
    /// Seek to this once pre-rolled (resuming at a position).
    start_at: Cell<Option<f64>>,
    /// The start seek is in flight.
    seeking_to_start: Cell<bool>,
    /// The first stream of a load has not started yet.
    awaiting_first_start: Cell<bool>,
    /// The device the pipeline was last built with.
    output_applied: RefCell<Option<String>>,
    /// Fade factor, 0–1, on top of the user's volume.
    gain: Cell<f64>,
    /// The volume last given to the pipeline.
    applied: Cell<Option<f64>>,
    ramp: Cell<Option<Ramp>>,
    then: Cell<Then>,
    /// When the action after the fade is due (the fade's end plus the
    /// sink's latency, when the volume is not applied by the sound server).
    then_at: Cell<Option<Instant>>,
    /// Extra wait after a fade-out before pausing or stopping.
    tail: Cell<Duration>,
    /// Fade in once playing (after a mid-track load, a resume, a seek).
    fade_in: Cell<bool>,
    /// A seek waiting for the fade-out before it.
    pending_seek: Cell<Option<f64>>,
    /// Pre-rolled (for a crossfade): ready to start at once.
    prerolled: Cell<bool>,
    _watch: RefCell<Option<gst::bus::BusWatchGuard>>,
}

impl Deck {
    fn new(name: &'static str) -> Result<Self> {
        let playbin = gst::ElementFactory::make("playbin3")
            .name(name)
            .build()
            .context("GStreamer's playbin3 is missing (install gst-plugins-base)")?;
        // Audio only: never build video or subtitle branches. Buffering
        // reports network stalls.
        playbin.set_property_from_str("flags", "audio+soft-volume+buffering");
        let shared = Arc::new(Mutex::new(Shared::default()));

        // Streams from the local proxy must not go through `http_proxy`.
        playbin.connect("source-setup", false, |values| {
            if let Ok(source) = values[1].get::<gst::Element>() {
                proxy::bypass_env_proxy(&source);
            }
            None
        });

        // Runs on a streaming thread: only hand over what is ready.
        playbin.connect("about-to-finish", false, {
            let shared = Arc::downgrade(&shared);
            move |values| {
                let playbin = values[0].get::<gst::Element>().ok()?;
                let shared = shared.upgrade()?;
                let next = lock(&shared).hand_over()?;
                if log::log_enabled!(log::Level::Debug) {
                    let left = playbin
                        .query_duration::<gst::ClockTime>()
                        .zip(playbin.query_position::<gst::ClockTime>())
                        .map(|(d, p)| d.saturating_sub(p).seconds_f64());
                    log::debug!(
                        "gapless: handing over track {} ({left:?} s left)",
                        next.track_id
                    );
                }
                playbin.set_property("uri", &next.uri);
                None
            }
        });

        Ok(Self {
            name,
            playbin,
            shared,
            role: Cell::new(Role::Idle),
            target_playing: Cell::new(false),
            buffering: Cell::new(false),
            start_at: Cell::new(None),
            seeking_to_start: Cell::new(false),
            awaiting_first_start: Cell::new(false),
            output_applied: RefCell::default(),
            gain: Cell::new(1.0),
            applied: Cell::new(None),
            ramp: Cell::new(None),
            then: Cell::new(Then::Nothing),
            then_at: Cell::new(None),
            tail: Cell::new(Duration::ZERO),
            fade_in: Cell::new(false),
            pending_seek: Cell::new(None),
            prerolled: Cell::new(false),
            _watch: RefCell::default(),
        })
    }

    fn set_state(&self, state: gst::State) {
        if let Err(err) = self.playbin.set_state(state) {
            log::warn!("could not switch playback to {state:?}: {err}");
        }
    }

    /// Stops at once and frees the deck.
    fn cut(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
        lock(&self.shared).reset();
        self.role.set(Role::Idle);
        self.target_playing.set(false);
        self.buffering.set(false);
        self.start_at.set(None);
        self.seeking_to_start.set(false);
        self.ramp.set(None);
        self.then.set(Then::Nothing);
        self.then_at.set(None);
        self.fade_in.set(false);
        self.pending_seek.set(None);
        self.prerolled.set(false);
    }

    fn is_playing(&self) -> bool {
        is_playing(&self.playbin)
    }

    /// Whether anything of it can be heard now.
    fn is_audible(&self) -> bool {
        self.is_playing() && self.gain.get() > 0.001
    }

    fn is_ramping(&self) -> bool {
        self.ramp.get().is_some() || self.then_at.get().is_some()
    }

    /// Fades to `to` (from where it is now), then does `then`.
    fn fade_to(&self, to: f64, length: Duration, shape: Shape, then: Then) {
        let ramp = Ramp::new(self.gain.get(), to, Instant::now(), length, shape);
        self.ramp.set(Some(ramp));
        self.then.set(then);
        self.then_at.set(None);
    }

    /// Builds the audio sink for the wanted device. Only while the pipeline
    /// is in `Null`. A device that is not there (yet) leaves the default in
    /// place and is looked for again on the next load, so a DAC plugged in
    /// later is picked up.
    fn apply_output_device(&self, wanted: Option<String>) {
        if *self.output_applied.borrow() == wanted {
            return;
        }
        let sink = wanted
            .as_deref()
            .and_then(|name| match audio_sink_for(name) {
                Some(sink) => Some(sink),
                None => {
                    log::warn!("output device “{name}” not found; using the default");
                    None
                }
            });
        // Only what was built counts as applied.
        let applied = if sink.is_some() { wanted } else { None };
        if sink.is_some() || self.output_applied.borrow().is_some() {
            self.playbin.set_property("audio-sink", sink.as_ref());
        }
        self.output_applied.replace(applied);
    }

    /// Puts a `volume` element in the pipeline for normalisation, or none.
    /// Takes effect on the next start from `Null`.
    fn install_filter(&self, gain: Option<f64>) {
        let filter = gain.and_then(|gain| match normalization_filter(gain, &self.shared) {
            Ok(filter) => Some(filter),
            Err(err) => {
                log::warn!("normalisation unavailable: {err:#}");
                None
            }
        });
        let (bin, volume) = filter.unzip();
        self.playbin.set_property("audio-filter", bin.as_ref());
        lock(&self.shared).filter = volume;
    }

    /// Loads `uri` from `Null`, playing or paused, from `start` seconds.
    fn setup(
        &self,
        uri: &str,
        play: bool,
        start: Option<f64>,
        gain: Option<f64>,
        output: Option<String>,
    ) {
        if self.is_audible() {
            log::debug!(
                "{}: stopped while audible (fade factor {:.2})",
                self.name,
                self.gain.get()
            );
        }
        self.cut();
        self.install_filter(gain);
        self.apply_output_device(output);
        self.playbin.set_property("uri", uri);
        let start = start.filter(|s| *s > 0.5);
        self.target_playing.set(play);
        self.start_at.set(start);
        self.awaiting_first_start.set(true);
        self.set_state(if play && start.is_none() {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
    }

    fn position(&self) -> Option<f64> {
        if let Some(start) = self.pending_seek.get().or(self.start_at.get()) {
            return Some(start);
        }
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
    }

    fn duration(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
            .filter(|d| *d > 0.0)
    }

    /// How long the sink holds audio that a pipeline-side volume change
    /// has not reached yet: nothing when the sound server applies the
    /// volume (a `GstStreamVolume` sink, such as PulseAudio's).
    fn measure_tail(&self) {
        let Some(sink) = audio_sink(&self.playbin) else {
            return;
        };
        let server_volume =
            glib::Type::from_name("GstStreamVolume").is_some_and(|iface| sink.type_().is_a(iface));
        let tail = if server_volume {
            Duration::ZERO
        } else {
            let mut query = gst::query::Latency::new();
            let latency = if self.playbin.query(&mut query) {
                query.result().1.nseconds()
            } else {
                0
            };
            Duration::from_nanos(latency).min(Duration::from_millis(300))
        };
        self.tail.set(tail);
    }
}

/// Settled in `Playing` (no state change pending).
fn is_playing(playbin: &gst::Element) -> bool {
    let (_, current, pending) = playbin.state(gst::ClockTime::ZERO);
    current == gst::State::Playing && pending == gst::State::VoidPending
}

fn seek(playbin: &gst::Element, position: f64) {
    let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
    let target = gst::ClockTime::from_nseconds((position.max(0.0) * 1e9) as u64);
    if let Err(err) = playbin.seek_simple(flags, target) {
        log::debug!("seek failed: {err}");
    }
}

/// The audio sink element inside a pipeline.
fn audio_sink(playbin: &gst::Element) -> Option<gst::Element> {
    let bin = playbin.downcast_ref::<gst::Bin>()?;
    let mut elements = bin.iterate_recurse();
    while let Ok(Some(element)) = elements.next() {
        if element.element_flags().contains(gst::ElementFlags::SINK) && !element.is::<gst::Bin>() {
            return Some(element);
        }
    }
    None
}

/// Whether a load takes the active deck (else the other one), given each
/// deck's role and fade factor once the active one has let go: a free deck;
/// with both still fading out (a change during a crossfade), the quieter
/// one, which is stopped at once while the louder one fades on (cutting the
/// louder would click).
fn load_on_active(active: (Role, f64), other: (Role, f64)) -> bool {
    if active.0 == Role::Idle {
        return true;
    }
    other.0 != Role::Idle && active.1 < other.1
}

/// A crossfade waiting for its moment.
#[derive(Clone, Debug)]
struct Armed {
    next: NextStream,
    length: Duration,
    /// The deck it pre-rolls on, once it does.
    deck: Option<usize>,
    /// The crossfade is due but the deck is not pre-rolled yet.
    due: bool,
}

struct Inner {
    decks: [Deck; 2],
    active: Cell<usize>,
    /// The user's volume as an amplitude factor (already curved).
    volume: Cell<f64>,
    muted: Cell<bool>,
    /// Short fades on manual changes.
    fades: Cell<bool>,
    /// The output device asked for (display name), `None` = system default.
    output_wanted: RefCell<Option<String>>,
    armed: RefCell<Option<Armed>>,
    /// A track whose pre-roll failed: it switches gaplessly instead.
    preroll_failed: Cell<Option<u64>>,
    timer: RefCell<Option<(glib::SourceId, Duration)>>,
    on_event: Box<dyn Fn(EngineEvent)>,
}

/// `playbin3` ×2, audio only. Events arrive on the main thread.
pub struct Engine {
    inner: Rc<Inner>,
}

impl Engine {
    pub fn new(on_event: impl Fn(EngineEvent) + 'static) -> Result<Self> {
        let decks = [Deck::new("deck-a")?, Deck::new("deck-b")?];
        let inner = Rc::new(Inner {
            decks,
            active: Cell::new(0),
            volume: Cell::new(1.0),
            muted: Cell::new(false),
            fades: Cell::new(true),
            output_wanted: RefCell::default(),
            armed: RefCell::default(),
            preroll_failed: Cell::new(None),
            timer: RefCell::default(),
            on_event: Box::new(on_event),
        });
        for (index, deck) in inner.decks.iter().enumerate() {
            let bus = deck.playbin.bus().context("playbin3 has no bus")?;
            let weak = Rc::downgrade(&inner);
            let watch = bus.add_watch_local(move |_, message| {
                let Some(inner) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                inner.on_message(index, message);
                glib::ControlFlow::Continue
            })?;
            deck._watch.replace(Some(watch));
        }
        Ok(Self { inner })
    }

    /// Starts `uri`, playing or paused, from `start` seconds (or the
    /// beginning). `gain` is the normalisation factor; `None` plays without
    /// any filter (bit-perfect). What played before fades out (when fades
    /// are on) while this loads on the other deck; a mid-track start fades
    /// in.
    pub fn load(&self, uri: &str, play: bool, start: Option<f64>, gain: Option<f64>) {
        self.inner.load(uri, play, start, gain);
    }

    /// Short fades on pause, resume, seek and manual track changes.
    pub fn set_fades(&self, on: bool) {
        self.inner.fades.set(on);
    }

    /// Chooses the output device for the next load (display name of a
    /// GStreamer `Audio/Sink` device; `None` = the system default).
    pub fn set_output_device(&self, name: Option<String>) {
        self.inner.output_wanted.replace(name);
    }

    /// Whether the pipeline has the normalisation filter (as last loaded).
    pub fn has_filter(&self) -> bool {
        lock(&self.inner.active().shared).filter.is_some()
    }

    /// Changes the normalisation factor of what is playing now.
    pub fn set_gain(&self, gain: f64) {
        let filter = lock(&self.inner.active().shared).filter.clone();
        if let Some(filter) = filter {
            filter.set_property("volume", gain.clamp(0.0, 10.0));
        }
    }

    /// Changes the gain waiting for the handed-over stream (normalisation
    /// changed during the gapless switch).
    pub fn set_pending_gain(&self, gain: Option<f64>) {
        let mut shared = lock(&self.inner.active().shared);
        if shared.handed_over.is_some() {
            shared.pending_gain = gain;
        }
    }

    /// Debugging (`SWELL_DEBUG_ENGINE_FAIL`): posts the error an HTTP
    /// source gives when the connection drops (`status` 0) or the server
    /// answers `status`.
    pub fn simulate_network_error(&self, status: u32) {
        let playbin = &self.inner.active().playbin;
        let details = gst::Structure::builder("details")
            .field("http-status-code", status)
            .build();
        let message =
            gst::message::Error::builder(gst::ResourceError::Read, "Simulated connection loss")
                .debug("simulated, URL: https://example.invalid/segment.mp4?token=SECRET")
                .details(details)
                .src(playbin)
                .build();
        if let Some(bus) = playbin.bus() {
            let _ = bus.post(message);
        }
    }

    /// The stream to switch to when the current one ends: gaplessly, or as
    /// a crossfade when `next.crossfade` says so. Returns false, changing
    /// nothing, if `about-to-finish` already took one for a gapless switch
    /// (checked atomically with the change): the caller must then keep the
    /// stream it handed over.
    #[must_use]
    pub fn set_next(&self, next: Option<NextStream>) -> bool {
        self.inner.set_next(next)
    }

    /// The next stream was handed over but has not started yet: the old one
    /// is still audible, its end already decoded.
    pub fn handover_pending(&self) -> bool {
        lock(&self.inner.active().shared).handed_over.is_some()
    }

    pub fn resume(&self) {
        self.inner.resume();
    }

    pub fn pause(&self) {
        self.inner.pause();
    }

    /// Stops, fading out what can be heard first (at once when fades are
    /// off or nothing is heard).
    pub fn stop_faded(&self) {
        let inner = &self.inner;
        inner.disarm();
        inner.quiet_outgoing();
        inner.release_active();
        inner.ensure_timer();
    }

    /// Amplitude factor 0.0–1.0 (already curved by the caller).
    pub fn set_volume(&self, volume: f64) {
        self.inner.volume.set(volume.clamp(0.0, 1.0));
        for deck in &self.inner.decks {
            self.inner.apply_volume(deck);
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.muted.set(muted);
        for deck in &self.inner.decks {
            deck.playbin.set_property("mute", muted);
        }
    }

    /// Audio is flowing: the pipeline settled in `Playing` and is not
    /// waiting for data.
    pub fn is_playing(&self) -> bool {
        let deck = self.inner.active();
        deck.role.get() == Role::Active
            && !deck.buffering.get()
            && deck.start_at.get().is_none()
            && deck.is_playing()
    }

    /// Waiting for data or for the start position: not audible yet even
    /// if wanted.
    pub fn is_waiting(&self) -> bool {
        let deck = self.inner.active();
        deck.buffering.get() || deck.start_at.get().is_some() || deck.seeking_to_start.get()
    }

    /// Seconds.
    pub fn position(&self) -> Option<f64> {
        let deck = self.inner.active();
        (deck.role.get() == Role::Active)
            .then(|| deck.position())
            .flatten()
    }

    /// Seconds.
    pub fn duration(&self) -> Option<f64> {
        let deck = self.inner.active();
        (deck.role.get() == Role::Active)
            .then(|| deck.duration())
            .flatten()
    }

    /// Seconds.
    pub fn seek(&self, position: f64) {
        self.inner.seek(position);
    }

    /// The sample format and rate going to the sound server, e.g.
    /// `("S24_32LE", 96000)`.
    pub fn output_format(&self) -> Option<(String, i32)> {
        let sink = audio_sink(&self.inner.active().playbin)?;
        let caps = sink.static_pad("sink").and_then(|pad| pad.current_caps())?;
        let structure = caps.structure(0)?;
        let format = structure.get::<String>("format").ok()?;
        let rate = structure.get::<i32>("rate").ok()?;
        Some((format, rate))
    }
}

impl Inner {
    fn active(&self) -> &Deck {
        &self.decks[self.active.get()]
    }

    fn other(&self) -> usize {
        1 - self.active.get()
    }

    fn emit(&self, event: EngineEvent) {
        (self.on_event)(event);
    }

    fn apply_volume(&self, deck: &Deck) {
        let volume = (self.volume.get() * deck.gain.get()).clamp(0.0, 1.0);
        if deck
            .applied
            .replace(Some(volume))
            .is_some_and(|old| (old - volume).abs() < 1e-4)
        {
            return;
        }
        deck.playbin.set_property("volume", volume);
    }

    fn set_gain_now(&self, deck: &Deck, gain: f64) {
        deck.gain.set(gain);
        self.apply_volume(deck);
    }

    /// The active deck lets go: it fades out and stops if it can be heard
    /// (and fades are on), otherwise it stops at once.
    fn release_active(self: &Rc<Self>) {
        let deck = self.active();
        if deck.role.get() != Role::Active {
            return;
        }
        if self.fades.get() && deck.is_audible() {
            lock(&deck.shared).reset();
            deck.role.set(Role::Outgoing);
            deck.target_playing.set(false);
            deck.fade_to(0.0, fade::MANUAL_FADE, Shape::Smooth, Then::Stop);
        } else {
            deck.cut();
        }
    }

    fn load(self: &Rc<Self>, uri: &str, play: bool, start: Option<f64>, gain: Option<f64>) {
        self.disarm();
        self.preroll_failed.set(None);
        // What still fades out of a crossfade goes quickly, as the active
        // deck does.
        self.quiet_outgoing();
        self.release_active();
        let (active, other) = (self.active(), &self.decks[self.other()]);
        let index = if load_on_active(
            (active.role.get(), active.gain.get()),
            (other.role.get(), other.gain.get()),
        ) {
            self.active.get()
        } else {
            self.other()
        };
        let deck = &self.decks[index];
        let fade_in = self.fades.get() && start.is_some_and(|s| s > 0.5);
        deck.setup(uri, play, start, gain, self.output_wanted.borrow().clone());
        deck.role.set(Role::Active);
        deck.fade_in.set(fade_in);
        self.set_gain_now(deck, if fade_in { 0.0 } else { 1.0 });
        deck.playbin.set_property("mute", self.muted.get());
        self.active.set(index);
        self.ensure_timer();
    }

    fn pause(self: &Rc<Self>) {
        let deck = self.active();
        deck.target_playing.set(false);
        deck.fade_in.set(false);
        if self.fades.get() && deck.is_audible() && deck.pending_seek.get().is_none() {
            deck.fade_to(0.0, fade::MANUAL_FADE, Shape::Smooth, Then::Pause);
        } else {
            deck.ramp.set(None);
            deck.then.set(Then::Nothing);
            deck.then_at.set(None);
            if let Some(position) = deck.pending_seek.take() {
                seek(&deck.playbin, position);
            }
            deck.set_state(gst::State::Paused);
        }
        self.quiet_outgoing();
        self.ensure_timer();
    }

    /// What still fades out of a crossfade goes quiet quickly.
    fn quiet_outgoing(&self) {
        let other = &self.decks[self.other()];
        if other.role.get() == Role::Outgoing {
            other.fade_to(0.0, fade::MANUAL_FADE, Shape::Smooth, Then::Stop);
        }
    }

    fn resume(self: &Rc<Self>) {
        let deck = self.active();
        deck.target_playing.set(true);
        if deck.then.get() == Then::Pause {
            // Still fading out to pause: come back from where it is.
            deck.fade_to(1.0, fade::MANUAL_FADE, Shape::Smooth, Then::Nothing);
        } else if !deck.is_playing() {
            if self.fades.get() && deck.start_at.get().is_none() {
                deck.fade_in.set(true);
                self.set_gain_now(deck, 0.0);
            } else if !deck.fade_in.get() {
                // Never left silent by an earlier fade.
                self.set_gain_now(deck, 1.0);
            }
            if !deck.buffering.get() && deck.start_at.get().is_none() {
                deck.set_state(gst::State::Playing);
            }
        }
        self.ensure_timer();
    }

    fn seek(self: &Rc<Self>, position: f64) {
        // A seek during a crossfade is about the new track: the old one
        // goes quiet quickly instead of fading on under it.
        self.quiet_outgoing();
        self.ensure_timer();
        let deck = self.active();
        if deck.start_at.get().is_some() {
            // Not pre-rolled yet: seek there once it is.
            deck.start_at.set(Some(position));
            return;
        }
        if deck.pending_seek.get().is_some() {
            // The fade-out before an earlier seek still runs.
            deck.pending_seek.set(Some(position));
            return;
        }
        if !deck.target_playing.get() {
            // Paused, or fading out to pause: pause at once, then seek;
            // resuming fades in.
            if deck.then.get() == Then::Pause {
                deck.ramp.set(None);
                deck.then.set(Then::Nothing);
                deck.then_at.set(None);
                self.set_gain_now(deck, 0.0);
                deck.set_state(gst::State::Paused);
            }
            seek(&deck.playbin, position);
            self.check_armed_after_seek();
            return self.ensure_timer();
        }
        if self.fades.get() && deck.is_audible() {
            deck.pending_seek.set(Some(position));
            deck.fade_to(0.0, fade::SEEK_FADE, Shape::Smooth, Then::Seek);
            self.ensure_timer();
        } else {
            seek(&deck.playbin, position);
        }
        self.check_armed_after_seek();
    }

    fn set_next(self: &Rc<Self>, next: Option<NextStream>) -> bool {
        let next = next.map(|mut next| {
            if next.crossfade.is_some() && self.preroll_failed.get() == Some(next.track_id) {
                next.crossfade = None;
            }
            next
        });
        if !lock(&self.active().shared).offer(next.clone()) {
            return false;
        }
        let wanted = next.and_then(|next| next.crossfade.map(|length| (next, length)));
        let same = matches!(
            (&*self.armed.borrow(), &wanted),
            (Some(armed), Some((next, length))) if armed.next == *next && armed.length == *length
        );
        if !same {
            self.disarm();
            if let Some((next, length)) = wanted {
                log::debug!(
                    "crossfade armed: track {} over {:.1} s",
                    next.track_id,
                    length.as_secs_f64()
                );
                self.armed.replace(Some(Armed {
                    next,
                    length,
                    deck: None,
                    due: false,
                }));
            }
        }
        self.ensure_timer();
        true
    }

    /// Forgets the crossfade waiting, stopping its pre-roll.
    fn disarm(&self) {
        let Some(armed) = self.armed.take() else {
            return;
        };
        if let Some(index) = armed.deck {
            let deck = &self.decks[index];
            if deck.role.get() == Role::Preroll {
                deck.cut();
            }
        }
    }

    /// After a seek: a pre-roll far ahead of where playback now is goes.
    fn check_armed_after_seek(&self) {
        let deck = self.active();
        let (Some(position), Some(duration)) = (deck.position(), deck.duration()) else {
            return;
        };
        let mut armed = self.armed.borrow_mut();
        let Some(armed) = armed.as_mut() else { return };
        let left = transition::crossfade_start(duration, armed.length) - position;
        if left > 0.0 {
            armed.due = false;
        }
        if left > PREROLL_LEAD + 5.0
            && let Some(index) = armed.deck.take()
        {
            log::debug!("seeked back: dropping the crossfade pre-roll");
            armed.due = false;
            self.decks[index].cut();
        }
    }

    /// Runs the timer: steps fades, starts crossfades.
    fn ensure_timer(self: &Rc<Self>) {
        let ramping = self.decks.iter().any(Deck::is_ramping);
        let wanted = if ramping {
            Some(RAMP_TICK)
        } else if self.armed.borrow().is_some() {
            Some(WATCH_TICK)
        } else {
            None
        };
        let current = self.timer.borrow().as_ref().map(|(_, every)| *every);
        if current == wanted {
            return;
        }
        if let Some((source, _)) = self.timer.take() {
            source.remove();
        }
        if let Some(every) = wanted {
            let weak: Weak<Self> = Rc::downgrade(self);
            let source = glib::timeout_add_local(every, move || {
                if let Some(inner) = weak.upgrade() {
                    inner.tick();
                }
                glib::ControlFlow::Continue
            });
            self.timer.replace(Some((source, every)));
        }
    }

    fn tick(self: &Rc<Self>) {
        let now = Instant::now();
        for deck in &self.decks {
            self.step(deck, now);
        }
        self.watch_crossfade();
        self.ensure_timer();
    }

    /// Advances a deck's fade, and acts once it is over.
    fn step(&self, deck: &Deck, now: Instant) {
        if let Some(ramp) = deck.ramp.get() {
            self.set_gain_now(deck, ramp.value(now));
            if !ramp.is_done(now) {
                return;
            }
            deck.ramp.set(None);
            let tail = if ramp.target() < 0.001 {
                deck.tail.get()
            } else {
                Duration::ZERO
            };
            deck.then_at.set(Some(now + tail));
        }
        if deck.then_at.get().is_some_and(|at| now >= at) {
            deck.then_at.set(None);
            match deck.then.replace(Then::Nothing) {
                Then::Nothing => {}
                Then::Pause => deck.set_state(gst::State::Paused),
                Then::Stop => {
                    log::debug!("{}: faded out, stopped", deck.name);
                    deck.cut();
                }
                Then::Seek => {
                    if let Some(position) = deck.pending_seek.take() {
                        if self.fades.get() {
                            deck.fade_in.set(true);
                        } else {
                            self.set_gain_now(deck, 1.0);
                        }
                        seek(&deck.playbin, position);
                    }
                }
            }
        }
    }

    /// Pre-rolls and starts the armed crossfade when its time comes.
    fn watch_crossfade(self: &Rc<Self>) {
        let Some(armed) = self.armed.borrow().clone() else {
            return;
        };
        let deck = self.active();
        if deck.role.get() != Role::Active || !deck.target_playing.get() || !deck.is_playing() {
            return;
        }
        let (Some(position), Some(duration)) = (deck.position(), deck.duration()) else {
            return;
        };
        let left = transition::crossfade_start(duration, armed.length) - position;
        if armed.deck.is_none() {
            if left > PREROLL_LEAD || self.decks[self.other()].role.get() == Role::Outgoing {
                // Too early, or the other deck still fades out: later.
                return;
            }
            self.preroll(&armed.next);
        }
        if left <= 0.0 {
            let ready = self
                .armed
                .borrow()
                .as_ref()
                .and_then(|a| a.deck)
                .is_some_and(|index| self.decks[index].prerolled.get());
            if ready {
                let length = armed
                    .length
                    .min(Duration::from_secs_f64((duration - position).max(0.0)));
                self.begin_crossfade(length);
            } else if let Some(armed) = self.armed.borrow_mut().as_mut() {
                armed.due = true;
            }
        }
    }

    /// Loads the next track paused on the other deck, silent.
    fn preroll(self: &Rc<Self>, next: &NextStream) {
        let index = self.other();
        let deck = &self.decks[index];
        log::debug!(
            "crossfade: pre-rolling track {} on {}",
            next.track_id,
            deck.name
        );
        deck.setup(
            &next.uri,
            false,
            None,
            next.gain,
            self.output_wanted.borrow().clone(),
        );
        deck.role.set(Role::Preroll);
        self.set_gain_now(deck, 0.0);
        deck.playbin.set_property("mute", self.muted.get());
        if let Some(armed) = self.armed.borrow_mut().as_mut() {
            armed.deck = Some(index);
        }
    }

    /// Starts the pre-rolled next track and fades between the decks over
    /// `length` (zero: switches at once). The next track becomes active.
    fn begin_crossfade(self: &Rc<Self>, length: Duration) {
        let Some(armed) = self.armed.take() else {
            return;
        };
        let Some(index) = armed.deck else { return };
        let out = self.active();
        let inc = &self.decks[index];
        log::info!(
            "crossfade into track {} over {:.1} s",
            armed.next.track_id,
            length.as_secs_f64()
        );
        lock(&out.shared).reset();
        out.role.set(Role::Outgoing);
        out.target_playing.set(false);
        inc.role.set(Role::Active);
        inc.target_playing.set(true);
        if length.is_zero() {
            out.cut();
            self.set_gain_now(inc, 1.0);
        } else {
            out.fade_to(0.0, length, Shape::EqualPower, Then::Stop);
            inc.fade_to(1.0, length, Shape::EqualPower, Then::Nothing);
        }
        inc.set_state(gst::State::Playing);
        self.active.set(index);
        self.ensure_timer();
        self.emit(EngineEvent::NextStarted(armed.next.track_id));
    }

    fn on_message(self: &Rc<Self>, index: usize, message: &gst::Message) {
        use gst::MessageView;
        let deck = &self.decks[index];
        if let MessageView::StreamStart(_) = message.view()
            && deck.awaiting_first_start.replace(false)
        {
            return;
        }
        match deck.role.get() {
            Role::Active => self.active_message(deck, message),
            Role::Preroll => self.preroll_message(deck, message),
            Role::Outgoing | Role::Idle => match message.view() {
                MessageView::Eos(_) => deck.cut(),
                MessageView::Error(err) => {
                    log::debug!(
                        "{} failed while fading out: {}",
                        deck.name,
                        failure::redact(err.error().message())
                    );
                    deck.cut();
                }
                _ => {}
            },
        }
    }

    fn preroll_message(self: &Rc<Self>, deck: &Deck, message: &gst::Message) {
        use gst::MessageView;
        match message.view() {
            MessageView::AsyncDone(_) => {
                deck.prerolled.set(true);
                deck.measure_tail();
                let due = self.armed.borrow().as_ref().is_some_and(|a| a.due);
                let active = self.active();
                let playing = active.role.get() == Role::Active
                    && active.target_playing.get()
                    && active.is_playing();
                if due && playing {
                    // Late: fade over what is left of the current track.
                    let active = self.active();
                    let left = active
                        .duration()
                        .zip(active.position())
                        .map_or(0.0, |(d, p)| (d - p).max(0.0));
                    let length = self
                        .armed
                        .borrow()
                        .as_ref()
                        .map_or(Duration::ZERO, |a| a.length)
                        .min(Duration::from_secs_f64(left));
                    self.begin_crossfade(length);
                }
            }
            MessageView::Buffering(buffering) => {
                deck.buffering.set(buffering.percent() < 100);
            }
            MessageView::Error(err) => {
                let track = self.armed.borrow().as_ref().map(|a| a.next.track_id);
                log::warn!(
                    "could not pre-roll track {track:?} for the crossfade: {}",
                    failure::redact(err.error().message())
                );
                deck.cut();
                // Fall back to the gapless switch, if it is not too late.
                if let Some(armed) = self.armed.take() {
                    self.preroll_failed.set(Some(armed.next.track_id));
                    if armed.next.gapless_fallback {
                        let next = NextStream {
                            crossfade: None,
                            ..armed.next
                        };
                        let _ = lock(&self.active().shared).offer(Some(next));
                    }
                }
            }
            _ => {}
        }
    }

    fn active_message(self: &Rc<Self>, deck: &Deck, message: &gst::Message) {
        use gst::MessageView;
        let playbin = &deck.playbin;
        match message.view() {
            MessageView::Eos(_) => {
                let pending = self.armed.borrow().as_ref().and_then(|a| a.deck);
                if pending.is_some_and(|index| self.decks[index].prerolled.get()) {
                    // The crossfade never got its moment (a seek to the
                    // very end): switch straight to the next track.
                    return self.begin_crossfade(Duration::ZERO);
                }
                self.disarm();
                let _ = playbin.set_state(gst::State::Null);
                self.emit(EngineEvent::Finished);
            }
            MessageView::Error(err) => {
                let _ = playbin.set_state(gst::State::Null);
                deck.start_at.set(None);
                deck.pending_seek.set(None);
                self.disarm();
                let error = err.error();
                let element = err.src().and_then(|src| src.downcast_ref::<gst::Element>());
                let factory = element
                    .and_then(gst::Element::factory)
                    .map(|factory| factory.name().to_string())
                    .unwrap_or_default();
                let from_sink = element.is_some_and(|element| {
                    element.element_flags().contains(gst::ElementFlags::SINK)
                        && !element.is::<gst::Bin>()
                }) || error.kind::<gst::ResourceError>().is_some_and(|code| {
                    // Sinks report a device that is gone or busy this way.
                    matches!(
                        code,
                        gst::ResourceError::OpenWrite | gst::ResourceError::Write
                    )
                });
                let debug = err.debug().map(|d| failure::redact(&d)).unwrap_or_default();
                let resource_io = error.kind::<gst::ResourceError>().is_some_and(|code| {
                    matches!(
                        code,
                        gst::ResourceError::Read
                            | gst::ResourceError::OpenRead
                            | gst::ResourceError::Seek
                            | gst::ResourceError::Failed
                            | gst::ResourceError::Busy
                    )
                });
                let info = EngineError {
                    resource_io,
                    from_http: failure::is_http_element(&factory),
                    http_status: err
                        .details()
                        .and_then(|d| d.get::<u32>("http-status-code").ok()),
                    download_error: debug.contains("Download error"),
                };
                let kind = failure::classify_engine(&info);
                let message = failure::redact(error.message());
                log::warn!("playback error ({kind:?}) from {factory}: {message}");
                log::debug!("playback error details: {debug} ({info:?})");
                self.emit(EngineEvent::Error {
                    message,
                    kind,
                    from_sink,
                });
            }
            MessageView::StreamStart(_) => {
                let (started, gain, filter) = {
                    let mut shared = lock(&deck.shared);
                    (
                        shared.handed_over.take(),
                        shared.pending_gain.take(),
                        shared.filter.clone(),
                    )
                };
                // The pad probe normally applied it already.
                if let (Some(gain), Some(filter)) = (gain, filter) {
                    filter.set_property("volume", gain.clamp(0.0, 10.0));
                }
                if let Some(track_id) = started {
                    self.emit(EngineEvent::NextStarted(track_id));
                }
            }
            MessageView::AsyncDone(_) => {
                log::trace!(
                    "async-done (start {:?}, seeking {}, buffering {})",
                    deck.start_at.get(),
                    deck.seeking_to_start.get(),
                    deck.buffering.get()
                );
                if let Some(start) = deck.start_at.take() {
                    deck.seeking_to_start.set(true);
                    seek(playbin, start);
                } else if deck.seeking_to_start.replace(false)
                    && deck.target_playing.get()
                    && !deck.buffering.get()
                {
                    let _ = playbin.set_state(gst::State::Playing);
                } else if deck.target_playing.get() && !deck.buffering.get() && is_playing(playbin)
                {
                    // A flushing seek while playing: the pipeline recovered by
                    // itself, possibly without a state change to report.
                    self.start_fade_in(deck);
                    self.emit(EngineEvent::Playing);
                }
            }
            MessageView::StateChanged(change) if message.src() == Some(playbin.upcast_ref()) => {
                log::trace!(
                    "{}: state {:?} -> {:?} (pending {:?})",
                    deck.name,
                    change.old(),
                    change.current(),
                    change.pending()
                );
                match change.current() {
                    gst::State::Playing => {
                        if change.old() != gst::State::Playing {
                            deck.measure_tail();
                        }
                        self.start_fade_in(deck);
                        self.emit(EngineEvent::Playing);
                    }
                    gst::State::Paused if change.pending() == gst::State::VoidPending => {
                        if !deck.target_playing.get() {
                            self.emit(EngineEvent::Paused);
                        } else if deck.buffering.get() {
                            self.emit(EngineEvent::Buffering);
                        }
                    }
                    _ => {}
                }
            }
            MessageView::Buffering(buffering) => {
                let percent = buffering.percent();
                log::trace!("buffering {percent} %");
                if percent < 100 {
                    // A handed-over stream fills up while the end of the
                    // current one still plays from the queues: pausing now
                    // would break the gapless switch.
                    if lock(&deck.shared).handed_over.is_some() {
                        return;
                    }
                    if !deck.buffering.replace(true) {
                        log::debug!("buffering ({percent} %)");
                        // Paused stays paused: only a wanted playback waits.
                        if deck.target_playing.get() {
                            let _ = playbin.set_state(gst::State::Paused);
                            self.emit(EngineEvent::Buffering);
                        }
                    }
                } else if deck.buffering.replace(false) {
                    log::debug!("buffering done");
                    let starting = deck.start_at.get().is_some() || deck.seeking_to_start.get();
                    if !deck.target_playing.get() {
                        self.emit(EngineEvent::Paused);
                    } else if !starting {
                        // Always ask: the pipeline may still be on its way to
                        // the pause asked for when buffering began.
                        let _ = playbin.set_state(gst::State::Playing);
                        if is_playing(playbin) {
                            self.emit(EngineEvent::Playing);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The deck plays: a fade-in waiting for it begins.
    fn start_fade_in(self: &Rc<Self>, deck: &Deck) {
        if deck.fade_in.replace(false) && deck.then.get() == Then::Nothing {
            deck.fade_to(1.0, fade::MANUAL_FADE, Shape::Smooth, Then::Nothing);
            self.ensure_timer();
        }
    }
}

/// `audioconvert ! volume`, the volume returned separately for gain
/// changes. A pad probe switches to a handed-over stream's gain exactly
/// where that stream begins.
fn normalization_filter(
    gain: f64,
    shared: &Arc<Mutex<Shared>>,
) -> Result<(gst::Element, gst::Element)> {
    let bin = gst::Bin::new();
    let convert = gst::ElementFactory::make("audioconvert").build()?;
    let volume = gst::ElementFactory::make("volume")
        .property("volume", gain.clamp(0.0, 10.0))
        .build()?;
    bin.add_many([&convert, &volume])?;
    convert.link(&volume)?;
    let sink = convert
        .static_pad("sink")
        .context("audioconvert has no sink")?;
    let src = volume.static_pad("src").context("volume has no src")?;
    let weak = Arc::downgrade(shared);
    let target = volume.downgrade();
    volume
        .static_pad("sink")
        .context("volume has no sink")?
        .add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
            let is_start = matches!(
                &info.data,
                Some(gst::PadProbeData::Event(event)) if event.type_() == gst::EventType::StreamStart
            );
            if !is_start {
                return gst::PadProbeReturn::Ok;
            }
            let gain = weak
                .upgrade()
                .and_then(|shared| lock(&shared).pending_gain.take());
            if let (Some(gain), Some(volume)) = (gain, target.upgrade()) {
                volume.set_property("volume", gain.clamp(0.0, 10.0));
            }
            gst::PadProbeReturn::Ok
        });
    bin.add_pad(&gst::GhostPad::with_target(&sink)?)?;
    bin.add_pad(&gst::GhostPad::with_target(&src)?)?;
    Ok((bin.upcast(), volume))
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some((source, _)) = self.inner.timer.take() {
            source.remove();
        }
        for deck in &self.inner.decks {
            let _ = deck.playbin.set_state(gst::State::Null);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(track_id: u64) -> NextStream {
        NextStream {
            track_id,
            uri: format!("file:///{track_id}.mpd"),
            gain: None,
            crossfade: None,
            gapless_fallback: true,
        }
    }

    /// The race between `about-to-finish` (streaming thread) and the main
    /// thread replacing the next stream: once handed over, offers are
    /// refused, so the stream that will play is never dropped or replaced.
    #[test]
    fn handed_over_stream_cannot_be_replaced() {
        let mut shared = Shared::default();
        assert!(shared.offer(Some(next(1))));
        assert!(shared.offer(Some(next(2))));
        assert_eq!(shared.hand_over().map(|n| n.track_id), Some(2));
        assert_eq!(shared.handed_over, Some(2));
        assert!(!shared.offer(Some(next(3))));
        assert!(!shared.offer(None));
        assert!(shared.next.is_none());
        assert_eq!(shared.handed_over, Some(2));
        // Nothing waits: a second about-to-finish takes nothing.
        assert!(shared.hand_over().is_none());
        shared.reset();
        assert!(shared.offer(None));
    }

    /// A stream meant for a crossfade is never handed over gaplessly: the
    /// engine starts it on the other deck itself.
    #[test]
    fn crossfade_stream_is_not_handed_over() {
        let mut shared = Shared::default();
        let crossfaded = NextStream {
            crossfade: Some(Duration::from_secs(6)),
            ..next(4)
        };
        assert!(shared.offer(Some(crossfaded.clone())));
        assert!(shared.hand_over().is_none());
        assert_eq!(shared.next, Some(crossfaded));
        assert!(shared.handed_over.is_none());
        // Still replaceable, e.g. by the gapless fallback.
        assert!(shared.offer(Some(next(4))));
        assert_eq!(shared.hand_over().map(|n| n.track_id), Some(4));
    }

    /// A load during a crossfade (both decks fading out) stops the quieter
    /// deck, never the louder one.
    #[test]
    fn loads_take_a_free_or_the_quieter_deck() {
        use Role::{Idle, Outgoing};
        assert!(load_on_active((Idle, 0.0), (Outgoing, 0.5)));
        assert!(!load_on_active((Outgoing, 0.9), (Idle, 0.0)));
        // Skipping early in a crossfade: the old track is still loud.
        assert!(load_on_active((Outgoing, 0.3), (Outgoing, 0.95)));
        // Late in it: the new track is the louder one.
        assert!(!load_on_active((Outgoing, 0.9), (Outgoing, 0.2)));
    }

    #[test]
    fn finds_dash_codecs() {
        let mpd = r#"<MPD><Period><AdaptationSet><Representation id="FLAC,96000,24" codecs="flac" bandwidth="1"/>"#;
        assert_eq!(dash_codecs(mpd).as_deref(), Some("flac"));
        assert_eq!(dash_codecs("<MPD/>"), None);
    }
}
