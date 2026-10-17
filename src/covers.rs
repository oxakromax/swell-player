//! Artwork (album covers, artist pictures, playlist and mix images): a small
//! download pool backed by a disk cache and an in-memory LRU of decoded
//! textures with a byte budget.
//!
//! - At most [`WORKERS`] downloads run at once, all over the shared HTTP
//!   client: TIDAL's image server speaks HTTP/2, so they are multiplexed on
//!   one kept-alive connection instead of a TLS handshake per image.
//! - The pending request closest to the middle of the viewport goes first,
//!   newest first among equals. Plain LIFO is not enough: `gtk::ListView`
//!   binds ~200 rows up front, so the last request is far off screen.
//! - Rows asking for the same cover (tracks of one album) share one request.
//! - A request whose rows are all recycled before a worker picks it up is
//!   skipped.
//!
//! The loader lives as long as the application (there is one window), so its
//! workers never need to stop.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use gtk::prelude::*;
use gtk::{gdk, glib};
use lru::LruCache;
use tokio::sync::{Notify, oneshot};

use crate::runtime;

const WORKERS: usize = 4;
/// Decoded textures kept in memory (RGBA: an 80 px cover is 25 KB, a 640 px
/// one 1.6 MB).
const MEMORY_BUDGET: usize = 64 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// A piece of artwork. TIDAL serves each kind in a fixed set of sizes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Image {
    /// Album cover image id.
    Cover(String),
    /// Artist picture image id.
    Artist(String),
    /// Playlist (square) image id.
    Playlist(String),
    /// A full URL (mix artwork).
    Url(String),
}

impl Image {
    fn sizes(&self) -> &'static [u32] {
        match self {
            Self::Cover(_) => &[80, 160, 320, 640, 1280],
            Self::Artist(_) => &[160, 320, 480, 750],
            Self::Playlist(_) => &[160, 320, 480, 640, 750, 1080],
            Self::Url(_) => &[0],
        }
    }

    /// Whether this names an image TIDAL serves. Ids become file names in
    /// the disk cache and URLs are fetched, so anything unexpected from the
    /// API (a `../` in an id, a foreign host) is refused.
    fn is_valid(&self) -> bool {
        match self {
            Self::Cover(id) | Self::Artist(id) | Self::Playlist(id) => is_image_id(id),
            Self::Url(url) => is_tidal_image_url(url),
        }
    }

    /// The smallest served size at least `pixels` wide.
    fn size_for(&self, pixels: u32) -> u32 {
        let sizes = self.sizes();
        sizes
            .iter()
            .copied()
            .find(|&size| size >= pixels)
            .unwrap_or(sizes[sizes.len() - 1])
    }

    fn url(&self, size: u32) -> String {
        let path = |id: &str| {
            format!(
                "https://resources.tidal.com/images/{}/{size}x{size}.jpg",
                id.replace('-', "/")
            )
        };
        match self {
            Self::Cover(id) | Self::Artist(id) | Self::Playlist(id) => path(id),
            Self::Url(url) => url.clone(),
        }
    }

    /// File name in the disk cache.
    fn file_name(&self, size: u32) -> String {
        use sha2::Digest;
        match self {
            Self::Cover(id) => format!("{size}/{id}.jpg"),
            Self::Artist(id) => format!("artist-{size}/{id}.jpg"),
            Self::Playlist(id) => format!("playlist-{size}/{id}.jpg"),
            Self::Url(url) => {
                // Mix image URLs carry a rotating token; key on the path.
                let stable = url.split('?').next().unwrap_or(url);
                let digest = sha2::Sha256::digest(stable.as_bytes());
                let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
                format!("url/{hex}.jpg")
            }
        }
    }
}

/// `^[0-9a-fA-F-]{8,64}$`: TIDAL's image ids are UUIDs.
fn is_image_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// An https URL on one of TIDAL's hosts (`images.tidal.com` for mixes,
/// `resources.tidal.com`...).
fn is_tidal_image_url(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.port().is_none()
        && ["tidal.com", "tidalhifi.com"]
            .iter()
            .any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|sub| sub.ends_with('.')))
}

/// Bumped when the disk cache is deleted: downloads that started before
/// are not written into the new one.
static DISK_EPOCH: AtomicU64 = AtomicU64::new(0);

fn disk_dir() -> PathBuf {
    glib::user_cache_dir().join("swell-player").join("covers")
}

/// Marks a cached file as recently used (its modification time), at most
/// once a day per file, so [`prune_disk_cache`] drops the least used first.
fn touch(path: &Path) {
    const DAY: Duration = Duration::from_secs(24 * 60 * 60);
    let Ok(file) = std::fs::File::options().append(true).open(path) else {
        return;
    };
    let stale = file
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > DAY);
    if stale {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

/// Keeps the artwork cache under `max_bytes` (least recently used files go
/// first) and drops files unused for `max_age`. Blocking: run it on the
/// blocking pool.
pub fn prune_disk_cache(max_bytes: u64, max_age: Duration) -> std::io::Result<()> {
    let mut files = Vec::new();
    let mut stack = vec![disk_dir()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                files.push((modified, meta.len(), entry.path()));
            }
        }
    }
    files.sort_by_key(|(modified, ..)| std::cmp::Reverse(*modified));
    let now = std::time::SystemTime::now();
    let mut kept = 0u64;
    let mut removed = 0usize;
    for (modified, len, path) in files {
        let old = now.duration_since(modified).unwrap_or_default() > max_age;
        if old || kept + len > max_bytes {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        } else {
            kept += len;
        }
    }
    log::debug!("artwork cache: {kept} bytes kept, {removed} files removed");
    Ok(())
}

/// Deletes the downloaded artwork (on sign-out). Downloads in flight are
/// not stored. Call from the tokio runtime.
pub async fn forget_disk_cache() -> Result<()> {
    DISK_EPOCH.fetch_add(1, Ordering::SeqCst);
    let dir = disk_dir();
    tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    })
    .await?
}

/// One cover being fetched, shared by the queue and the rows waiting for it.
struct Request {
    /// Rows still waiting; at zero the job is skipped.
    waiters: AtomicUsize,
    /// List position of the latest row that asked for it.
    position: AtomicU32,
}

impl Request {
    fn wanted(&self) -> bool {
        self.waiters.load(Ordering::Relaxed) > 0
    }
}

enum Outcome {
    Ready(gdk::Texture),
    /// The server has no usable image; do not ask again this session.
    Missing,
    /// A network problem; a later request may succeed.
    Failed,
}

struct Job {
    url: String,
    file: String,
    request: Arc<Request>,
    reply: oneshot::Sender<Outcome>,
}

#[derive(Default)]
struct Queue {
    jobs: Mutex<Vec<Job>>,
    ready: Notify,
    /// List position at the middle of the viewport.
    focus: AtomicU32,
}

impl Queue {
    fn push(&self, job: Job) {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(job);
        self.ready.notify_one();
    }

    /// The wanted request nearest the viewport; the newest among equals.
    fn pop(&self) -> Option<Job> {
        let focus = self.focus.load(Ordering::Relaxed);
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        jobs.retain(|job| job.request.wanted());
        let nearest = jobs
            .iter()
            .enumerate()
            .rev()
            .min_by_key(|(_, job)| job.request.position.load(Ordering::Relaxed).abs_diff(focus))
            .map(|(index, _)| index)?;
        Some(jobs.remove(nearest))
    }

    async fn next(&self) -> Job {
        loop {
            if let Some(job) = self.pop() {
                return job;
            }
            self.ready.notified().await;
        }
    }
}

/// Keeps a row's interest in a cover; dropping it withdraws that interest.
pub struct Ticket {
    request: Arc<Request>,
    cancelled: Rc<Cell<bool>>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancelled.set(true);
        self.request.waiters.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Why an image did not come (see [`CoverLoader::load_or_report`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The server has none: asking again this session is pointless.
    Missing,
    /// A network problem: a later request may succeed.
    Failed,
}

type Done = Box<dyn FnOnce(Result<&gdk::Texture, Unavailable>)>;
type Waiter = (Rc<Cell<bool>>, Done);

struct Pending {
    request: Arc<Request>,
    waiters: Vec<Waiter>,
}

pub struct CoverLoader {
    queue: Arc<Queue>,
    memory: RefCell<Memory>,
    pending: RefCell<HashMap<String, Pending>>,
    missing: RefCell<HashSet<String>>,
}

impl CoverLoader {
    pub fn new(http: reqwest::Client) -> Rc<Self> {
        let queue = Arc::new(Queue::default());
        let dir: Arc<Path> = disk_dir().into();
        for _ in 0..WORKERS {
            runtime::runtime().spawn(worker(queue.clone(), http.clone(), dir.clone()));
        }
        Rc::new(Self {
            queue,
            memory: RefCell::new(Memory {
                textures: LruCache::unbounded(),
                bytes: 0,
            }),
            pending: RefCell::default(),
            missing: RefCell::default(),
        })
    }

    /// Tells the pool which list position is in the middle of the viewport.
    /// Drops every decoded texture and remembered miss (on sign-out).
    pub fn clear_memory(&self) {
        let mut memory = self.memory.borrow_mut();
        memory.textures.clear();
        memory.bytes = 0;
        self.missing.borrow_mut().clear();
    }

    pub fn set_focus(&self, position: u32) {
        self.queue.focus.store(position, Ordering::Relaxed);
    }

    /// Calls `done` on the main thread once `image` is available at least
    /// `pixels` wide (immediately if it is in memory). Pass the widget size
    /// times its scale factor. `position` is the requesting row's place in
    /// its list (rows nearest the viewport centre load first; use 0 for
    /// anything not in a list). Returns a ticket while the request is
    /// pending; drop it to cancel.
    pub fn load(
        self: &Rc<Self>,
        image: &Image,
        pixels: u32,
        position: u32,
        done: impl FnOnce(&gdk::Texture) + 'static,
    ) -> Option<Ticket> {
        self.load_or_report(image, pixels, position, move |result| {
            if let Ok(texture) = result {
                done(texture);
            }
        })
    }

    /// Like [`load`](Self::load), but `done` also hears why an image did
    /// not come (unless the ticket was dropped first).
    pub fn load_or_report(
        self: &Rc<Self>,
        image: &Image,
        pixels: u32,
        position: u32,
        done: impl FnOnce(Result<&gdk::Texture, Unavailable>) + 'static,
    ) -> Option<Ticket> {
        if !image.is_valid() {
            log::debug!("ignoring an unexpected image reference: {image:?}");
            done(Err(Unavailable::Missing));
            return None;
        }
        let size = image.size_for(pixels);
        let file = image.file_name(size);
        let id = file.as_str();
        if let Some(texture) = self.memory.borrow_mut().textures.get(id) {
            done(Ok(texture));
            return None;
        }
        if self.missing.borrow().contains(id) {
            done(Err(Unavailable::Missing));
            return None;
        }

        let cancelled = Rc::new(Cell::new(false));
        let waiter: Waiter = (cancelled.clone(), Box::new(done));

        if let Some(pending) = self.pending.borrow_mut().get_mut(id)
            && pending.request.wanted()
        {
            pending.request.waiters.fetch_add(1, Ordering::Relaxed);
            pending.request.position.store(position, Ordering::Relaxed);
            pending.waiters.push(waiter);
            return Some(Ticket {
                request: pending.request.clone(),
                cancelled,
            });
        }

        let request = Arc::new(Request {
            waiters: AtomicUsize::new(1),
            position: AtomicU32::new(position),
        });
        self.pending.borrow_mut().insert(
            id.to_owned(),
            Pending {
                request: request.clone(),
                waiters: vec![waiter],
            },
        );
        let (reply, outcome) = oneshot::channel();
        self.queue.push(Job {
            url: image.url(size),
            file: file.clone(),
            request: request.clone(),
            reply,
        });

        let this = self.clone();
        let id = id.to_owned();
        let job_request = request.clone();
        glib::spawn_future_local(async move {
            let outcome = outcome.await;
            // A newer request may have replaced this one; leave it alone.
            let pending = {
                let mut pending = this.pending.borrow_mut();
                match pending.get(&id) {
                    Some(entry) if Arc::ptr_eq(&entry.request, &job_request) => pending.remove(&id),
                    _ => None,
                }
            };
            let waiters = pending.into_iter().flat_map(|p| p.waiters);
            let waiters = waiters.filter(|(cancelled, _)| !cancelled.get());
            match outcome {
                Ok(Outcome::Ready(texture)) => {
                    for (_, done) in waiters {
                        done(Ok(&texture));
                    }
                    this.memory.borrow_mut().put(id, texture);
                }
                Ok(Outcome::Missing) => {
                    this.missing.borrow_mut().insert(id);
                    for (_, done) in waiters {
                        done(Err(Unavailable::Missing));
                    }
                }
                // Failed, or skipped because nobody wanted it any more.
                Ok(Outcome::Failed) | Err(_) => {
                    for (_, done) in waiters {
                        done(Err(Unavailable::Failed));
                    }
                }
            }
        });
        Some(Ticket { request, cancelled })
    }
}

/// Decoded textures, least recently used dropped first past the budget.
struct Memory {
    textures: LruCache<String, gdk::Texture>,
    bytes: usize,
}

impl Memory {
    fn cost(texture: &gdk::Texture) -> usize {
        (texture.width().max(1) as usize) * (texture.height().max(1) as usize) * 4
    }

    fn put(&mut self, key: String, texture: gdk::Texture) {
        self.bytes += Self::cost(&texture);
        if let Some(old) = self.textures.put(key, texture) {
            self.bytes -= Self::cost(&old);
        }
        while self.bytes > MEMORY_BUDGET && self.textures.len() > 1 {
            match self.textures.pop_lru() {
                Some((_, old)) => self.bytes -= Self::cost(&old),
                None => break,
            }
        }
    }
}

async fn worker(queue: Arc<Queue>, http: reqwest::Client, dir: Arc<Path>) {
    loop {
        let job = queue.next().await;
        let outcome = fetch(&http, &dir, &job.url, &job.file).await;
        let _ = job.reply.send(outcome);
    }
}

async fn fetch(http: &reqwest::Client, dir: &Path, url: &str, file: &str) -> Outcome {
    let id = file;
    let path = dir.join(file);
    let epoch = DISK_EPOCH.load(Ordering::SeqCst);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        match decode(bytes) {
            Ok((texture, _)) => {
                touch(&path);
                return Outcome::Ready(texture);
            }
            Err(err) => {
                log::debug!("dropping unreadable cached cover {id}: {err:#}");
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }

    let bytes = match download(http, url).await {
        Ok(bytes) => bytes,
        Err(err) => {
            log::debug!("cover {id}: {err}");
            return match err.status() {
                Some(status) if status.is_client_error() => Outcome::Missing,
                _ => Outcome::Failed,
            };
        }
    };
    // Only images that decode are cached.
    match decode(bytes) {
        Ok((texture, bytes)) => {
            if DISK_EPOCH.load(Ordering::SeqCst) != epoch {
                return Outcome::Ready(texture);
            }
            if let Err(err) = store(&path, &bytes).await {
                log::debug!("could not cache cover {id}: {err:#}");
            }
            Outcome::Ready(texture)
        }
        Err(err) => {
            log::debug!("cover {id} does not decode: {err:#}");
            Outcome::Missing
        }
    }
}

fn decode(bytes: Vec<u8>) -> Result<(gdk::Texture, glib::Bytes)> {
    let bytes = glib::Bytes::from_owned(bytes);
    let texture = gdk::Texture::from_bytes(&bytes).context("not a readable image")?;
    Ok((texture, bytes))
}

async fn download(http: &reqwest::Client, url: &str) -> reqwest::Result<Vec<u8>> {
    let response = http
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

/// Writes through a uniquely named temporary file, so a crash or a parallel
/// write never leaves a truncated image under the final name.
async fn store(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let partial = path.with_extension(format!(
        "{}.{}.part",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    tokio::fs::write(&partial, bytes).await?;
    if let Err(err) = tokio::fs::rename(&partial, path).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(err.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tidal_images_are_loaded() {
        assert!(Image::Cover("a1b2c3d4-e5f6-4789-abcd-ef0123456789".into()).is_valid());
        assert!(Image::Artist("ABCDEF12".into()).is_valid());
        for id in ["", "../../etc", "abc", "a1b2c3d4/../x", "a1b2c3d4-e5f6.jpg", &"a".repeat(65)] {
            assert!(!Image::Playlist(id.into()).is_valid(), "{id}");
        }
        for url in [
            "https://images.tidal.com/im/im?w=640&h=640&uuid=x",
            "https://resources.tidal.com/images/ab/cd/550x400.jpg",
            "https://tidal.com/x.jpg",
            "https://images.tidalhifi.com/x.jpg",
        ] {
            assert!(Image::Url(url.into()).is_valid(), "{url}");
        }
        for url in [
            "http://images.tidal.com/x.jpg",
            "https://eviltidal.com/x.jpg",
            "https://tidal.com.example.org/x.jpg",
            "https://images.tidal.com:8443/x.jpg",
            "file:///etc/passwd",
            "https://example.com/?h=images.tidal.com",
            "not a url",
        ] {
            assert!(!Image::Url(url.into()).is_valid(), "{url}");
        }
    }
}
