//! Album covers: a small download pool backed by a disk cache and an
//! in-memory LRU of decoded textures.
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
use std::num::NonZeroUsize;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use gtk::{gdk, glib};
use lru::LruCache;
use tokio::sync::{Notify, oneshot};

use crate::runtime;

/// Edge of the square JPEG requested from TIDAL's image server.
pub const COVER_SIZE: u32 = 80;
const WORKERS: usize = 4;
/// Decoded 80×80 covers take ~25 KB each; 400 of them stay under 10 MB.
const MEMORY_ENTRIES: usize = 400;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);

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
    id: String,
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

type Waiter = (Rc<Cell<bool>>, Box<dyn FnOnce(&gdk::Texture)>);

struct Pending {
    request: Arc<Request>,
    waiters: Vec<Waiter>,
}

pub struct CoverLoader {
    queue: Arc<Queue>,
    memory: RefCell<LruCache<String, gdk::Texture>>,
    pending: RefCell<HashMap<String, Pending>>,
    missing: RefCell<HashSet<String>>,
}

impl CoverLoader {
    pub fn new(http: reqwest::Client) -> Rc<Self> {
        let queue = Arc::new(Queue::default());
        let dir: Arc<Path> = glib::user_cache_dir()
            .join("swell-player")
            .join("covers")
            .join(COVER_SIZE.to_string())
            .into();
        for _ in 0..WORKERS {
            runtime::runtime().spawn(worker(queue.clone(), http.clone(), dir.clone()));
        }
        Rc::new(Self {
            queue,
            memory: RefCell::new(LruCache::new(NonZeroUsize::new(MEMORY_ENTRIES).unwrap())),
            pending: RefCell::default(),
            missing: RefCell::default(),
        })
    }

    /// Tells the pool which list position is in the middle of the viewport.
    pub fn set_focus(&self, position: u32) {
        self.queue.focus.store(position, Ordering::Relaxed);
    }

    /// Calls `done` on the main thread once the cover `id` is available
    /// (immediately if it is in memory). `position` is the requesting row's
    /// place in its list. Returns a ticket while the request is pending;
    /// drop it to cancel.
    pub fn load(
        self: &Rc<Self>,
        id: &str,
        position: u32,
        done: impl FnOnce(&gdk::Texture) + 'static,
    ) -> Option<Ticket> {
        if let Some(texture) = self.memory.borrow_mut().get(id) {
            done(texture);
            return None;
        }
        if self.missing.borrow().contains(id) {
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
            id: id.to_owned(),
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
            match outcome {
                Ok(Outcome::Ready(texture)) => {
                    for (cancelled, done) in pending.into_iter().flat_map(|p| p.waiters) {
                        if !cancelled.get() {
                            done(&texture);
                        }
                    }
                    this.memory.borrow_mut().put(id, texture);
                }
                Ok(Outcome::Missing) => {
                    this.missing.borrow_mut().insert(id);
                }
                // Failed, or skipped because nobody wanted it any more.
                Ok(Outcome::Failed) | Err(_) => {}
            }
        });
        Some(Ticket { request, cancelled })
    }
}

async fn worker(queue: Arc<Queue>, http: reqwest::Client, dir: Arc<Path>) {
    loop {
        let job = queue.next().await;
        let outcome = fetch(&http, &dir, &job.id).await;
        let _ = job.reply.send(outcome);
    }
}

async fn fetch(http: &reqwest::Client, dir: &Path, id: &str) -> Outcome {
    let path = dir.join(format!("{id}.jpg"));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        match decode(bytes) {
            Ok((texture, _)) => return Outcome::Ready(texture),
            Err(err) => {
                log::debug!("dropping unreadable cached cover {id}: {err:#}");
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }

    let bytes = match download(http, id).await {
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
            if let Err(err) = store(dir, &path, &bytes).await {
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

async fn download(http: &reqwest::Client, id: &str) -> reqwest::Result<Vec<u8>> {
    let url = format!(
        "https://resources.tidal.com/images/{}/{COVER_SIZE}x{COVER_SIZE}.jpg",
        id.replace('-', "/")
    );
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
async fn store(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    tokio::fs::create_dir_all(dir).await?;
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
