//! Audio playback with GStreamer's `playbin3`.
//!
//! TIDAL hands out two kinds of stream:
//! - Hi-Res Lossless as an MPEG-DASH manifest (FLAC in fragmented MP4). The
//!   manifest is written to one file per track and played as a `file://`
//!   URI; `dashdemux2` then fetches the segments itself, streaming.
//! - Everything else as a "BTS" manifest: JSON with a direct URL to the file.
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

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gst::prelude::*;
use gtk::glib;
use serde::Deserialize;

use super::failure::{self, EngineError, Failure};
use super::format::{FormatDetails, ReplayGain};
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
}

impl Stream {
    /// Decodes the manifest of `info`, TIDAL's answer for `track_id` (the
    /// id asked for, which the answer's own id need not match). DASH
    /// manifests are written to disk, so this runs on the I/O runtime.
    pub async fn from_playback_info(track_id: u64, info: PlaybackInfo) -> Result<Self> {
        if info.track_id != track_id {
            log::debug!("track {track_id} streams as track {}", info.track_id);
        }
        let manifest = BASE64
            .decode(info.manifest.as_bytes())
            .context("the playback manifest is not base64")?;
        let mime = info.manifest_mime_type.to_ascii_lowercase();

        let (uri, codecs) = if mime.starts_with("application/dash+xml") {
            let codecs = dash_codecs(&String::from_utf8_lossy(&manifest));
            let path = write_manifest(track_id, &manifest).await?;
            (glib::filename_to_uri(&path, None)?.to_string(), codecs)
        } else if mime.starts_with("application/vnd.tidal.bts") {
            let bts: BtsManifest = serde_json::from_slice(&manifest)?;
            if !bts.encryption_type.eq_ignore_ascii_case("NONE") {
                bail!(
                    "encrypted streams ({}) are not supported",
                    bts.encryption_type
                );
            }
            let uri = bts
                .urls
                .into_iter()
                .next()
                .context("the manifest lists no URL")?;
            (uri, bts.codecs)
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
        })
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

/// The first `codecs="…"` of an MPD.
fn dash_codecs(mpd: &str) -> Option<String> {
    let start = mpd.find("codecs=\"")? + "codecs=\"".len();
    let end = mpd[start..].find('"')?;
    Some(mpd[start..start + end].to_owned())
}

/// Tracks whose manifests must survive pruning: the current and the next.
static KEEP_MANIFESTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Sets which tracks' DASH manifests are in use (the current one and the
/// one prepared for gapless playback); the others are deleted when the
/// next manifest is written.
pub fn keep_manifests(track_ids: &[u64]) {
    *KEEP_MANIFESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = track_ids.to_vec();
}

/// Manifests carry signed segment URLs, so they go to this process's
/// private, in-memory runtime directory, one file per track.
async fn write_manifest(track_id: u64, manifest: &[u8]) -> Result<PathBuf> {
    let dir = super::rundir::dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{track_id}.mpd"));
    let partial = super::rundir::temp_path(&path);
    let written = match tokio::fs::write(&partial, manifest).await {
        Ok(()) => tokio::fs::rename(&partial, &path).await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(err.into());
    }

    let keep = KEEP_MANIFESTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let mut entries = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let old = entry.path();
        if old == path || old.extension().is_none_or(|ext| ext != "mpd") {
            continue;
        }
        let id = old
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok());
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
    /// Playback failed; `kind` says whether the network is to blame.
    Error { message: String, kind: Failure },
    /// The track handed over with [`Engine::set_next`] started playing.
    NextStarted(u64),
}

/// The next track, ready for a gapless switch.
#[derive(Clone, Debug)]
pub struct NextStream {
    pub track_id: u64,
    pub uri: String,
    /// ReplayGain factor; ignored when no normalisation filter is in place.
    pub gain: Option<f64>,
}

/// State shared with GStreamer's streaming threads.
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

/// Main-thread state of the bus handler.
#[derive(Default)]
struct Local {
    /// What the user wants: playing or paused.
    target_playing: Cell<bool>,
    buffering: Cell<bool>,
    /// Seek to this once pre-rolled (resuming at a position).
    start_at: Cell<Option<f64>>,
    /// The start seek is in flight.
    seeking_to_start: Cell<bool>,
    /// The first stream of a load has not started yet.
    awaiting_first_start: Cell<bool>,
    /// The output device asked for (display name), `None` = system default.
    output_wanted: RefCell<Option<String>>,
    /// The device the pipeline was last built with.
    output_applied: RefCell<Option<String>>,
}

/// `playbin3`, audio only. Events arrive on the main thread.
pub struct Engine {
    playbin: gst::Element,
    shared: Arc<Mutex<Shared>>,
    local: Rc<Local>,
    _bus_watch: gst::bus::BusWatchGuard,
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

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Engine {
    pub fn new(on_event: impl Fn(EngineEvent) + 'static) -> Result<Self> {
        let playbin = gst::ElementFactory::make("playbin3")
            .build()
            .context("GStreamer's playbin3 is missing (install gst-plugins-base)")?;
        // Audio only: never build video or subtitle branches. Buffering
        // reports network stalls.
        playbin.set_property_from_str("flags", "audio+soft-volume+buffering");

        let shared = Arc::new(Mutex::new(Shared::default()));
        let local = Rc::new(Local::default());

        // Runs on a streaming thread: only hand over what is ready.
        playbin.connect("about-to-finish", false, {
            let shared = Arc::downgrade(&shared);
            move |values| {
                let playbin = values[0].get::<gst::Element>().ok()?;
                let shared = shared.upgrade()?;
                let next = {
                    let mut shared = lock(&shared);
                    let next = shared.next.take()?;
                    shared.handed_over = Some(next.track_id);
                    shared.pending_gain = next.gain;
                    next
                };
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

        let bus = playbin.bus().context("playbin3 has no bus")?;
        let element = playbin.downgrade();
        let watch = bus.add_watch_local({
            let shared = shared.clone();
            let local = local.clone();
            move |_, message| {
                let Some(playbin) = element.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                handle_message(&playbin, &shared, &local, message, &on_event);
                glib::ControlFlow::Continue
            }
        })?;

        Ok(Self {
            playbin,
            shared,
            local,
            _bus_watch: watch,
        })
    }

    /// Starts `uri`, playing or paused, from `start` seconds (or the
    /// beginning). `gain` is the normalisation factor; `None` plays without
    /// any filter (bit-perfect).
    pub fn load(&self, uri: &str, play: bool, start: Option<f64>, gain: Option<f64>) {
        let _ = self.playbin.set_state(gst::State::Null);
        {
            let mut shared = lock(&self.shared);
            shared.next = None;
            shared.handed_over = None;
            shared.pending_gain = None;
        }
        self.install_filter(gain);
        self.apply_output_device();
        self.playbin.set_property("uri", uri);
        let start = start.filter(|s| *s > 0.5);
        self.local.target_playing.set(play);
        self.local.buffering.set(false);
        self.local.start_at.set(start);
        self.local.seeking_to_start.set(false);
        self.local.awaiting_first_start.set(true);
        self.set_state(if play && start.is_none() {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
    }

    /// Chooses the output device for the next load (display name of a
    /// GStreamer `Audio/Sink` device; `None` = the system default).
    pub fn set_output_device(&self, name: Option<String>) {
        self.local.output_wanted.replace(name);
    }

    /// Builds the audio sink for the wanted device. Only while the pipeline
    /// is in `Null` (from `load`).
    fn apply_output_device(&self) {
        let wanted = self.local.output_wanted.borrow().clone();
        if *self.local.output_applied.borrow() == wanted {
            return;
        }
        let sink = wanted.as_deref().and_then(|name| match audio_sink_for(name) {
            Some(sink) => Some(sink),
            None => {
                log::warn!("output device “{name}” not found; using the default");
                None
            }
        });
        self.playbin.set_property("audio-sink", sink.as_ref());
        self.local.output_applied.replace(wanted);
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

    /// Whether the pipeline has the normalisation filter (as last loaded).
    pub fn has_filter(&self) -> bool {
        lock(&self.shared).filter.is_some()
    }

    /// Changes the normalisation factor of what is playing now.
    pub fn set_gain(&self, gain: f64) {
        let filter = lock(&self.shared).filter.clone();
        if let Some(filter) = filter {
            filter.set_property("volume", gain.clamp(0.0, 10.0));
        }
    }

    /// Changes the gain waiting for the handed-over stream (normalisation
    /// changed during the gapless switch).
    pub fn set_pending_gain(&self, gain: Option<f64>) {
        let mut shared = lock(&self.shared);
        if shared.handed_over.is_some() {
            shared.pending_gain = gain;
        }
    }

    /// Debugging (`SWELL_DEBUG_ENGINE_FAIL`): posts the error an HTTP
    /// source gives when the connection drops.
    pub fn simulate_network_error(&self) {
        let details = gst::Structure::builder("details")
            .field("http-status-code", 0u32)
            .build();
        let message = gst::message::Error::builder(gst::ResourceError::Read, "Simulated connection loss")
            .debug("simulated, URL: https://example.invalid/segment.mp4?token=SECRET")
            .details(details)
            .src(&self.playbin)
            .build();
        if let Some(bus) = self.playbin.bus() {
            let _ = bus.post(message);
        }
    }

    /// The stream to switch to, gaplessly, when the current one ends.
    pub fn set_next(&self, next: Option<NextStream>) {
        lock(&self.shared).next = next;
    }

    /// The next stream was handed over but has not started yet: the old one
    /// is still audible, its end already decoded.
    pub fn handover_pending(&self) -> bool {
        lock(&self.shared).handed_over.is_some()
    }

    pub fn resume(&self) {
        self.local.target_playing.set(true);
        if !self.local.buffering.get() && self.local.start_at.get().is_none() {
            self.set_state(gst::State::Playing);
        }
    }

    pub fn pause(&self) {
        self.local.target_playing.set(false);
        self.set_state(gst::State::Paused);
    }

    pub fn stop(&self) {
        self.set_state(gst::State::Null);
        self.local.target_playing.set(false);
        self.local.buffering.set(false);
        self.local.start_at.set(None);
        let mut shared = lock(&self.shared);
        shared.next = None;
        shared.handed_over = None;
        shared.pending_gain = None;
    }

    fn set_state(&self, state: gst::State) {
        if let Err(err) = self.playbin.set_state(state) {
            log::warn!("could not switch playback to {state:?}: {err}");
        }
    }

    /// Amplitude factor 0.0–1.0 (already curved by the caller).
    pub fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
    }

    pub fn set_muted(&self, muted: bool) {
        self.playbin.set_property("mute", muted);
    }

    /// Audio is flowing: the pipeline settled in `Playing` and is not
    /// waiting for data.
    pub fn is_playing(&self) -> bool {
        !self.local.buffering.get()
            && self.local.start_at.get().is_none()
            && is_playing(&self.playbin)
    }

    /// Waiting for data or for the start position: not audible yet even
    /// if wanted.
    pub fn is_waiting(&self) -> bool {
        self.local.buffering.get()
            || self.local.start_at.get().is_some()
            || self.local.seeking_to_start.get()
    }

    /// Seconds.
    pub fn position(&self) -> Option<f64> {
        if let Some(start) = self.local.start_at.get() {
            return Some(start);
        }
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
    }

    /// Seconds.
    pub fn duration(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
            .filter(|d| *d > 0.0)
    }

    /// Seconds.
    pub fn seek(&self, position: f64) {
        if self.local.start_at.get().is_some() {
            // Not pre-rolled yet: seek there once it is.
            self.local.start_at.set(Some(position));
            return;
        }
        seek(&self.playbin, position);
    }

    /// The sample format and rate going to the sound server, e.g.
    /// `("S24_32LE", 96000)`.
    pub fn output_format(&self) -> Option<(String, i32)> {
        let bin = self.playbin.downcast_ref::<gst::Bin>()?;
        let mut elements = bin.iterate_recurse();
        while let Ok(Some(element)) = elements.next() {
            let is_sink = element.element_flags().contains(gst::ElementFlags::SINK)
                && !element.is::<gst::Bin>();
            if !is_sink {
                continue;
            }
            let caps = element
                .static_pad("sink")
                .and_then(|pad| pad.current_caps());
            if let Some(structure) = caps.as_ref().and_then(|caps| caps.structure(0)) {
                let format = structure.get::<String>("format").ok()?;
                let rate = structure.get::<i32>("rate").ok()?;
                return Some((format, rate));
            }
        }
        None
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

/// `audioconvert ! volume`, the volume returned separately for gain
/// changes. A pad probe switches to a handed-over stream's gain exactly
/// where that stream begins.
fn normalization_filter(
    gain: f64,
    shared: &Arc<Mutex<Shared>>,
) -> Result<(gst::Element, gst::Element)> {
    let bin = gst::Bin::with_name("normalization");
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

fn handle_message(
    playbin: &gst::Element,
    shared: &Arc<Mutex<Shared>>,
    local: &Local,
    message: &gst::Message,
    on_event: &impl Fn(EngineEvent),
) {
    use gst::MessageView;
    match message.view() {
        MessageView::Eos(_) => {
            let _ = playbin.set_state(gst::State::Null);
            on_event(EngineEvent::Finished);
        }
        MessageView::Error(err) => {
            let _ = playbin.set_state(gst::State::Null);
            local.start_at.set(None);
            let error = err.error();
            let factory = err
                .src()
                .and_then(|src| src.downcast_ref::<gst::Element>())
                .and_then(gst::Element::factory)
                .map(|factory| factory.name().to_string())
                .unwrap_or_default();
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
            on_event(EngineEvent::Error { message, kind });
        }
        MessageView::StreamStart(_) => {
            if local.awaiting_first_start.replace(false) {
                return;
            }
            let (started, gain, filter) = {
                let mut shared = lock(shared);
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
                on_event(EngineEvent::NextStarted(track_id));
            }
        }
        MessageView::AsyncDone(_) => {
            log::trace!(
                "async-done (start {:?}, seeking {}, buffering {})",
                local.start_at.get(),
                local.seeking_to_start.get(),
                local.buffering.get()
            );
            if let Some(start) = local.start_at.take() {
                local.seeking_to_start.set(true);
                seek(playbin, start);
            } else if local.seeking_to_start.replace(false)
                && local.target_playing.get()
                && !local.buffering.get()
            {
                let _ = playbin.set_state(gst::State::Playing);
            } else if local.target_playing.get() && !local.buffering.get() && is_playing(playbin) {
                // A flushing seek while playing: the pipeline recovered by
                // itself, possibly without a state change to report.
                on_event(EngineEvent::Playing);
            }
        }
        MessageView::StateChanged(change) if message.src() == Some(playbin.upcast_ref()) => {
            log::trace!(
                "state {:?} -> {:?} (pending {:?})",
                change.old(),
                change.current(),
                change.pending()
            );
            match change.current() {
                gst::State::Playing => on_event(EngineEvent::Playing),
                gst::State::Paused if change.pending() == gst::State::VoidPending => {
                    if !local.target_playing.get() {
                        on_event(EngineEvent::Paused);
                    } else if local.buffering.get() {
                        on_event(EngineEvent::Buffering);
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
                if lock(shared).handed_over.is_some() {
                    return;
                }
                if !local.buffering.replace(true) {
                    log::debug!("buffering ({percent} %)");
                    // Paused stays paused: only a wanted playback waits.
                    if local.target_playing.get() {
                        let _ = playbin.set_state(gst::State::Paused);
                        on_event(EngineEvent::Buffering);
                    }
                }
            } else if local.buffering.replace(false) {
                log::debug!("buffering done");
                let starting = local.start_at.get().is_some() || local.seeking_to_start.get();
                if !local.target_playing.get() {
                    on_event(EngineEvent::Paused);
                } else if !starting {
                    // Always ask: the pipeline may still be on its way to
                    // the pause asked for when buffering began.
                    let _ = playbin.set_state(gst::State::Playing);
                    if is_playing(playbin) {
                        on_event(EngineEvent::Playing);
                    }
                }
            }
        }
        _ => {}
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dash_codecs() {
        let mpd = r#"<MPD><Period><AdaptationSet><Representation id="FLAC,96000,24" codecs="flac" bandwidth="1"/>"#;
        assert_eq!(dash_codecs(mpd).as_deref(), Some("flac"));
        assert_eq!(dash_codecs("<MPD/>"), None);
    }
}
