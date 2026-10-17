//! Audio playback with GStreamer's `playbin3`.
//!
//! TIDAL hands out two kinds of stream:
//! - Hi-Res Lossless as an MPEG-DASH manifest (FLAC in fragmented MP4). The
//!   manifest is written to one file per track and played as a `file://`
//!   URI; `dashdemux2` then fetches the segments itself, streaming.
//! - Everything else as a "BTS" manifest: JSON with a direct URL to the file.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gst::prelude::*;
use gtk::glib;
use serde::Deserialize;
use tidalrs::TrackDashPlaybackInfo;

/// What TIDAL returned for a track, resolved to something `playbin3` plays.
pub struct Stream {
    pub uri: String,
    /// Human-readable format, e.g. "24-bit/96 kHz FLAC".
    pub format: String,
}

impl Stream {
    /// Decodes the manifest of `info`. DASH manifests are written to disk,
    /// so this runs on the I/O runtime.
    pub async fn from_playback_info(info: TrackDashPlaybackInfo) -> Result<Self> {
        let manifest = BASE64
            .decode(info.manifest.as_bytes())
            .context("the playback manifest is not base64")?;
        let mime = info.manifest_mime_type.to_ascii_lowercase();
        let format = describe_format(&info);

        let uri = if mime.starts_with("application/dash+xml") {
            let path = write_manifest(info.track_id, &manifest).await?;
            glib::filename_to_uri(&path, None)?.to_string()
        } else if mime.starts_with("application/vnd.tidal.bts") {
            let bts: BtsManifest = serde_json::from_slice(&manifest)?;
            if !bts.encryption_type.eq_ignore_ascii_case("NONE") {
                bail!(
                    "encrypted streams ({}) are not supported",
                    bts.encryption_type
                );
            }
            bts.urls
                .into_iter()
                .next()
                .context("the manifest lists no URL")?
        } else {
            bail!("unsupported manifest type {}", info.manifest_mime_type);
        };
        Ok(Self { uri, format })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BtsManifest {
    encryption_type: String,
    urls: Vec<String>,
}

fn describe_format(info: &TrackDashPlaybackInfo) -> String {
    match (info.bit_depth, info.sample_rate) {
        (Some(bits), Some(rate)) => {
            let khz = f64::from(rate) / 1000.0;
            format!("{bits}-bit/{khz} kHz FLAC")
        }
        _ => format!("{:?}", info.audio_quality),
    }
}

/// Manifests carry signed segment URLs, so they go to the private, in-memory
/// runtime directory, one file per track. Only the latest one is kept.
async fn write_manifest(track_id: u64, manifest: &[u8]) -> Result<PathBuf> {
    let dir = glib::user_runtime_dir().join("swell-player");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{track_id}.mpd"));
    tokio::fs::write(&path, manifest).await?;

    let mut entries = tokio::fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let old = entry.path();
        if old != path && old.extension().is_some_and(|ext| ext == "mpd") {
            let _ = tokio::fs::remove_file(old).await;
        }
    }
    Ok(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Buffering,
    Playing,
    Paused,
}

pub enum Event {
    State(State),
    Finished,
    Error(String),
}

pub struct Player {
    playbin: gst::Element,
    /// The state last asked for, or `Stopped` after an error or the end.
    state: Rc<Cell<State>>,
    _bus_watch: gst::bus::BusWatchGuard,
}

impl Player {
    /// `on_event` is called on the main thread.
    pub fn new(on_event: impl Fn(&Event) + 'static) -> Result<Self> {
        let playbin = gst::ElementFactory::make("playbin3")
            .build()
            .context("GStreamer's playbin3 is missing (install gst-plugins-base)")?;
        // Audio only: never build video or subtitle branches.
        playbin.set_property_from_str("flags", "audio+soft-volume");

        let bus = playbin.bus().context("playbin3 has no bus")?;
        let element = playbin.downgrade();
        let state = Rc::new(Cell::new(State::Stopped));
        let watch_state = state.clone();
        let watch = bus.add_watch_local(move |_, message| {
            use gst::MessageView;
            let Some(playbin) = element.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match message.view() {
                MessageView::Eos(_) => {
                    let _ = playbin.set_state(gst::State::Null);
                    watch_state.set(State::Stopped);
                    on_event(&Event::Finished);
                }
                MessageView::Error(err) => {
                    let _ = playbin.set_state(gst::State::Null);
                    watch_state.set(State::Stopped);
                    on_event(&Event::Error(err.error().to_string()));
                    log::warn!("playback error: {} ({:?})", err.error(), err.debug());
                }
                MessageView::StateChanged(change)
                    if message.src() == Some(playbin.upcast_ref()) =>
                {
                    match change.current() {
                        gst::State::Playing => on_event(&Event::State(State::Playing)),
                        gst::State::Paused if change.pending() == gst::State::VoidPending => {
                            on_event(&Event::State(State::Paused));
                        }
                        _ => {}
                    }
                }
                MessageView::Buffering(buffering) if buffering.percent() < 100 => {
                    on_event(&Event::State(State::Buffering));
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })?;

        Ok(Self {
            playbin,
            state,
            _bus_watch: watch,
        })
    }

    pub fn play(&self, uri: &str) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", uri);
        self.set_state(State::Playing);
    }

    pub fn toggle(&self) {
        match self.state.get() {
            State::Playing | State::Buffering => self.set_state(State::Paused),
            State::Paused => self.set_state(State::Playing),
            State::Stopped => {}
        }
    }

    pub fn stop(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.state.set(State::Stopped);
    }

    fn set_state(&self, state: State) {
        let target = match state {
            State::Playing => gst::State::Playing,
            State::Paused => gst::State::Paused,
            State::Stopped | State::Buffering => gst::State::Null,
        };
        if let Err(err) = self.playbin.set_state(target) {
            log::warn!("could not switch playback to {target:?}: {err}");
        }
        self.state.set(state);
    }

    pub fn position(&self) -> Option<gst::ClockTime> {
        self.playbin.query_position()
    }

    pub fn duration(&self) -> Option<gst::ClockTime> {
        self.playbin.query_duration()
    }

    pub fn seek(&self, position: gst::ClockTime) {
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT;
        if let Err(err) = self.playbin.seek_simple(flags, position) {
            log::debug!("seek failed: {err}");
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}
