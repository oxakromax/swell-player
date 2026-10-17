//! The signed-in API client: the TIDAL client plus the app-level work built
//! on it (keeping the keyring in sync, syncing favourites, resolving
//! streams).

use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::{StreamExt, stream};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::cache::{Cache, CachedCollection};
use crate::config;
use crate::model::TrackInfo;
use crate::playback::Stream;
use crate::session::{self, LoginFlow, StoredSession};
use crate::tidal::{self, Authz, Client, DeviceCode, Pkce};

/// One HTTP client for the whole app: API calls and image downloads share
/// its connection pool. Both TIDAL hosts speak HTTP/2, so concurrent
/// requests are multiplexed over one kept-alive connection per host.
///
/// Names are resolved with hickory instead of glibc's `getaddrinfo`, which
/// on some networks stalls for its 5 s timeout when one of the parallel
/// A/AAAA answers is lost.
///
/// HTTP/2 pings detect a connection that died silently (network switch,
/// VPN), so requests multiplexed on it fail fast instead of hanging; every
/// request also has an overall deadline.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .http2_keep_alive_interval(Duration::from_secs(20))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        .build()
        .expect("failed to build the HTTP client")
}

/// Opens the connections to TIDAL's API and image hosts in the background,
/// so DNS and TLS overlap with reading the keyring instead of delaying the
/// first real request.
pub fn preconnect(http: &reqwest::Client) {
    for url in [API_HOST, IMAGE_HOST] {
        let request = http.head(url);
        crate::runtime::runtime().spawn(async move {
            if let Err(err) = request.send().await {
                log::debug!("preconnect to {url} failed: {err}");
            }
        });
    }
}

const API_HOST: &str = "https://api.tidal.com/";
const IMAGE_HOST: &str = "https://resources.tidal.com/";

pub struct Api {
    pub client: Client,
    cache: tokio::sync::OnceCell<Option<Cache>>,
}

impl Deref for Api {
    type Target = Client;

    fn deref(&self) -> &Client {
        &self.client
    }
}

impl Api {
    /// A client for a session; refreshed tokens are written back to the
    /// keyring unless the user has signed out meanwhile.
    pub fn restore(stored: StoredSession, http: reqwest::Client) -> Arc<Self> {
        let flow = stored.flow;
        let client = Client::new(http, flow, stored.authz, move |authz: &Authz| {
            let authz = authz.clone();
            tokio::spawn(async move {
                if let Err(err) = session::save(&StoredSession { flow, authz }).await {
                    log::warn!("could not store refreshed tokens: {err:#}");
                }
            });
        });
        Arc::new(Self {
            client,
            cache: tokio::sync::OnceCell::new(),
        })
    }

    /// The library cache, opened on first use (`None` if it cannot be).
    pub async fn cache(&self) -> Option<&Cache> {
        self.cache
            .get_or_init(|| async {
                Cache::open()
                    .await
                    .inspect_err(|err| log::warn!("library cache unavailable: {err:#}"))
                    .ok()
            })
            .await
            .as_ref()
    }

    /// Stale-while-revalidate GET: sends the cached answer (if any) parsed
    /// with `parse`, then, unless it is younger than `fresh_for`, fetches,
    /// stores and sends the fresh one — but only if it differs. An error is
    /// sent only when there was nothing cached to show.
    pub fn load_cached<T: Send + 'static>(
        self: &Arc<Self>,
        base: tidal::Base,
        path: String,
        fresh_for: Duration,
        parse: fn(&serde_json::Value) -> T,
    ) -> tokio::sync::mpsc::UnboundedReceiver<Result<T>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let this = self.clone();
        crate::runtime::runtime().spawn(async move {
            let key = format!("{}{path}", base.prefix());
            let user_id = this.user_id();
            let cached = match this.cache().await {
                Some(cache) => cache.response(user_id, key.clone()).await.ok().flatten(),
                None => None,
            };
            let mut shown = None;
            if let Some((data, age)) = cached
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data)
            {
                let _ = tx.send(Ok(parse(&value)));
                if age < fresh_for {
                    return;
                }
                shown = Some(data);
            }
            let call = tidal::Call::get(base, path.clone());
            let response = match this.send(&call).await {
                Ok(response) => response,
                Err(err) => {
                    if shown.is_none() {
                        let _ = tx.send(Err(err));
                    } else {
                        log::debug!("revalidating {path} failed: {err:#}");
                    }
                    return;
                }
            };
            let fresh = response.body.to_vec();
            if shown.as_deref() != Some(fresh.as_slice()) {
                match serde_json::from_slice::<serde_json::Value>(&fresh) {
                    Ok(value) => {
                        let _ = tx.send(Ok(parse(&value)));
                    }
                    Err(err) => {
                        if shown.is_none() {
                            let _ = tx.send(Err(err.into()));
                        }
                        return;
                    }
                }
            }
            if !this.is_signed_out()
                && let Some(cache) = this.cache().await
                && let Err(err) = cache.store_response(user_id, key, fresh).await
            {
                log::debug!("could not cache {path}: {err:#}");
            }
        });
        rx
    }

    /// Forgets cached answers under `prefix` (e.g. after editing a playlist).
    pub async fn invalidate(&self, base: tidal::Base, prefix: &str) {
        if let Some(cache) = self.cache().await {
            let _ = cache
                .forget_responses(self.user_id(), format!("{}{prefix}", base.prefix()))
                .await;
        }
    }

    /// Persists the tokens the client currently holds. Called once a login
    /// has won, never by a flow that might still lose.
    pub async fn store(&self) -> Result<()> {
        session::save(&StoredSession {
            flow: self.flow,
            authz: (*self.authz()).clone(),
        })
        .await
    }

    /// First half of the PKCE login: the URL to open in the browser.
    pub fn start_pkce() -> Result<Pkce> {
        Pkce::start(LoginFlow::Pkce.oauth_client(), config::PKCE_REDIRECT_URI)
    }

    /// Second half of the PKCE login: `redirect_url` is the address of the
    /// page TIDAL lands on after signing in; it carries the one-time code.
    pub async fn finish_pkce(
        http: reqwest::Client,
        pkce: &Pkce,
        redirect_url: &str,
    ) -> Result<Arc<Self>> {
        let authz = pkce
            .finish(&http, LoginFlow::Pkce.oauth_client(), redirect_url)
            .await?;
        Ok(Self::restore(
            StoredSession {
                flow: LoginFlow::Pkce,
                authz,
            },
            http,
        ))
    }

    pub async fn start_device_login(http: &reqwest::Client) -> Result<DeviceCode> {
        DeviceCode::start(http, LoginFlow::DeviceCode.oauth_client()).await
    }

    /// Polls until the user approves the code or it expires.
    pub async fn finish_device_login(
        http: reqwest::Client,
        device: &DeviceCode,
    ) -> Result<Arc<Self>> {
        let authz = device
            .wait(&http, LoginFlow::DeviceCode.oauth_client())
            .await?;
        Ok(Self::restore(
            StoredSession {
                flow: LoginFlow::DeviceCode,
                authz,
            },
            http,
        ))
    }

    /// Removes the tokens from the keyring, then (best effort) the user's
    /// cached library. Running work must be stopped first.
    pub async fn sign_out(&self) -> Result<()> {
        self.mark_signed_out();
        session::clear().await?;
        let user_id = self.user_id();
        let forget = async { Cache::open().await?.forget_user(user_id).await };
        if let Err(err) = forget.await {
            log::warn!("could not clear the library cache: {err:#}");
        }
        Ok(())
    }

    /// Brings the favourite tracks up to date and reports to `updates`.
    ///
    /// With nothing cached, the list fills page by page ([`FavoritesUpdate::Append`]).
    /// With a cached list, one request usually settles it: favourites come
    /// newest first, so if the first page and the total still match, nothing
    /// was added or removed. Otherwise the whole list is fetched and sent at
    /// once ([`FavoritesUpdate::Replace`]). A fresh download is written back
    /// to `cache`. Success always ends with [`FavoritesUpdate::Finished`].
    pub async fn sync_favorite_tracks(
        self: Arc<Self>,
        cache: Option<Cache>,
        cached: Option<CachedHead>,
        updates: UnboundedSender<Result<FavoritesUpdate>>,
    ) {
        if let Err(err) = self.sync_favorites(cache, cached, &updates).await {
            let _ = updates.send(Err(err));
        }
    }

    async fn sync_favorites(
        &self,
        cache: Option<Cache>,
        cached: Option<CachedHead>,
        updates: &UnboundedSender<Result<FavoritesUpdate>>,
    ) -> Result<()> {
        let first = self.favorite_tracks_page(0).await?;
        let total = first.total;
        let first = into_infos(first.items);
        if cached
            .as_ref()
            .is_some_and(|cached| cached.matches(total, &first))
        {
            let _ = updates.send(Ok(FavoritesUpdate::Finished));
            return Ok(());
        }

        // Without a cached list on screen, show each page as it arrives.
        let progressive = cached.is_none();
        let mut all = Vec::with_capacity(total);
        if progressive {
            let _ = updates.send(Ok(FavoritesUpdate::Append(first.clone())));
        }
        all.extend(first);

        let mut rest = stream::iter((FAVORITES_PAGE..total).step_by(FAVORITES_PAGE))
            .map(|offset| self.favorite_tracks_page(offset))
            .buffered(FAVORITES_CONCURRENCY);
        while let Some(page) = rest.next().await {
            let page = into_infos(page?.items);
            if progressive {
                let _ = updates.send(Ok(FavoritesUpdate::Append(page.clone())));
            }
            all.extend(page);
        }
        if !progressive {
            let _ = updates.send(Ok(FavoritesUpdate::Replace(all.clone())));
        }
        let _ = updates.send(Ok(FavoritesUpdate::Finished));

        if let Some(cache) = cache
            && !self.is_signed_out()
            && let Err(err) = cache.store_favorite_tracks(self.user_id(), total, all).await
        {
            log::warn!("could not cache favourites: {err:#}");
        }
        Ok(())
    }

    /// Asks TIDAL how to stream `track_id` at the best quality this session
    /// allows. Hi-Res comes back as MPEG-DASH, the rest as a direct URL.
    pub async fn stream(&self, track_id: u64) -> Result<Stream> {
        self.stream_with_quality(track_id, self.flow.max_quality())
            .await
    }

    /// Like [`Self::stream`], asking for `quality` (which the caller caps
    /// at what the session allows). TIDAL may answer with less when the
    /// track is not available in it.
    pub async fn stream_with_quality(&self, track_id: u64, quality: tidal::Quality) -> Result<Stream> {
        let info = self.playback_info(track_id, quality).await?;
        Stream::from_playback_info(info).await
    }

    async fn favorite_tracks_page(&self, offset: usize) -> Result<tidal::models::Page<tidal::models::Track>> {
        let page = self.favorite_tracks(offset, FAVORITES_PAGE).await?;
        log::debug!(
            "favourites at offset {offset}: {} items of {}",
            page.items.len(),
            page.total
        );
        Ok(page)
    }
}

impl Api {
    /// Brings a list of favourite albums or artists up to date, like
    /// [`Self::sync_favorite_tracks`] does for tracks, keeping the whole
    /// list as one cached answer.
    ///
    /// Sends the cached list first ([`ListUpdate::Cached`]), then checks it
    /// with one request: favourites come newest first, so if the first page
    /// and the total still match, it is current. Otherwise the list is
    /// fetched again, page by page ([`ListUpdate::Append`]) when nothing was
    /// cached or at once ([`ListUpdate::Replace`]), and cached. Success
    /// always ends with [`ListUpdate::Finished`].
    pub fn sync_favorite_list<T>(
        self: &Arc<Self>,
        kind: tidal::Favorite,
    ) -> UnboundedReceiver<Result<ListUpdate<T>>>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let this = self.clone();
        crate::runtime::runtime().spawn(async move {
            if let Err(err) = this.sync_list(kind, &tx).await {
                let _ = tx.send(Err(err));
            }
        });
        rx
    }

    fn favorite_list_path(&self, kind: tidal::Favorite) -> String {
        let plural = match kind {
            tidal::Favorite::Track => "tracks",
            tidal::Favorite::Album => "albums",
            tidal::Favorite::Artist => "artists",
            tidal::Favorite::Playlist => "playlists",
        };
        format!("users/{}/favorites/{plural}", self.user_id())
    }

    async fn sync_list<T: DeserializeOwned>(
        &self,
        kind: tidal::Favorite,
        updates: &UnboundedSender<Result<ListUpdate<T>>>,
    ) -> Result<()> {
        let user_id = self.user_id();
        let path = self.favorite_list_path(kind);
        let key = format!("{}{path}?all", tidal::Base::V1.prefix());
        let cache = self.cache().await;

        let mut cached: Option<(Vec<Value>, usize, Duration)> = None;
        if let Some(cache) = cache
            && let Ok(Some((data, age))) = cache.response(user_id, key.clone()).await
            && let Ok(value) = serde_json::from_slice::<Value>(&data)
        {
            let (items, total) = list_of(value);
            let _ = updates.send(Ok(ListUpdate::Cached(parse_items(&items))));
            cached = Some((items, total, age));
        }

        let (first, total) = list_of(self.favorite_list_page(&path, 0).await?);
        if let Some((items, cached_total, age)) = &cached
            && *age < FAVORITES_MAX_AGE
            && *cached_total == total
            && first.len() <= items.len()
            && first.iter().zip(items).all(|(a, b)| item_id(a) == item_id(b))
        {
            let _ = updates.send(Ok(ListUpdate::Finished));
            return Ok(());
        }

        let progressive = cached.is_none();
        if progressive {
            let _ = updates.send(Ok(ListUpdate::Append(parse_items(&first))));
        }
        let mut all = first;
        let mut rest = stream::iter((FAVORITES_PAGE..total).step_by(FAVORITES_PAGE))
            .map(|offset| self.favorite_list_page(&path, offset))
            .buffered(FAVORITES_CONCURRENCY);
        while let Some(page) = rest.next().await {
            let (items, _) = list_of(page?);
            if progressive {
                let _ = updates.send(Ok(ListUpdate::Append(parse_items(&items))));
            }
            all.extend(items);
        }
        if !progressive {
            let _ = updates.send(Ok(ListUpdate::Replace(parse_items(&all))));
        }
        let _ = updates.send(Ok(ListUpdate::Finished));

        if let Some(cache) = cache
            && !self.is_signed_out()
        {
            let data = serde_json::to_vec(&serde_json::json!({
                "totalNumberOfItems": total,
                "items": all,
            }))?;
            if let Err(err) = cache.store_response(user_id, key, data).await {
                log::warn!("could not cache {path}: {err:#}");
            }
        }
        Ok(())
    }

    async fn favorite_list_page(&self, path: &str, offset: usize) -> Result<Value> {
        self.get(
            tidal::Call::get(tidal::Base::V1, path)
                .page(offset, FAVORITES_PAGE)
                .query("order", "DATE")
                .query("orderDirection", "DESC"),
        )
        .await
    }
}

/// What [`Api::sync_favorite_list`] reports.
pub enum ListUpdate<T> {
    /// The list as cached last time, to show before TIDAL answers.
    Cached(Vec<T>),
    /// More items for the end of the list (nothing was cached).
    Append(Vec<T>),
    /// The cached list was stale: this is the whole current list.
    Replace(Vec<T>),
    /// The list on screen is now current.
    Finished,
}

/// The wrapped items (`{"created", "item"}`) and total of a v1 page.
fn list_of(mut value: Value) -> (Vec<Value>, usize) {
    let total = value
        .get("totalNumberOfItems")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let items = match value.get_mut("items").map(Value::take) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    (items, total)
}

fn item_id(wrapped: &Value) -> Option<&Value> {
    wrapped.get("item")?.get("id")
}

/// The items the models accept (others are skipped).
fn parse_items<T: DeserializeOwned>(items: &[Value]) -> Vec<T> {
    items
        .iter()
        .filter_map(|wrapped| T::deserialize(wrapped.get("item")?).ok())
        .collect()
}

/// TIDAL answers 50 favourites by default and accepts up to 100 per request.
const FAVORITES_PAGE: usize = 100;
const FAVORITES_CONCURRENCY: usize = 4;
/// A cached list older than this is reloaded even if its head still matches,
/// to pick up tracks that became unavailable or changed metadata.
const FAVORITES_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub enum FavoritesUpdate {
    /// More tracks for the end of the list.
    Append(Vec<TrackInfo>),
    /// The cached list was stale: this is the whole current list.
    Replace(Vec<TrackInfo>),
    /// The list on screen is now current.
    Finished,
}

/// What a cached list looked like, to check it against the first page.
pub struct CachedHead {
    ids: Vec<u64>,
    total: usize,
    age: Duration,
}

impl CachedHead {
    pub fn of(cached: &CachedCollection) -> Self {
        Self {
            ids: cached
                .tracks
                .iter()
                .take(FAVORITES_PAGE)
                .map(|track| track.id)
                .collect(),
            total: cached.total,
            age: cached.age,
        }
    }

    fn matches(&self, total: usize, first_page: &[TrackInfo]) -> bool {
        self.age < FAVORITES_MAX_AGE
            && self.total == total
            && first_page.len() <= self.ids.len()
            && first_page
                .iter()
                .zip(&self.ids)
                .all(|(track, id)| track.id == *id)
    }
}

fn into_infos(items: Vec<tidal::models::Track>) -> Vec<TrackInfo> {
    items.into_iter().map(TrackInfo::from).collect()
}
