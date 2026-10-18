//! A local HTTP proxy that plays the prefetched start of upcoming tracks
//! from memory.
//!
//! Every stream Swell plays goes through it: `playbin3` (`souphttpsrc`) and
//! `dashdemux2` fetch `http://127.0.0.1:<port>/…`, and the proxy answers
//! from what `prefetch.rs` downloaded ahead, continuing from TIDAL with a
//! `Range` request where the cached bytes end:
//! - lossless files (one signed URL) are routed as `/p/<key>`: requests
//!   inside the cached prefix are answered from memory at once while the
//!   rest is requested from the CDN in parallel; seeks past it are passed
//!   through with their `Range`;
//! - Hi-Res DASH manifests are rewritten so the initialisation and media
//!   segments are fetched as `/d/<key>/i/<name>` and `/d/<key>/m/<name>`:
//!   the init segment and the first media segments come from memory, the
//!   others are passed through.
//!
//! Why a proxy and not an `appsrc`: it serves both kinds of stream the same
//! way while `souphttpsrc` and `dashdemux2` keep doing what they do well
//! (buffering, seeking with ranges, reconnecting, reporting HTTP errors
//! with their status, which `failure.rs` classifies as before); an `appsrc`
//! could not feed `dashdemux2`'s own segment downloads at all. The proxy
//! also keeps one warm HTTP/2 connection to the CDN for every track, so
//! even what is not cached starts without a new TLS handshake.
//!
//! Only 127.0.0.1 is listened on, routes are unguessable random keys, and
//! signed URLs never leave the process (logs redact them). The cached
//! bytes live in memory within the budget of `budget.rs`. If the proxy
//! cannot start (or `SWELL_NO_PROXY=1`), streams play straight from TIDAL
//! as before.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::budget::{self, Item};
use super::failure;

/// Biggest request head accepted.
const MAX_HEAD: usize = 16 << 10;
/// A client that sends nothing for this long is dropped.
const HEAD_TIMEOUT: Duration = Duration::from_secs(15);
/// Media segments prefetched per DASH track at most.
const MAX_SEGMENTS: usize = 4;
/// Bytes prefetched per track at most.
pub const MAX_ENTRY: usize = 16 << 20;

/// How a stream is played through the proxy.
#[derive(Clone, Debug)]
pub struct Route {
    pub key: String,
    /// What to give `playbin3` (lossless files; DASH plays its rewritten
    /// manifest file instead).
    pub uri: String,
    pub dash: bool,
}

/// The proxy, started on first use; `None` when it cannot run.
pub fn get() -> Option<Arc<Proxy>> {
    static PROXY: OnceLock<Option<Arc<Proxy>>> = OnceLock::new();
    PROXY
        .get_or_init(|| {
            if std::env::var_os("SWELL_NO_PROXY").is_some_and(|v| v != "0") {
                log::info!("SWELL_NO_PROXY: streaming straight from TIDAL");
                return None;
            }
            match Proxy::start() {
                Ok(proxy) => Some(proxy),
                Err(err) => {
                    log::warn!("the local stream proxy is unavailable: {err:#}");
                    None
                }
            }
        })
        .clone()
}

/// The proxy, only if it already runs (never starts it).
pub fn running() -> Option<Arc<Proxy>> {
    RUNNING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

static RUNNING: Mutex<Option<Arc<Proxy>>> = Mutex::new(None);

pub struct Proxy {
    port: u16,
    store: Mutex<Store>,
    http: reqwest::Client,
}

#[derive(Debug)]
enum Upstream {
    Progressive {
        url: String,
    },
    Dash {
        init: String,
        /// The media URL up to the file name, and its query.
        media_dir: String,
        media_query: String,
        info: DashInfo,
    },
}

#[derive(Debug, Default)]
enum Cached {
    #[default]
    Nothing,
    Prefix {
        bytes: Bytes,
        /// Size of the whole file.
        total: u64,
        content_type: Option<String>,
    },
    /// DASH files by path (`i/<name>`, `m/<name>`).
    Files(HashMap<String, Bytes>),
}

impl Cached {
    fn len(&self) -> usize {
        match self {
            Cached::Nothing => 0,
            Cached::Prefix { bytes, .. } => bytes.len(),
            Cached::Files(files) => files.values().map(Bytes::len).sum(),
        }
    }
}

#[derive(Debug)]
struct Entry {
    track_id: u64,
    upstream: Upstream,
    cached: Cached,
    /// Requests being served now.
    active: usize,
    last_used: u64,
    touched: Instant,
}

#[derive(Default)]
struct Store {
    routes: HashMap<String, Entry>,
    /// Wanted tracks by rank (0: playing now), from the prefetcher.
    ranks: HashMap<u64, usize>,
    budget: usize,
    /// Bytes reserved for downloads in flight.
    reserved: usize,
    clock: u64,
}

impl Store {
    fn used(&self) -> usize {
        self.cached() + self.reserved
    }

    fn cached(&self) -> usize {
        self.routes.values().map(|e| e.cached.len()).sum()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn items(&self, except: &str) -> Vec<Item<String>> {
        self.routes
            .iter()
            .filter(|(key, _)| key.as_str() != except)
            .map(|(key, entry)| Item {
                key: key.clone(),
                bytes: entry.cached.len(),
                rank: self.ranks.get(&entry.track_id).copied(),
                last_used: entry.last_used,
            })
            .collect()
    }

    /// Makes room for `bytes` more for the route `key`, evicting less
    /// wanted entries' bytes. False when it does not fit.
    fn reserve(&mut self, key: &str, bytes: usize) -> bool {
        let Some(entry) = self.routes.get(key) else {
            return false;
        };
        let rank = self
            .ranks
            .get(&entry.track_id)
            .copied()
            .unwrap_or(usize::MAX);
        let items = self.items(key);
        let Some(evicted) = budget::evictions(&items, self.used(), self.budget, bytes, rank) else {
            return false;
        };
        for key in evicted {
            if let Some(entry) = self.routes.get_mut(&key) {
                log::debug!(
                    "prefetch: dropping {} KiB of track {} for room",
                    entry.cached.len() >> 10,
                    entry.track_id
                );
                entry.cached = Cached::Nothing;
            }
        }
        self.reserved += bytes;
        true
    }

    fn release(&mut self, bytes: usize) {
        self.reserved = self.reserved.saturating_sub(bytes);
    }

    /// Lowers the budget, evicting what no longer fits (least wanted,
    /// then least recently used, first).
    fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        while self.used() > self.budget {
            let worst = self
                .items("")
                .into_iter()
                .filter(|item| item.bytes > 0)
                .max_by_key(|item| {
                    (
                        item.rank.unwrap_or(usize::MAX),
                        std::cmp::Reverse(item.last_used),
                    )
                });
            let Some(worst) = worst else { break };
            if let Some(entry) = self.routes.get_mut(&worst.key) {
                entry.cached = Cached::Nothing;
            }
        }
    }
}

/// Decrements an entry's active count when a request ends.
struct Serving<'a> {
    proxy: &'a Proxy,
    key: String,
}

impl Drop for Serving<'_> {
    fn drop(&mut self) {
        let mut store = self.proxy.lock();
        let tick = store.tick();
        if let Some(entry) = store.routes.get_mut(&self.key) {
            entry.active = entry.active.saturating_sub(1);
            entry.last_used = tick;
            entry.touched = Instant::now();
        }
    }
}

/// A random, unguessable route key.
fn new_key() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    let mut key = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

/// The HTTP client for the CDN: like the API's, but without an overall
/// deadline (a body streams for a whole track); reads that stall fail.
fn cdn_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .http2_keep_alive_interval(Duration::from_secs(20))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        .build()?)
}

impl Proxy {
    fn start() -> Result<Arc<Self>> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .context("could not listen on 127.0.0.1")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let store = Store {
            budget: budget::budget(budget::available_memory()),
            ..Store::default()
        };
        let proxy = Arc::new(Self {
            port,
            store: Mutex::new(store),
            http: cdn_client()?,
        });
        let weak = Arc::downgrade(&proxy);
        crate::runtime::runtime().spawn(async move {
            let listener = match TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(err) => return log::warn!("the stream proxy failed: {err}"),
            };
            loop {
                let (socket, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        log::debug!("stream proxy: accept failed: {err}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Some(proxy) = weak.upgrade() else { break };
                tokio::spawn(async move {
                    if let Err(err) = proxy.serve(socket).await {
                        log::debug!("stream proxy: {}", failure::redact(&format!("{err:#}")));
                    }
                });
            }
        });
        log::info!(
            "stream proxy on 127.0.0.1:{port}, prefetch budget {} MiB",
            proxy.lock().budget >> 20
        );
        RUNNING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(proxy.clone());
        Ok(proxy)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn insert(&self, track_id: u64, upstream: Upstream) -> String {
        let key = new_key();
        let mut store = self.lock();
        let tick = store.tick();
        store.routes.insert(
            key.clone(),
            Entry {
                track_id,
                upstream,
                cached: Cached::Nothing,
                active: 0,
                last_used: tick,
                touched: Instant::now(),
            },
        );
        key
    }

    /// Routes a lossless file (one signed URL). No network.
    pub fn route_progressive(&self, track_id: u64, url: &str) -> Route {
        let key = self.insert(
            track_id,
            Upstream::Progressive {
                url: url.to_owned(),
            },
        );
        Route {
            uri: format!("{}/p/{key}", self.base()),
            key,
            dash: false,
        }
    }

    /// Routes a DASH manifest: returns it rewritten to fetch its segments
    /// through the proxy. Fails (the caller plays the original) for
    /// manifests of a shape it does not know.
    pub fn route_dash(&self, track_id: u64, mpd: &str) -> Result<(String, Route)> {
        let parsed = parse_mpd(mpd)?;
        let key = new_key();
        let base = format!("{}/d/{key}", self.base());
        let rewritten = parsed.rewrite(mpd, &base);
        let upstream = Upstream::Dash {
            init: parsed.init_url.clone(),
            media_dir: parsed.media_dir.clone(),
            media_query: parsed.media_query.clone(),
            info: parsed.info,
        };
        let mut store = self.lock();
        let tick = store.tick();
        store.routes.insert(
            key.clone(),
            Entry {
                track_id,
                upstream,
                cached: Cached::Nothing,
                active: 0,
                last_used: tick,
                touched: Instant::now(),
            },
        );
        drop(store);
        Ok((
            rewritten,
            Route {
                key,
                uri: String::new(),
                dash: true,
            },
        ))
    }

    /// Which tracks are wanted, most wanted first (the one playing now,
    /// then the upcoming ones); others' bytes are evicted first and their
    /// routes forgotten once unused (see [`Self::collect`]).
    pub fn set_ranks(&self, tracks: &[u64]) {
        let mut store = self.lock();
        store.ranks.clear();
        for (rank, &id) in tracks.iter().enumerate() {
            store.ranks.entry(id).or_insert(rank);
        }
    }

    /// Forgets routes of tracks no longer wanted that nothing used for
    /// `grace` (a deck fading out still streams from its route), and
    /// cached bytes of streams that were replaced (`stale`: route keys).
    pub fn collect(&self, grace: Duration, stale: &[String]) {
        let mut store = self.lock();
        let ranks = std::mem::take(&mut store.ranks);
        store.routes.retain(|key, entry| {
            let wanted = ranks.contains_key(&entry.track_id) && !stale.contains(key);
            wanted || entry.active > 0 || entry.touched.elapsed() < grace
        });
        for key in stale {
            if let Some(entry) = store.routes.get_mut(key) {
                entry.cached = Cached::Nothing;
            }
        }
        store.ranks = ranks;
    }

    /// Drops every route and cached byte (sign-out).
    pub fn clear(&self) {
        let mut store = self.lock();
        store.routes.clear();
        store.ranks.clear();
    }

    /// Cached bytes and the budget.
    pub fn usage(&self) -> (usize, usize) {
        let store = self.lock();
        (store.used(), store.budget)
    }

    // ---- Prefetching ----

    /// Downloads the first `seconds` of the route `key` into memory,
    /// within the budget; `bytes_per_second` estimates the size of a
    /// lossless file's prefix. Returns the bytes cached.
    pub async fn prefetch(&self, key: &str, seconds: f64, bytes_per_second: f64) -> Result<usize> {
        {
            let mut store = self.lock();
            store.set_budget(budget::budget(budget::available_memory()));
            if store.budget == 0 {
                return Ok(0);
            }
        }
        let plan = {
            let store = self.lock();
            let entry = store.routes.get(key).context("the route is gone")?;
            match &entry.upstream {
                Upstream::Progressive { url } => Plan::Prefix {
                    url: url.clone(),
                    bytes: prefix_bytes(seconds, bytes_per_second),
                },
                Upstream::Dash {
                    init,
                    media_dir,
                    media_query,
                    info,
                } => {
                    let mut files = vec![(format!("i/{}", file_name(init)), init.clone())];
                    for name in info.first_segments(seconds) {
                        let url = with_query(&format!("{media_dir}{name}"), media_query);
                        files.push((format!("m/{name}"), url));
                    }
                    Plan::Files(files)
                }
            }
        };
        match plan {
            Plan::Prefix { url, bytes } => self.prefetch_prefix(key, &url, bytes).await,
            Plan::Files(files) => self.prefetch_files(key, files).await,
        }
    }

    async fn prefetch_prefix(&self, key: &str, url: &str, want: usize) -> Result<usize> {
        if !self.lock().reserve(key, want) {
            log::debug!("prefetch: no room for {} KiB", want >> 10);
            return Ok(0);
        }
        let result = async {
            let mut response = self
                .http
                .get(url)
                .header(reqwest::header::RANGE, format!("bytes=0-{}", want - 1))
                .send()
                .await?
                .error_for_status()?;
            let total = match response.status().as_u16() {
                206 => header(&response, "content-range")
                    .and_then(|range| parse_content_range(&range))
                    .map(|(_, _, total)| total)
                    .context("no total in the Content-Range")?,
                _ => response.content_length().context("no Content-Length")?,
            };
            let content_type = header(&response, "content-type");
            let mut bytes = BytesMut::with_capacity(want);
            while bytes.len() < want {
                let Some(chunk) = response.chunk().await? else {
                    break;
                };
                let room = want - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            anyhow::Ok((bytes.freeze(), total, content_type))
        }
        .await;
        let mut store = self.lock();
        store.release(want);
        let (bytes, total, content_type) = result?;
        let len = bytes.len();
        let Some(entry) = store.routes.get_mut(key) else {
            return Ok(0);
        };
        if len == 0 || len as u64 > total {
            return Ok(0);
        }
        entry.cached = Cached::Prefix {
            bytes,
            total,
            content_type,
        };
        Ok(len)
    }

    async fn prefetch_files(&self, key: &str, files: Vec<(String, String)>) -> Result<usize> {
        let mut cached = 0;
        for (path, url) in files {
            let mut response = self.http.get(&url).send().await?.error_for_status()?;
            let Some(len) = response
                .content_length()
                .and_then(|l| usize::try_from(l).ok())
            else {
                break;
            };
            if cached + len > MAX_ENTRY || !self.lock().reserve(key, len) {
                log::debug!("prefetch: no room for {path}");
                break;
            }
            let body = async {
                let mut bytes = BytesMut::with_capacity(len);
                while let Some(chunk) = response.chunk().await? {
                    if bytes.len() + chunk.len() > len {
                        bail!("more bytes than announced");
                    }
                    bytes.extend_from_slice(&chunk);
                }
                anyhow::Ok(bytes.freeze())
            }
            .await;
            let mut store = self.lock();
            store.release(len);
            let body = body?;
            let Some(entry) = store.routes.get_mut(key) else {
                return Ok(cached);
            };
            cached += body.len();
            match &mut entry.cached {
                Cached::Files(files) => {
                    files.insert(path, body);
                }
                other => {
                    *other = Cached::Files(HashMap::from([(path, body)]));
                }
            }
        }
        Ok(cached)
    }

    // ---- Serving ----

    async fn serve(&self, mut socket: TcpStream) -> Result<()> {
        socket.set_nodelay(true)?;
        let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut socket))
            .await
            .context("the request head timed out")??;
        let request = parse_request(&head)?;
        let head_only = match request.method.as_str() {
            "GET" => false,
            "HEAD" => true,
            _ => return respond_empty(&mut socket, 405).await,
        };
        let Some((key, file)) = split_path(&request.path) else {
            return respond_empty(&mut socket, 404).await;
        };
        // Look the route up and mark it in use.
        let found = {
            let mut store = self.lock();
            let tick = store.tick();
            store.routes.get_mut(key).map(|entry| {
                entry.active += 1;
                entry.last_used = tick;
                entry.touched = Instant::now();
                let upstream = match &entry.upstream {
                    Upstream::Progressive { url } => url.clone(),
                    Upstream::Dash {
                        init,
                        media_dir,
                        media_query,
                        ..
                    } => match file.split_once('/') {
                        Some(("i", _)) => init.clone(),
                        Some(("m", name)) => with_query(&format!("{media_dir}{name}"), media_query),
                        _ => String::new(),
                    },
                };
                let cached = match (&entry.cached, file) {
                    (
                        Cached::Prefix {
                            bytes,
                            total,
                            content_type,
                        },
                        "",
                    ) => Hit::Prefix(bytes.clone(), *total, content_type.clone()),
                    (Cached::Files(files), file) if !file.is_empty() => {
                        files.get(file).cloned().map_or(Hit::Miss, Hit::File)
                    }
                    _ => Hit::Miss,
                };
                (entry.track_id, upstream, cached)
            })
        };
        let Some((track_id, upstream, hit)) = found else {
            return respond_empty(&mut socket, 404).await;
        };
        let _serving = Serving {
            proxy: self,
            key: key.to_owned(),
        };
        if upstream.is_empty() {
            return respond_empty(&mut socket, 404).await;
        }
        log::trace!(
            "proxy: track {track_id} {} {:?} ({})",
            if file.is_empty() { "file" } else { file },
            request.range,
            match &hit {
                Hit::Miss => "upstream",
                _ => "cached",
            }
        );
        match hit {
            Hit::Prefix(bytes, total, content_type)
                if request
                    .range
                    .is_none_or(|(start, _)| start < bytes.len() as u64) =>
            {
                self.serve_prefix(
                    &mut socket,
                    &request,
                    &upstream,
                    (bytes, total, content_type),
                    head_only,
                )
                .await
            }
            Hit::File(bytes) => serve_bytes(&mut socket, &request, &bytes, head_only).await,
            _ => {
                self.pass_through(&mut socket, &request, &upstream, head_only)
                    .await
            }
        }
    }

    /// Answers from the cached prefix, then streams the rest from
    /// upstream, requested in parallel.
    async fn serve_prefix(
        &self,
        socket: &mut TcpStream,
        request: &Request,
        upstream: &str,
        (prefix, total, content_type): (Bytes, u64, Option<String>),
        head_only: bool,
    ) -> Result<()> {
        let (start, end) = request.range.unwrap_or((0, None));
        if total == 0 || start >= total {
            return respond_empty(socket, 416).await;
        }
        let end = end.unwrap_or(total - 1).min(total - 1);
        let cached_end = (end + 1).min(prefix.len() as u64);
        let rest = (!head_only && end + 1 > cached_end).then(|| {
            let request = self
                .http
                .get(upstream)
                .header(reqwest::header::RANGE, format!("bytes={cached_end}-{end}"));
            tokio::spawn(request.send())
        });
        let status = if request.range.is_some() { 206 } else { 200 };
        let mut head = status_line(status);
        let _ = write!(head, "Content-Length: {}\r\n", end - start + 1);
        if status == 206 {
            let _ = write!(head, "Content-Range: bytes {start}-{end}/{total}\r\n");
        }
        if let Some(content_type) = &content_type {
            let _ = write!(head, "Content-Type: {content_type}\r\n");
        }
        head.push_str("Accept-Ranges: bytes\r\nConnection: close\r\n\r\n");
        socket.write_all(head.as_bytes()).await?;
        if head_only {
            return Ok(());
        }
        socket
            .write_all(&prefix[start as usize..cached_end as usize])
            .await?;
        let Some(rest) = rest else {
            return Ok(());
        };
        let mut response = rest.await??;
        let range = header(&response, "content-range").and_then(|r| parse_content_range(&r));
        if response.status().as_u16() != 206
            || range.is_none_or(|(from, _, size)| from != cached_end || size != total)
        {
            // Not the same file any more: close, so the source fails
            // (and the track is reloaded) instead of playing garbage.
            bail!(
                "upstream answered {} {range:?} for the rest after {cached_end}",
                response.status()
            );
        }
        while let Some(chunk) = response.chunk().await? {
            socket.write_all(&chunk).await?;
        }
        Ok(())
    }

    /// Forwards the request to `upstream` and streams the answer back.
    async fn pass_through(
        &self,
        socket: &mut TcpStream,
        request: &Request,
        upstream: &str,
        head_only: bool,
    ) -> Result<()> {
        let mut forward = if head_only {
            self.http.head(upstream)
        } else {
            self.http.get(upstream)
        };
        if let Some(range) = &request.range_header {
            forward = forward.header(reqwest::header::RANGE, range);
        }
        let asked = Instant::now();
        let mut response = match forward.send().await {
            Ok(response) => response,
            Err(err) => {
                respond_empty(socket, 502).await?;
                return Err(anyhow!(err).context("upstream request failed"));
            }
        };
        let status = response.status().as_u16();
        log::trace!(
            "proxy: {} answered {status} in {} ms",
            file_name(upstream),
            asked.elapsed().as_millis()
        );
        if !response.status().is_success() {
            return respond_empty(socket, status).await;
        }
        let mut head = status_line(status);
        for name in ["content-length", "content-range", "content-type"] {
            if let Some(value) = header(&response, name) {
                let _ = write!(head, "{name}: {value}\r\n");
            }
        }
        head.push_str("Accept-Ranges: bytes\r\nConnection: close\r\n\r\n");
        socket.write_all(head.as_bytes()).await?;
        if head_only {
            return Ok(());
        }
        while let Some(chunk) = response.chunk().await? {
            socket.write_all(&chunk).await?;
        }
        Ok(())
    }
}

enum Plan {
    Prefix { url: String, bytes: usize },
    Files(Vec<(String, String)>),
}

enum Hit {
    Miss,
    Prefix(Bytes, u64, Option<String>),
    File(Bytes),
}

/// Bytes to prefetch of a lossless file for `seconds` of audio, with room
/// for the headers before the audio.
pub fn prefix_bytes(seconds: f64, bytes_per_second: f64) -> usize {
    let bytes = (seconds.max(0.0) * bytes_per_second.max(0.0)) as usize + (64 << 10);
    bytes.clamp(256 << 10, MAX_ENTRY)
}

fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn with_query(url: &str, query: &str) -> String {
    if query.is_empty() {
        url.to_owned()
    } else {
        format!("{url}?{query}")
    }
}

/// The last path segment of a URL, without its query.
fn file_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}

fn status_line(status: u16) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        502 => "Bad Gateway",
        _ => "Status",
    };
    format!("HTTP/1.1 {status} {reason}\r\n")
}

async fn respond_empty(socket: &mut TcpStream, status: u16) -> Result<()> {
    let mut head = status_line(status);
    head.push_str("Content-Length: 0\r\nConnection: close\r\n\r\n");
    socket.write_all(head.as_bytes()).await?;
    Ok(())
}

/// Serves a cached file, honouring a range.
async fn serve_bytes(
    socket: &mut TcpStream,
    request: &Request,
    bytes: &Bytes,
    head_only: bool,
) -> Result<()> {
    let total = bytes.len() as u64;
    let (start, end) = request.range.unwrap_or((0, None));
    if start >= total.max(1) {
        return respond_empty(socket, 416).await;
    }
    let end = end.unwrap_or(total - 1).min(total - 1);
    let status = if request.range.is_some() { 206 } else { 200 };
    let mut head = status_line(status);
    let _ = write!(head, "Content-Length: {}\r\n", end - start + 1);
    if status == 206 {
        let _ = write!(head, "Content-Range: bytes {start}-{end}/{total}\r\n");
    }
    head.push_str("Content-Type: video/mp4\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n");
    socket.write_all(head.as_bytes()).await?;
    if !head_only {
        socket
            .write_all(&bytes[start as usize..=end as usize])
            .await?;
    }
    Ok(())
}

async fn read_head(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    let mut buffer = [0u8; 2048];
    loop {
        let n = socket.read(&mut buffer).await?;
        if n == 0 {
            bail!("the client closed before sending a request");
        }
        head.extend_from_slice(&buffer[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(head);
        }
        if head.len() > MAX_HEAD {
            bail!("request head too big");
        }
    }
}

#[derive(Debug, PartialEq)]
struct Request {
    method: String,
    path: String,
    /// The `Range` header as sent, and parsed.
    range_header: Option<String>,
    range: Option<(u64, Option<u64>)>,
}

fn parse_request(head: &[u8]) -> Result<Request> {
    let text = std::str::from_utf8(head).context("the request is not UTF-8")?;
    let mut lines = text.split("\r\n");
    let line = lines.next().context("no request line")?;
    let mut parts = line.split(' ');
    let method = parts.next().context("no method")?.to_owned();
    let path = parts.next().context("no path")?.to_owned();
    let mut range_header = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            range_header = Some(value.trim().to_owned());
        }
    }
    let range = range_header.as_deref().and_then(parse_range);
    Ok(Request {
        method,
        path,
        range_header,
        range,
    })
}

/// `bytes=START-[END]` (one range; suffix ranges are not used by
/// GStreamer).
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        end => Some(end.parse::<u64>().ok().filter(|e| *e >= start)?),
    };
    Some((start, end))
}

/// `bytes START-END/TOTAL`.
fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let spec = value.trim().strip_prefix("bytes ")?;
    let (range, total) = spec.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((
        start.trim().parse().ok()?,
        end.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

/// `/p/<key>` → (key, ""), `/d/<key>/<file>` → (key, file).
fn split_path(path: &str) -> Option<(&str, &str)> {
    let path = path.split(['?', '#']).next()?;
    if let Some(key) = path.strip_prefix("/p/") {
        return (!key.is_empty() && !key.contains('/')).then_some((key, ""));
    }
    let rest = path.strip_prefix("/d/")?;
    let (key, file) = rest.split_once('/')?;
    (!key.is_empty() && !file.is_empty()).then_some((key, file))
}

// ---- DASH manifests ----

/// What the proxy needs of a DASH manifest's `SegmentTemplate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DashInfo {
    /// The media file name, e.g. `$Number$.mp4`.
    template: String,
    start_number: u64,
    timescale: u64,
    /// Durations of the first segments, in `timescale` units.
    durations: Vec<u64>,
}

impl DashInfo {
    /// File names of the media segments covering the first `seconds`.
    fn first_segments(&self, seconds: f64) -> Vec<String> {
        if !self.template.contains("$Number$") || self.timescale == 0 {
            return Vec::new();
        }
        let probe = self.template.replace("$Number$", "0");
        if probe.contains('$') {
            // Other identifiers ($Time$, widths): not worth guessing.
            return Vec::new();
        }
        let mut covered = 0.0;
        let mut names = Vec::new();
        for (i, duration) in self.durations.iter().enumerate().take(MAX_SEGMENTS) {
            if covered >= seconds {
                break;
            }
            covered += *duration as f64 / self.timescale as f64;
            let number = self.start_number + i as u64;
            names.push(self.template.replace("$Number$", &number.to_string()));
        }
        names
    }
}

#[derive(Debug)]
struct ParsedMpd {
    init_url: String,
    media_dir: String,
    media_query: String,
    info: DashInfo,
    /// Byte ranges of the two attribute values in the manifest.
    init_span: (usize, usize),
    media_span: (usize, usize),
    init_name: String,
}

impl ParsedMpd {
    fn rewrite(&self, mpd: &str, base: &str) -> String {
        let init = xml_escape(&format!("{base}/i/{}", self.init_name));
        let media = xml_escape(&format!("{base}/m/{}", self.info.template));
        let mut spans = [(self.init_span, init), (self.media_span, media)];
        spans.sort_by_key(|((start, _), _)| *start);
        let mut out = String::with_capacity(mpd.len());
        let mut at = 0;
        for ((start, end), value) in spans {
            out.push_str(&mpd[at..start]);
            out.push_str(&value);
            at = end;
        }
        out.push_str(&mpd[at..]);
        out
    }
}

/// The value of attribute `name` in `tag` (the text of one start tag), and
/// where it is (byte offsets into `tag`).
fn attribute<'a>(tag: &'a str, name: &str) -> Option<(&'a str, usize, usize)> {
    let mut from = 0;
    while let Some(found) = tag[from..].find(name) {
        let at = from + found;
        let before_ok = at > 0 && tag.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = &tag[at + name.len()..];
        if before_ok && let Some(rest) = rest.strip_prefix("=\"") {
            let start = at + name.len() + 2;
            let end = start + rest.find('"')?;
            return Some((&tag[start..end], start, end));
        }
        from = at + name.len();
    }
    None
}

fn parse_mpd(mpd: &str) -> Result<ParsedMpd> {
    if mpd.contains("<BaseURL") {
        bail!("manifests with a BaseURL are played as they are");
    }
    let tag_start = mpd.find("<SegmentTemplate").context("no SegmentTemplate")?;
    if mpd[tag_start + 1..].contains("<SegmentTemplate") {
        bail!("more than one SegmentTemplate");
    }
    let tag_end = tag_start + mpd[tag_start..].find('>').context("unterminated tag")?;
    let tag = &mpd[tag_start..tag_end];
    let (init, init_start, init_end) = attribute(tag, "initialization").context("no init")?;
    let (media, media_start, media_end) = attribute(tag, "media").context("no media")?;
    let init_url = xml_unescape(init);
    let media_url = xml_unescape(media);
    let (media_path, media_query) = media_url.split_once('?').unwrap_or((&media_url, ""));
    let slash = media_path.rfind('/').context("no media path")?;
    let (media_dir, template) = media_path.split_at(slash + 1);
    if media_dir.contains('$') || !template.contains('$') || !media_dir.starts_with("http") {
        bail!("unexpected media template");
    }
    if !init_url.starts_with("http") {
        bail!("unexpected init URL");
    }
    let init_name = file_name(&init_url).to_owned();
    if init_name.is_empty() {
        bail!("no init file name");
    }
    let number = |name| attribute(tag, name).and_then(|(v, _, _)| v.parse::<u64>().ok());
    let timescale = number("timescale").unwrap_or(1);
    let start_number = number("startNumber").unwrap_or(1);
    let mut durations = Vec::new();
    if let Some(timeline) = mpd[tag_end..]
        .find("<SegmentTimeline")
        .map(|at| &mpd[tag_end + at..])
    {
        let timeline = &timeline[..timeline.find("</SegmentTimeline").unwrap_or(timeline.len())];
        for s in timeline.split("<S ").skip(1) {
            let s = &s[..s.find('>').unwrap_or(s.len())];
            let s = format!(" {s}");
            let Some(d) = attribute(&s, "d").and_then(|(v, _, _)| v.parse::<u64>().ok()) else {
                continue;
            };
            let repeat = attribute(&s, "r")
                .and_then(|(v, _, _)| v.parse::<i64>().ok())
                .unwrap_or(0);
            let count = if repeat < 0 {
                MAX_SEGMENTS
            } else {
                repeat as usize + 1
            };
            durations.extend(std::iter::repeat_n(d, count.min(MAX_SEGMENTS)));
            if durations.len() >= MAX_SEGMENTS {
                break;
            }
        }
    } else if let Some(d) = number("duration") {
        durations = vec![d; MAX_SEGMENTS];
    }
    durations.truncate(MAX_SEGMENTS);
    Ok(ParsedMpd {
        init_url,
        media_dir: media_dir.to_owned(),
        media_query: media_query.to_owned(),
        info: DashInfo {
            template: template.to_owned(),
            start_number,
            timescale,
            durations,
        },
        init_span: (tag_start + init_start, tag_start + init_end),
        media_span: (tag_start + media_start, tag_start + media_end),
        init_name,
    })
}

/// The signed URLs in a DASH manifest (for their expiry).
pub fn dash_urls(mpd: &str) -> Vec<String> {
    let Some(start) = mpd.find("<SegmentTemplate") else {
        return Vec::new();
    };
    let tag = &mpd[start..start + mpd[start..].find('>').unwrap_or(mpd.len() - start)];
    ["initialization", "media"]
        .iter()
        .filter_map(|name| attribute(tag, name).map(|(v, _, _)| xml_unescape(v)))
        .collect()
}

fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MPD: &str = r#"<?xml version='1.0' encoding='UTF-8'?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT5M15.602S"><Period id="0"><AdaptationSet id="0" contentType="audio" mimeType="audio/mp4"><Representation id="FLAC_HIRES,48000,24" codecs="flac" bandwidth="1675219" audioSamplingRate="48000"><SegmentTemplate timescale="48000" initialization="https://sp-ad-cf.audio.tidal.com/mediatracks/AbC/0.mp4?Policy=P&amp;Signature=S~x&amp;Key-Pair-Id=K" media="https://sp-ad-cf.audio.tidal.com/mediatracks/AbC/$Number$.mp4?Policy=P&amp;Signature=S~x&amp;Key-Pair-Id=K" startNumber="1"><SegmentTimeline><S d="188416" r="79"/><S d="75651"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;

    #[test]
    fn rewrites_tidal_manifests() {
        let parsed = parse_mpd(MPD).unwrap();
        assert_eq!(
            parsed.init_url,
            "https://sp-ad-cf.audio.tidal.com/mediatracks/AbC/0.mp4?Policy=P&Signature=S~x&Key-Pair-Id=K"
        );
        assert_eq!(
            parsed.media_dir,
            "https://sp-ad-cf.audio.tidal.com/mediatracks/AbC/"
        );
        assert_eq!(parsed.media_query, "Policy=P&Signature=S~x&Key-Pair-Id=K");
        assert_eq!(parsed.init_name, "0.mp4");
        assert_eq!(parsed.info.template, "$Number$.mp4");
        assert_eq!(parsed.info.timescale, 48000);
        assert_eq!(parsed.info.start_number, 1);
        assert_eq!(parsed.info.durations, vec![188416; MAX_SEGMENTS]);
        let rewritten = parsed.rewrite(MPD, "http://127.0.0.1:5/d/KEY");
        assert!(rewritten.contains(r#"initialization="http://127.0.0.1:5/d/KEY/i/0.mp4""#));
        assert!(rewritten.contains(r#"media="http://127.0.0.1:5/d/KEY/m/$Number$.mp4""#));
        assert!(!rewritten.contains("Signature"));
        assert!(!rewritten.contains("tidal.com"));
        // The rest is untouched.
        assert!(rewritten.starts_with("<?xml version='1.0'"));
        assert!(rewritten.contains(r#"<S d="188416" r="79"/>"#));
        let media = format!("{}$Number$.mp4?{}", parsed.media_dir, parsed.media_query);
        assert_eq!(dash_urls(MPD), [parsed.init_url.clone(), media]);
    }

    #[test]
    fn picks_the_first_segments() {
        let info = parse_mpd(MPD).unwrap().info;
        // 3.925 s each: 8 s need three.
        assert_eq!(info.first_segments(8.0), ["1.mp4", "2.mp4", "3.mp4"]);
        assert_eq!(info.first_segments(3.0), ["1.mp4"]);
        assert_eq!(info.first_segments(0.0), Vec::<String>::new());
        assert_eq!(info.first_segments(1000.0).len(), MAX_SEGMENTS);
        let other = DashInfo {
            template: "$Time$.mp4".into(),
            ..info.clone()
        };
        assert!(other.first_segments(8.0).is_empty());
        let padded = DashInfo {
            template: "seg-$Number%05d$.m4s".into(),
            ..info
        };
        assert!(padded.first_segments(8.0).is_empty());
    }

    #[test]
    fn refuses_unknown_shapes() {
        assert!(
            parse_mpd("<MPD><BaseURL>https://x/</BaseURL><SegmentTemplate media=\"a\"/></MPD>")
                .is_err()
        );
        assert!(parse_mpd("<MPD/>").is_err());
        let two = MPD.replace(
            "</Representation>",
            "</Representation><Representation><SegmentTemplate media=\"x\"/></Representation>",
        );
        assert!(parse_mpd(&two).is_err());
        let relative = MPD.replace(
            "https://sp-ad-cf.audio.tidal.com/mediatracks/AbC/$Number$",
            "$Number$",
        );
        assert!(parse_mpd(&relative).is_err());
        // A uniform duration instead of a timeline.
        let uniform = r#"<MPD><SegmentTemplate timescale="1000" duration="4000" initialization="https://h/a/init.mp4" media="https://h/a/$Number$.m4s" startNumber="0"/></MPD>"#;
        let parsed = parse_mpd(uniform).unwrap();
        assert_eq!(parsed.info.first_segments(8.0), ["0.m4s", "1.m4s"]);
        assert_eq!(parsed.media_query, "");
    }

    #[test]
    fn parses_requests_and_ranges() {
        let request = parse_request(
            b"GET /p/abc HTTP/1.1\r\nHost: 127.0.0.1\r\nrange: bytes=100-\r\nIcy-MetaData: 1\r\n\r\n",
        )
        .unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/p/abc");
        assert_eq!(request.range, Some((100, None)));
        assert_eq!(request.range_header.as_deref(), Some("bytes=100-"));
        assert_eq!(parse_range("bytes=0-499"), Some((0, Some(499))));
        assert_eq!(parse_range("bytes=500-100"), None);
        assert_eq!(parse_range("bytes=0-1,5-6"), None);
        assert_eq!(parse_range("items=0-1"), None);
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, 99, 1000)));
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(split_path("/p/abc"), Some(("abc", "")));
        assert_eq!(split_path("/d/abc/m/3.mp4"), Some(("abc", "m/3.mp4")));
        assert_eq!(split_path("/d/abc"), None);
        assert_eq!(split_path("/x/abc"), None);
        assert_eq!(split_path("/p/"), None);
    }

    #[test]
    fn prefix_sizes_are_bounded() {
        // CD quality, about 110 KB/s of FLAC: 8 s plus headers.
        assert_eq!(prefix_bytes(8.0, 110_000.0), 880_000 + (64 << 10));
        assert_eq!(prefix_bytes(0.0, 0.0), 256 << 10);
        assert_eq!(prefix_bytes(1e6, 1e6), MAX_ENTRY);
    }

    /// A stand-in for the CDN on 127.0.0.1: serves `file` with ranges.
    async fn fake_cdn(file: Bytes) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let file = file.clone();
                tokio::spawn(async move {
                    let head = read_head(&mut socket).await.unwrap();
                    let request = parse_request(&head).unwrap();
                    let total = file.len() as u64;
                    let (start, end) = request.range.unwrap_or((0, None));
                    let end = end.unwrap_or(total - 1).min(total - 1);
                    let mut head = status_line(if request.range.is_some() { 206 } else { 200 });
                    let _ = write!(head, "Content-Length: {}\r\n", end - start + 1);
                    if request.range.is_some() {
                        let _ = write!(head, "Content-Range: bytes {start}-{end}/{total}\r\n");
                    }
                    head.push_str("Content-Type: audio/flac\r\nConnection: close\r\n\r\n");
                    socket.write_all(head.as_bytes()).await.unwrap();
                    let _ = socket.write_all(&file[start as usize..=end as usize]).await;
                });
            }
        });
        format!("http://127.0.0.1:{port}/track.flac?token=secret")
    }

    async fn fetch(url: &str, range: Option<&str>) -> (u16, Option<String>, Vec<u8>) {
        let client = reqwest::Client::new();
        let mut request = client.get(url);
        if let Some(range) = range {
            request = request.header(reqwest::header::RANGE, range);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let content_range = header(&response, "content-range");
        (
            status,
            content_range,
            response.bytes().await.unwrap().to_vec(),
        )
    }

    /// The whole path of a lossless file: routed, its prefix prefetched,
    /// then read through the proxy whole and in ranges inside, across and
    /// past the cached prefix, always byte for byte the original.
    #[test]
    fn serves_prefix_then_upstream() {
        let file: Bytes = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        crate::runtime::runtime().block_on(async {
            let upstream = fake_cdn(file.clone()).await;
            let proxy = Proxy::start().unwrap();
            let route = proxy.route_progressive(42, &upstream);
            assert!(!route.uri.contains("secret"));
            proxy.set_ranks(&[42]);
            let cached = proxy.prefetch(&route.key, 1.0, 100_000.0).await.unwrap();
            assert_eq!(cached, prefix_bytes(1.0, 100_000.0));
            assert!(cached < file.len());
            assert!(proxy.usage().0 <= proxy.usage().1);

            let (status, _, body) = fetch(&route.uri, None).await;
            assert_eq!(status, 200);
            assert!(body == file[..], "whole file");
            for (range, from, to) in [
                ("bytes=10-99", 10, 99),
                ("bytes=1000-", 1000, file.len() - 1),
                (
                    &format!("bytes={}-{}", cached - 5, cached + 5),
                    cached - 5,
                    cached + 5,
                ),
                (
                    &format!("bytes={}-", cached + 10),
                    cached + 10,
                    file.len() - 1,
                ),
            ] {
                let (status, content_range, body) = fetch(&route.uri, Some(range)).await;
                assert_eq!(status, 206, "{range}");
                assert_eq!(
                    content_range.as_deref(),
                    Some(format!("bytes {from}-{to}/{}", file.len()).as_str())
                );
                assert!(body == file[from..=to], "{range}");
            }
            // Unknown routes are not found.
            let (status, _, _) = fetch(&route.uri.replace(&route.key, "nope"), None).await;
            assert_eq!(status, 404);
            // Forgotten once not wanted and unused.
            proxy.set_ranks(&[]);
            proxy.collect(Duration::ZERO, &[]);
            let (status, _, _) = fetch(&route.uri, None).await;
            assert_eq!(status, 404);
        });
    }

    #[test]
    fn store_never_exceeds_its_budget() {
        let mut store = Store {
            budget: 1000,
            ..Store::default()
        };
        for (i, key) in ["a", "b", "c"].iter().enumerate() {
            store.routes.insert(
                (*key).to_owned(),
                Entry {
                    track_id: i as u64,
                    upstream: Upstream::Progressive { url: String::new() },
                    cached: Cached::Nothing,
                    active: 0,
                    last_used: 0,
                    touched: Instant::now(),
                },
            );
        }
        store.ranks = HashMap::from([(0, 0), (1, 1), (2, 2)]);
        assert!(store.reserve("a", 600));
        store.release(600);
        store.routes.get_mut("a").unwrap().cached = Cached::Prefix {
            bytes: Bytes::from(vec![0; 600]),
            total: 10_000,
            content_type: None,
        };
        // Rank 1 cannot evict rank 0.
        assert!(!store.reserve("b", 500));
        assert!(store.reserve("b", 400));
        store.release(400);
        store.routes.get_mut("b").unwrap().cached = Cached::Files(HashMap::from([(
            "i/0.mp4".to_owned(),
            Bytes::from(vec![0; 400]),
        )]));
        assert_eq!(store.used(), 1000);
        // Track 1 is no longer wanted: rank 2 may take its place.
        store.ranks.remove(&1);
        assert!(store.reserve("c", 300));
        assert_eq!(store.routes["b"].cached.len(), 0);
        assert!(store.used() <= store.budget);
        // A lower budget evicts until it fits.
        store.release(300);
        store.set_budget(500);
        assert!(store.used() <= 500);
    }
}
