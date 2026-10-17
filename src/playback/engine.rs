//! Audio playback with GStreamer's `playbin3`.
//!
//! TIDAL hands out two kinds of stream:
//! - Hi-Res Lossless as an MPEG-DASH manifest (FLAC in fragmented MP4). The
//!   manifest is written to one file per track and played as a `file://`
//!   URI; `dashdemux2` then fetches the segments itself, streaming.
//! - Everything else as a "BTS" manifest: JSON with a direct URL to the file.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gst::prelude::*;
use gtk::glib;
use serde::Deserialize;
use crate::tidal::models::PlaybackInfo;

/// What TIDAL returned for a track, resolved to something `playbin3` plays.
pub struct Stream {
    pub uri: String,
    /// Human-readable format, e.g. "24-bit/96 kHz FLAC".
    pub format: String,
}

impl Stream {
    /// Decodes the manifest of `info`. DASH manifests are written to disk,
    /// so this runs on the I/O runtime.
    pub async fn from_playback_info(info: PlaybackInfo) -> Result<Self> {
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

fn describe_format(info: &PlaybackInfo) -> String {
    match (info.bit_depth, info.sample_rate) {
        (Some(bits), Some(rate)) => {
            let khz = f64::from(rate) / 1000.0;
            format!("{bits}-bit/{khz} kHz FLAC")
        }
        _ => match info.audio_quality.as_deref() {
            Some("LOSSLESS") => "16-bit/44.1 kHz FLAC".to_owned(),
            Some("HIGH") => "AAC 320 kbit/s".to_owned(),
            Some("LOW") => "AAC 96 kbit/s".to_owned(),
            other => other.unwrap_or("Unknown format").to_owned(),
        },
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

pub enum EngineEvent {
    Playing,
    Paused,
    Buffering,
    Finished,
    Error(String),
}

/// `playbin3`, audio only. Events arrive on the main thread.
pub struct Engine {
    playbin: gst::Element,
    _bus_watch: gst::bus::BusWatchGuard,
}

impl Engine {
    pub fn new(on_event: impl Fn(EngineEvent) + 'static) -> Result<Self> {
        let playbin = gst::ElementFactory::make("playbin3")
            .build()
            .context("GStreamer's playbin3 is missing (install gst-plugins-base)")?;
        // Audio only: never build video or subtitle branches.
        playbin.set_property_from_str("flags", "audio+soft-volume");

        let bus = playbin.bus().context("playbin3 has no bus")?;
        let element = playbin.downgrade();
        let watch = bus.add_watch_local(move |_, message| {
            use gst::MessageView;
            let Some(playbin) = element.upgrade() else {
                return glib::ControlFlow::Break;
            };
            match message.view() {
                MessageView::Eos(_) => {
                    let _ = playbin.set_state(gst::State::Null);
                    on_event(EngineEvent::Finished);
                }
                MessageView::Error(err) => {
                    let _ = playbin.set_state(gst::State::Null);
                    log::warn!("playback error: {} ({:?})", err.error(), err.debug());
                    on_event(EngineEvent::Error(err.error().to_string()));
                }
                MessageView::StateChanged(change)
                    if message.src() == Some(playbin.upcast_ref()) =>
                {
                    match change.current() {
                        gst::State::Playing => on_event(EngineEvent::Playing),
                        gst::State::Paused if change.pending() == gst::State::VoidPending => {
                            on_event(EngineEvent::Paused);
                        }
                        _ => {}
                    }
                }
                MessageView::Buffering(buffering) if buffering.percent() < 100 => {
                    on_event(EngineEvent::Buffering);
                }
                _ => {}
            }
            glib::ControlFlow::Continue
        })?;

        Ok(Self {
            playbin,
            _bus_watch: watch,
        })
    }

    /// Starts `uri` from the beginning, playing or paused.
    pub fn load(&self, uri: &str, play: bool) {
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", uri);
        self.set_state(if play {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
    }

    pub fn resume(&self) {
        self.set_state(gst::State::Playing);
    }

    pub fn pause(&self) {
        self.set_state(gst::State::Paused);
    }

    pub fn stop(&self) {
        self.set_state(gst::State::Null);
    }

    fn set_state(&self, state: gst::State) {
        if let Err(err) = self.playbin.set_state(state) {
            log::warn!("could not switch playback to {state:?}: {err}");
        }
    }

    /// Linear 0.0–1.0.
    pub fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
    }

    /// Seconds.
    pub fn position(&self) -> Option<f64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
    }

    /// Seconds.
    pub fn duration(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.nseconds() as f64 / 1e9)
    }

    /// Seconds.
    pub fn seek(&self, position: f64) {
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT;
        let target = gst::ClockTime::from_nseconds((position * 1e9) as u64);
        if let Err(err) = self.playbin.seek_simple(flags, target) {
            log::debug!("seek failed: {err}");
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}
