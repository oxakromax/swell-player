//! Thin layer over the `tidalrs` client: building it for a login flow, the
//! sign-in steps, and keeping the keyring in sync when tokens are refreshed.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tidalrs::{DeviceAuthorizationResponse, TidalClient};

use crate::config;
use crate::session::{self, LoginFlow, StoredSession};

/// One HTTP client for the whole app: API calls and image downloads share
/// its keep-alive connection pool.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(4)
        .build()
        .expect("failed to build the HTTP client")
}

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
        session::save(&StoredSession { flow: self.flow, authz: (*authz).clone() }).await
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

    pub async fn favorite_track_count(&self) -> Result<usize> {
        let page = self.client.favorite_tracks(Some(0), Some(1), None, None).await?;
        Ok(page.total)
    }
}
