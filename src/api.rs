//! Thin layer over the `tidalrs` client: building it for a login flow, the
//! sign-in steps, and keeping the keyring in sync when tokens are refreshed.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{StreamExt, stream};
use tidalrs::{DeviceAuthorizationResponse, FavoriteTrack, TidalClient};
use tokio::sync::mpsc::UnboundedSender;

use crate::config;
use crate::model::TrackInfo;
use crate::session::{self, LoginFlow, StoredSession};

/// One HTTP client for the whole app: API calls and image downloads share
/// its connection pool. Both TIDAL hosts speak HTTP/2, so concurrent
/// requests are multiplexed over one kept-alive connection per host.
///
/// Names are resolved with hickory instead of glibc's `getaddrinfo`, which
/// on some networks stalls for its 5 s timeout when one of the parallel
/// A/AAAA answers is lost.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
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
    pub client: TidalClient,
    pub flow: LoginFlow,
}

impl Api {
    pub fn new(flow: LoginFlow, http: reqwest::Client) -> Arc<Self> {
        Self::build(flow, http, None)
    }

    pub fn restore(stored: StoredSession, http: reqwest::Client) -> Arc<Self> {
        Self::build(stored.flow, http, Some(stored.authz))
    }

    fn build(flow: LoginFlow, http: reqwest::Client, authz: Option<tidalrs::Authz>) -> Arc<Self> {
        let oauth = flow.oauth_client();
        let mut client = TidalClient::new(oauth.id.to_owned())
            .with_client_secret(oauth.secret)
            .with_client(http)
            .with_authz_refresh_callback(move |authz| {
                // Called on the I/O runtime right after a refresh: persist the
                // new tokens so the next start does not need to refresh again.
                tokio::spawn(async move {
                    if let Err(err) = session::save(&StoredSession { flow, authz }).await {
                        log::warn!("could not store refreshed tokens: {err:#}");
                    }
                });
            });
        if let Some(authz) = authz {
            client = client.with_authz(authz);
        }
        Arc::new(Self { client, flow })
    }

    /// Persists the tokens the client currently holds.
    async fn store(&self) -> Result<()> {
        let authz = self.client.get_authz().context("not signed in")?;
        session::save(&StoredSession {
            flow: self.flow,
            authz: (*authz).clone(),
        })
        .await
    }

    /// First half of the PKCE login: the URL to open in the browser.
    pub fn pkce_login_url(&self) -> Result<String> {
        Ok(self.client.start_pkce(config::PKCE_REDIRECT_URI)?)
    }

    /// Second half of the PKCE login: `redirect_url` is the address of the
    /// page TIDAL lands on after signing in; it carries the one-time code.
    pub async fn finish_pkce_login(&self, redirect_url: &str) -> Result<()> {
        self.client.finish_pkce(redirect_url.trim()).await?;
        self.store().await
    }

    pub async fn start_device_login(&self) -> Result<DeviceAuthorizationResponse> {
        Ok(self.client.device_authorization().await?)
    }

    /// Polls until the user approves the code or it expires.
    pub async fn finish_device_login(&self, device: &DeviceAuthorizationResponse) -> Result<()> {
        self.client
            .wait_for_authorization(&device.device_code, device.expires_in, device.interval)
            .await?;
        self.store().await
    }

    pub async fn sign_out(&self) -> Result<()> {
        session::clear().await
    }

    /// Fetches every favourite track, newest first, and sends them to `pages`
    /// one page at a time and in order, so the list can show the first page
    /// while the rest is still loading.
    ///
    /// The first request reveals the total; the remaining pages are then
    /// fetched with at most [`FAVORITES_CONCURRENCY`] requests in flight.
    pub async fn load_favorite_tracks(
        self: Arc<Self>,
        pages: UnboundedSender<Result<Vec<TrackInfo>>>,
    ) {
        let first = match self.favorite_tracks_page(0).await {
            Ok(first) => first,
            Err(err) => {
                let _ = pages.send(Err(err));
                return;
            }
        };
        let total = first.total;
        if pages.send(Ok(into_infos(first.items))).is_err() {
            return;
        }

        let mut rest = stream::iter((FAVORITES_PAGE..total).step_by(FAVORITES_PAGE))
            .map(|offset| self.favorite_tracks_page(offset))
            .buffered(FAVORITES_CONCURRENCY);
        while let Some(page) = rest.next().await {
            let page = page.map(|page| into_infos(page.items));
            let failed = page.is_err();
            if pages.send(page).is_err() || failed {
                return;
            }
        }
    }

    async fn favorite_tracks_page(&self, offset: usize) -> Result<tidalrs::List<FavoriteTrack>> {
        let offset = offset.try_into()?;
        let limit = FAVORITES_PAGE.try_into()?;
        let page = self
            .client
            .favorite_tracks(Some(offset), Some(limit), None, None)
            .await?;
        log::debug!(
            "favourites at offset {offset}: {} items of {}",
            page.items.len(),
            page.total
        );
        Ok(page)
    }
}

/// TIDAL answers 50 favourites by default and accepts up to 100 per request.
const FAVORITES_PAGE: usize = 100;
const FAVORITES_CONCURRENCY: usize = 4;

fn into_infos(items: Vec<FavoriteTrack>) -> Vec<TrackInfo> {
    items
        .into_iter()
        .map(|favorite| favorite.item.into())
        .collect()
}
