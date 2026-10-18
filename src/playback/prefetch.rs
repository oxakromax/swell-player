//! Prefetching: while a track plays, the streams of the next few tracks
//! (in the queue's own play order, so shuffle and repeat are respected)
//! are resolved, and their first seconds of audio downloaded into the
//! local proxy's memory (`proxy.rs`), so skipping to one starts at once.
//!
//! - At most [`PREFETCH_TRACKS`] tracks, one download at a time, nearest
//!   first; a download for a track that stopped being upcoming is
//!   cancelled, and what was cached for it dropped.
//! - Only while playing (and a moment after a track starts, not to compete
//!   with it), never on a metered network or without one.
//! - Signed URLs expire (`expiry.rs`): a stream past its freshness is
//!   resolved and downloaded again when still wanted.
//! - The bytes stay within the budget of `budget.rs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gtk::gio::prelude::*;
use gtk::{gio, glib};

use super::engine::Stream;
use super::failure;
use super::format::FormatDetails;
use super::proxy;
use crate::api::Api;
use crate::runtime;
use crate::tidal::Quality;

/// Upcoming tracks prefetched.
pub const PREFETCH_TRACKS: usize = 3;
/// Seconds of audio cached per upcoming track.
pub const PREFETCH_SECONDS: f64 = 8.0;
/// Prefetching waits until the current track has played this long.
pub const SETTLE_SECONDS: f64 = 1.5;
/// Routes of tracks no longer wanted are forgotten once unused this long
/// (a deck fading out still streams through its route).
const ROUTE_GRACE: Duration = Duration::from_secs(60);
/// A track whose prefetch failed is not tried again for this long.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// A prefetched track whose audio did not fit is tried again after this
/// (room frees up as playback moves on).
const FILL_AFTER: Duration = Duration::from_secs(5);

/// What to prefetch: the first `n` tracks of `upcoming` (in play order),
/// without the current one and without repeats.
pub fn targets(current: Option<u64>, upcoming: &[u64], n: usize) -> Vec<u64> {
    let mut targets: Vec<u64> = Vec::with_capacity(n);
    for &id in upcoming {
        if targets.len() == n {
            break;
        }
        if Some(id) != current && !targets.contains(&id) {
            targets.push(id);
        }
    }
    targets
}

/// About how many bytes a second of the stream takes (for the size of a
/// lossless file's prefix).
pub fn bytes_per_second(details: &FormatDetails) -> f64 {
    match (details.bit_depth, details.sample_rate, details.bitrate) {
        // FLAC is typically 50–70 % of the PCM rate; stereo.
        (Some(bits), Some(rate), _) if details.is_lossless() => {
            f64::from(bits) * f64::from(rate) * 2.0 / 8.0 * 0.7
        }
        (_, _, Some(kbps)) => f64::from(kbps) * 1000.0 / 8.0,
        _ => 120_000.0,
    }
}

type ReadyCallback = Box<dyn Fn(u64)>;

struct Slot {
    stream: Rc<Stream>,
    at: Instant,
    /// All of its first seconds fit in the budget.
    complete: bool,
    /// When its audio was last downloaded or tried.
    filled_at: Instant,
}

struct Job {
    track_id: u64,
    handle: glib::JoinHandle<()>,
}

/// What a job brings back: a newly resolved stream with its cached
/// bytes, or (`None`) more bytes for a stream already there.
type JobResult = anyhow::Result<(Option<Stream>, proxy::Prefetched)>;

/// Inputs of the last [`Prefetcher::update`], to go on when a job ends.
#[derive(Clone, Default)]
struct Wanted {
    current: Option<u64>,
    upcoming: Vec<u64>,
    quality: Option<Quality>,
    active: bool,
}

pub struct Prefetcher {
    api: Arc<Api>,
    weak_self: Weak<Self>,
    slots: RefCell<HashMap<u64, Slot>>,
    job: RefCell<Option<Job>>,
    failed: RefCell<HashMap<u64, Instant>>,
    wanted: RefCell<Wanted>,
    monitor: gio::NetworkMonitor,
    /// Told when a track's stream is ready.
    on_ready: RefCell<Option<ReadyCallback>>,
    /// Logged once per change.
    allowed: Cell<Option<bool>>,
}

impl Prefetcher {
    pub fn new(api: Arc<Api>) -> Rc<Self> {
        Rc::new_cyclic(|weak| Self {
            api,
            weak_self: weak.clone(),
            slots: RefCell::default(),
            job: RefCell::default(),
            failed: RefCell::default(),
            wanted: RefCell::default(),
            monitor: gio::NetworkMonitor::default(),
            on_ready: RefCell::default(),
            allowed: Cell::new(None),
        })
    }

    pub fn connect_ready(&self, callback: impl Fn(u64) + 'static) {
        self.on_ready.replace(Some(Box::new(callback)));
    }

    /// The stream prefetched for `track_id`, if it can still be played.
    pub fn ready(&self, track_id: u64) -> Option<(Rc<Stream>, Instant)> {
        let slots = self.slots.borrow();
        let slot = slots.get(&track_id)?;
        slot.stream
            .is_fresh(slot.at)
            .then(|| (slot.stream.clone(), slot.at))
    }

    /// A download for `track_id` is under way.
    pub fn pending(&self, track_id: u64) -> bool {
        self.job
            .borrow()
            .as_ref()
            .is_some_and(|job| job.track_id == track_id)
    }

    /// Whether `track_id` will be prefetched as soon as playback settles,
    /// so the caller need not resolve it itself.
    pub fn will_fetch(&self, track_id: u64) -> bool {
        proxy::running().is_some()
            && self.network_allows()
            && !self.failed.borrow().contains_key(&track_id)
            && targets(
                self.wanted.borrow().current,
                &self.wanted.borrow().upcoming,
                PREFETCH_TRACKS,
            )
            .contains(&track_id)
    }

    /// Forgets the stream prefetched for `track_id` (it failed to play):
    /// the next load resolves it afresh.
    pub fn forget(&self, track_id: u64) {
        let slot = self.slots.borrow_mut().remove(&track_id);
        let job = self
            .job
            .borrow_mut()
            .take_if(|job| job.track_id == track_id);
        if let Some(job) = job {
            job.handle.abort();
        }
        let stale: Vec<String> = slot
            .and_then(|slot| slot.stream.route.as_ref().map(|r| r.key.clone()))
            .into_iter()
            .collect();
        if let Some(proxy) = proxy::running() {
            proxy.collect(ROUTE_GRACE, &stale);
        }
        self.keep_manifests();
    }

    fn keep_manifests(&self) {
        super::engine::keep_prefetched_manifests(&self.track_ids());
    }

    /// Tracks with a prefetched stream (their DASH manifests must stay).
    pub fn track_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.slots.borrow().keys().copied().collect();
        ids.extend(self.job.borrow().as_ref().map(|job| job.track_id));
        ids
    }

    /// Forgets everything (quality changed, signed out).
    pub fn clear(&self) {
        if let Some(job) = self.job.take() {
            job.handle.abort();
        }
        let stale: Vec<String> = self
            .slots
            .take()
            .into_values()
            .filter_map(|slot| slot.stream.route.as_ref().map(|r| r.key.clone()))
            .collect();
        self.failed.borrow_mut().clear();
        if let Some(proxy) = proxy::running() {
            proxy.collect(ROUTE_GRACE, &stale);
        }
    }

    /// Prefetching is allowed by the network: connected, not metered.
    fn network_allows(&self) -> bool {
        let allowed = self.monitor.is_network_available() && !self.monitor.is_network_metered();
        if self.allowed.replace(Some(allowed)) != Some(allowed) {
            log::debug!(
                "prefetch {} (network available: {}, metered: {})",
                if allowed { "on" } else { "off" },
                self.monitor.is_network_available(),
                self.monitor.is_network_metered()
            );
        }
        allowed
    }

    /// Brings the prefetched set in line with the queue: `upcoming` in play
    /// order (more than [`PREFETCH_TRACKS`] is fine), `active` when
    /// downloading is wanted now (playing, settled).
    pub fn update(&self, current: Option<u64>, upcoming: Vec<u64>, quality: Quality, active: bool) {
        self.wanted.replace(Wanted {
            current,
            upcoming,
            quality: Some(quality),
            active,
        });
        self.refresh();
    }

    fn refresh(&self) {
        let wanted = self.wanted.borrow().clone();
        let targets = targets(wanted.current, &wanted.upcoming, PREFETCH_TRACKS);

        // Drop what is no longer upcoming (the current track's stays: it
        // may be playing from it), or expired.
        let mut stale = Vec::new();
        self.slots.borrow_mut().retain(|id, slot| {
            let keep = (targets.contains(id) || Some(*id) == wanted.current)
                && slot.stream.is_fresh(slot.at);
            if !keep && let Some(route) = &slot.stream.route {
                log::debug!("prefetch: dropping track {id}");
                stale.push(route.key.clone());
            }
            keep
        });
        let cancel = self
            .job
            .borrow()
            .as_ref()
            .is_some_and(|job| !targets.contains(&job.track_id));
        if cancel && let Some(job) = self.job.take() {
            log::debug!("prefetch: cancelled track {}", job.track_id);
            job.handle.abort();
        }
        if let Some(proxy) = proxy::running() {
            // The current track ranks last: it already started, its first
            // seconds only matter again for a restart.
            let mut ranks = targets.clone();
            ranks.extend(wanted.current);
            proxy.set_ranks(&ranks);
            proxy.collect(ROUTE_GRACE, &stale);
        }

        self.keep_manifests();
        let Some(quality) = wanted.quality else {
            return;
        };
        if !wanted.active
            || self.job.borrow().is_some()
            || proxy::running().is_none()
            || !self.network_allows()
        {
            return;
        }
        let now = Instant::now();
        self.failed
            .borrow_mut()
            .retain(|_, at| now.duration_since(*at) < RETRY_AFTER);
        let next = targets.iter().copied().find(|id| {
            !self.slots.borrow().contains_key(id) && !self.failed.borrow().contains_key(id)
        });
        if let Some(track_id) = next {
            return self.start(track_id, Some(quality));
        }
        // Resolved, but its audio did not fit then (or was evicted since):
        // try again.
        let Some(proxy) = proxy::running() else {
            return;
        };
        let unfilled = targets.iter().copied().find(|id| {
            self.slots.borrow().get(id).is_some_and(|slot| {
                slot.filled_at.elapsed() >= FILL_AFTER
                    && slot
                        .stream
                        .route
                        .as_ref()
                        .is_some_and(|route| !slot.complete || proxy.cached(&route.key) == 0)
            })
        });
        if let Some(track_id) = unfilled {
            self.start(track_id, None);
        }
    }

    /// Resolves `track_id` (with `quality`) and caches its first seconds,
    /// or (`None`) only caches them for the stream already resolved.
    fn start(&self, track_id: u64, quality: Option<Quality>) {
        let route = self
            .slots
            .borrow()
            .get(&track_id)
            .and_then(|slot| Some((slot.stream.route.clone()?, slot.stream.details.clone())));
        let api = self.api.clone();
        let weak = self.weak_self.clone();
        let started = Instant::now();
        let handle = glib::spawn_future_local(async move {
            let result = runtime::spawn(async move {
                let (stream, route, details) = match (quality, route) {
                    (Some(quality), _) => {
                        let stream = api.stream_with_quality(track_id, quality).await?;
                        let (route, details) = (stream.route.clone(), stream.details.clone());
                        (Some(stream), route, details)
                    }
                    (None, Some((route, details))) => (None, Some(route), details),
                    (None, None) => anyhow::bail!("nothing to fill"),
                };
                let mut cached = proxy::Prefetched::default();
                if let (Some(route), Some(proxy)) = (route, proxy::running()) {
                    let rate = bytes_per_second(&details);
                    match proxy.prefetch(&route.key, PREFETCH_SECONDS, rate).await {
                        Ok(prefetched) => cached = prefetched,
                        Err(err) => log::debug!(
                            "prefetch: no audio cached for track {track_id}: {}",
                            failure::redact(&format!("{err:#}"))
                        ),
                    }
                }
                anyhow::Ok((stream, cached))
            })
            .await;
            if let Some(this) = weak.upgrade() {
                this.finished(track_id, started, result);
            }
        });
        self.job.replace(Some(Job { track_id, handle }));
        self.keep_manifests();
    }

    fn finished(&self, track_id: u64, started: Instant, result: JobResult) {
        let ours = self
            .job
            .borrow()
            .as_ref()
            .is_some_and(|job| job.track_id == track_id);
        if !ours {
            return;
        }
        self.job.replace(None);
        match result {
            Ok((stream, cached)) => {
                let usage = proxy::running().map(|p| p.usage());
                log::debug!(
                    "prefetched track {track_id} ({}): {} KiB{} in {} ms; cache {:?} KiB of {:?}",
                    stream.as_ref().map_or("audio only", |s| s.format.as_str()),
                    cached.bytes >> 10,
                    if cached.complete { "" } else { " (partly)" },
                    started.elapsed().as_millis(),
                    usage.map(|(used, _)| used >> 10),
                    usage.map(|(_, budget)| budget >> 10),
                );
                let mut slots = self.slots.borrow_mut();
                match stream {
                    Some(stream) => {
                        slots.insert(
                            track_id,
                            Slot {
                                stream: Rc::new(stream),
                                at: started,
                                complete: cached.complete,
                                filled_at: Instant::now(),
                            },
                        );
                    }
                    None => {
                        if let Some(slot) = slots.get_mut(&track_id) {
                            slot.complete = cached.complete;
                            slot.filled_at = Instant::now();
                        }
                    }
                }
            }
            Err(err) => {
                log::debug!(
                    "prefetch of track {track_id} failed: {}",
                    failure::redact(&format!("{err:#}"))
                );
                self.failed.borrow_mut().insert(track_id, Instant::now());
            }
        }
        self.refresh();
        if self.slots.borrow().contains_key(&track_id)
            && let Some(callback) = self.on_ready.borrow().as_ref()
        {
            callback(track_id);
        }
    }
}

impl Drop for Prefetcher {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            job.handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::format::Codec;

    #[test]
    fn prefetches_the_next_few_in_play_order() {
        assert_eq!(targets(Some(1), &[2, 3, 4, 5], 3), [2, 3, 4]);
        // Repeat-one peeks the current track first: skipped.
        assert_eq!(targets(Some(1), &[1, 2, 3, 4], 3), [2, 3, 4]);
        // Repeat-all wrapping round: no duplicates.
        assert_eq!(targets(Some(3), &[4, 1, 2, 3, 4], 5), [4, 1, 2]);
        assert_eq!(targets(None, &[], 3), Vec::<u64>::new());
        assert_eq!(targets(Some(1), &[2, 3], 0), Vec::<u64>::new());
    }

    fn details(
        codec: Codec,
        bits: Option<u32>,
        rate: Option<u32>,
        kbps: Option<u32>,
    ) -> FormatDetails {
        FormatDetails {
            codec,
            bit_depth: bits,
            sample_rate: rate,
            bitrate: kbps,
            quality: None,
            audio_mode: None,
            replay_gain_db: None,
        }
    }

    #[test]
    fn estimates_stream_rates() {
        let cd = bytes_per_second(&details(Codec::Flac, Some(16), Some(44_100), None));
        assert!((cd - 123_480.0).abs() < 1.0);
        let hires = bytes_per_second(&details(Codec::Flac, Some(24), Some(192_000), None));
        assert!(hires > 800_000.0);
        let aac = bytes_per_second(&details(Codec::Aac, None, Some(44_100), Some(320)));
        assert_eq!(aac, 40_000.0);
        let unknown = bytes_per_second(&details(Codec::Unknown, None, None, None));
        assert_eq!(unknown, 120_000.0);
    }
}
