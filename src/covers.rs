//! Album covers: a small download pool backed by a disk cache and an
//! in-memory LRU of decoded textures.
//!
//! - At most [`WORKERS`] downloads run at once, all over the shared HTTP
//!   client: TIDAL's image server speaks HTTP/2, so they are multiplexed on
//!   one kept-alive connection instead of a TLS handshake per image.
//! - The pending request closest to the middle of the viewport goes first,
//!   newest first among equals. Plain LIFO is not enough: `gtk::ListView`
//!   binds ~200 rows up front, so the last request is far off screen.
//! - A request whose row is recycled before a worker picks it up is skipped.

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gtk::{gdk, glib};
use lru::LruCache;
use tokio::sync::{Notify, oneshot};

use crate::runtime;

/// Edge of the square JPEG requested from TIDAL's image server.
pub const COVER_SIZE: u32 = 80;
const WORKERS: usize = 4;
/// Decoded 80×80 covers take ~25 KB each; 400 of them stay under 10 MB.
const MEMORY_ENTRIES: usize = 400;

struct Job {
    id: String,
    /// Position of the requesting row in its list.
    position: u32,
    cancelled: Arc<AtomicBool>,
    reply: oneshot::Sender<gdk::Texture>,
}

#[derive(Default)]
struct Queue {
    stack: Mutex<Vec<Job>>,
    ready: Notify,
    /// List position at the middle of the viewport.
    focus: AtomicU32,
}

impl Queue {
    fn push(&self, job: Job) {
        self.stack.lock().unwrap().push(job);
        self.ready.notify_one();
    }

    /// The wanted request nearest the viewport; the newest among equals.
    fn pop(&self) -> Option<Job> {
        let focus = self.focus.load(Ordering::Relaxed);
        let mut stack = self.stack.lock().unwrap();
        stack.retain(|job| !job.cancelled.load(Ordering::Relaxed));
        let nearest = stack
            .iter()
            .enumerate()
            .rev()
            .min_by_key(|(_, job)| job.position.abs_diff(focus))
            .map(|(index, _)| index)?;
        Some(stack.remove(nearest))
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

/// Keeps a cover request alive; dropping it cancels the request.
pub struct Ticket(Arc<AtomicBool>);

impl Drop for Ticket {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub struct CoverLoader {
    queue: Arc<Queue>,
    memory: RefCell<LruCache<String, gdk::Texture>>,
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

        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply, receiver) = oneshot::channel();
        self.queue.push(Job {
            id: id.to_owned(),
            position,
            cancelled: cancelled.clone(),
            reply,
        });

        let this = self.clone();
        let id = id.to_owned();
        let flag = cancelled.clone();
        glib::spawn_future_local(async move {
            if let Ok(texture) = receiver.await {
                if !flag.load(Ordering::Relaxed) {
                    done(&texture);
                }
                this.memory.borrow_mut().put(id, texture);
            }
        });
        Some(Ticket(cancelled))
    }
}

async fn worker(queue: Arc<Queue>, http: reqwest::Client, dir: Arc<Path>) {
    loop {
        let job = queue.next().await;
        match fetch(&http, &dir, &job.id).await {
            Ok(texture) => {
                let _ = job.reply.send(texture);
            }
            Err(err) => log::debug!("cover {}: {err:#}", job.id),
        }
    }
}

async fn fetch(http: &reqwest::Client, dir: &Path, id: &str) -> Result<gdk::Texture> {
    let path = dir.join(format!("{id}.jpg"));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = download(http, id).await?;
            if let Err(err) = store(dir, &path, &bytes).await {
                log::debug!("could not cache cover {id}: {err:#}");
            }
            bytes
        }
    };
    Ok(gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))?)
}

async fn download(http: &reqwest::Client, id: &str) -> Result<Vec<u8>> {
    let url = format!(
        "https://resources.tidal.com/images/{}/{COVER_SIZE}x{COVER_SIZE}.jpg",
        id.replace('-', "/")
    );
    let response = http.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

/// Writes through a temporary file so a crash never leaves a truncated image.
async fn store(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let partial: PathBuf = path.with_extension("part");
    tokio::fs::write(&partial, bytes).await?;
    tokio::fs::rename(&partial, path).await?;
    Ok(())
}
