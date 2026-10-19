//! Swell's own client for TIDAL's (unofficial) v1 and v2 APIs.
//!
//! - One shared `reqwest::Client` (HTTP/2, kept alive) for every call.
//! - The access token is refreshed shortly before it expires, so requests
//!   rarely pay for a 401 round trip; a 401 still triggers one refresh
//!   (shared by every request that hits it) and one retry.
//! - Idempotent requests are retried on 429/5xx and dropped connections with
//!   a short backoff; writes are never retried, so they cannot apply twice.
//! - Models are lenient: missing fields take defaults, and one malformed
//!   item drops only that item, never the whole page.

pub mod auth;
mod endpoints;
pub mod models;
pub mod pages;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

pub use auth::{Authz, DeviceCode, Pkce};
pub use endpoints::*;

use crate::session::LoginFlow;

const API_V1: &str = "https://api.tidal.com/v1/";
const API_V2: &str = "https://api.tidal.com/v2/";
/// Sent as `x-tidal-client-version`; v2 endpoints reject requests without it.
const CLIENT_VERSION: &str = "2025.7.16";
/// Refresh this long before the token expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(120);
const MAX_RETRIES: u32 = 3;

/// Errors with a meaning for the UI. Everything else is a plain
/// `anyhow::Error` with a readable message.
#[derive(Debug)]
pub enum Error {
    /// The refresh token was rejected: the user has to sign in again.
    SessionExpired,
    /// TIDAL answered with an error.
    Api {
        status: u16,
        sub_status: u32,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionExpired => f.write_str("the TIDAL session has expired"),
            Self::Api {
                status,
                sub_status,
                message,
            } => write!(f, "TIDAL error {status}/{sub_status}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Whether `err` means the user has to sign in again.
pub fn is_auth_error(err: &anyhow::Error) -> bool {
    match err.downcast_ref::<Error>() {
        Some(Error::SessionExpired) => true,
        // 11xxx is TIDAL's "token invalid"; other 401s are e.g. a track not
        // playable in this region.
        Some(Error::Api {
            status: 401,
            sub_status,
            ..
        }) => (11000..12000).contains(sub_status),
        _ => false,
    }
}

/// Whether `err` is TIDAL saying the item does not exist (or not here).
pub fn is_not_found(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<Error>(),
        Some(Error::Api { status: 404, .. })
    )
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ErrorBody {
    status: Option<u16>,
    sub_status: Option<u32>,
    user_message: Option<String>,
    description: Option<String>,
    error: Option<String>,
}

fn api_error(status: StatusCode, body: &[u8]) -> Error {
    let parsed: ErrorBody = serde_json::from_slice(body).unwrap_or_default();
    Error::Api {
        status: parsed.status.unwrap_or(status.as_u16()),
        sub_status: parsed.sub_status.unwrap_or(0),
        message: parsed
            .user_message
            .or(parsed.description)
            .or(parsed.error)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_owned()),
    }
}

#[derive(Clone, Copy)]
pub enum Base {
    V1,
    V2,
}

impl Base {
    /// Short tag for cache keys.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::V1 => "v1/",
            Self::V2 => "v2/",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Self::V1 => API_V1,
            Self::V2 => API_V2,
        }
    }
}

/// One API call, described so it can be re-sent after a refresh.
pub struct Call {
    pub method: Method,
    pub base: Base,
    pub path: String,
    pub query: Vec<(&'static str, String)>,
    pub form: Option<Vec<(&'static str, String)>>,
    pub headers: Vec<(&'static str, String)>,
}

impl Call {
    pub fn get(base: Base, path: impl Into<String>) -> Self {
        Self {
            method: Method::GET,
            base,
            path: path.into(),
            query: Vec::new(),
            form: None,
            headers: Vec::new(),
        }
    }

    pub fn post(base: Base, path: impl Into<String>) -> Self {
        Self {
            method: Method::POST,
            ..Self::get(base, path)
        }
    }

    pub fn delete(base: Base, path: impl Into<String>) -> Self {
        Self {
            method: Method::DELETE,
            ..Self::get(base, path)
        }
    }

    pub fn put(base: Base, path: impl Into<String>) -> Self {
        Self {
            method: Method::PUT,
            ..Self::get(base, path)
        }
    }

    /// Adds `offset` and `limit`.
    pub fn page(self, offset: usize, limit: usize) -> Self {
        self.query("offset", offset).query("limit", limit)
    }

    pub fn query(mut self, key: &'static str, value: impl ToString) -> Self {
        self.query.push((key, value.to_string()));
        self
    }

    pub fn form(mut self, key: &'static str, value: impl ToString) -> Self {
        self.form
            .get_or_insert_with(Vec::new)
            .push((key, value.to_string()));
        self
    }

    pub fn header(mut self, key: &'static str, value: impl ToString) -> Self {
        self.headers.push((key, value.to_string()));
        self
    }

    fn idempotent(&self) -> bool {
        self.method == Method::GET
    }
}

pub struct Response {
    pub status: StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: bytes::Bytes,
}

impl Response {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).with_context(|| {
            format!(
                "unexpected answer from TIDAL: {}",
                String::from_utf8_lossy(&self.body[..self.body.len().min(200)])
            )
        })
    }

    pub fn etag(&self) -> Option<String> {
        self.headers
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }
}

type RefreshCallback = Box<dyn Fn(Authz) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct Client {
    http: reqwest::Client,
    pub flow: LoginFlow,
    authz: RwLock<Arc<Authz>>,
    /// Held while tokens are refreshed and saved, and by sign-out while it
    /// clears them, so a refresh can never write tokens back afterwards.
    refreshing: tokio::sync::Mutex<()>,
    on_refresh: RefreshCallback,
    /// Set on sign-out: nothing may be written back afterwards.
    signed_out: AtomicBool,
    locale: String,
}

impl Client {
    /// `on_refresh` persists refreshed tokens; it is awaited before the
    /// refresh completes (and never runs after sign-out has begun).
    pub fn new<F, Fut>(http: reqwest::Client, flow: LoginFlow, authz: Authz, on_refresh: F) -> Self
    where
        F: Fn(Authz) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self {
            http,
            flow,
            authz: RwLock::new(Arc::new(authz)),
            refreshing: tokio::sync::Mutex::new(()),
            on_refresh: Box::new(move |authz| Box::pin(on_refresh(authz))),
            signed_out: AtomicBool::new(false),
            locale: locale(),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn authz(&self) -> Arc<Authz> {
        self.authz.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The signed-in user's id, or 0 if TIDAL never said (only fit for
    /// paths and cache keys; use [`Self::known_user_id`] to compare).
    pub fn user_id(&self) -> u64 {
        self.authz().user_id.unwrap_or(0)
    }

    /// The signed-in user's id, if known. Never 0, which editorial
    /// playlists use as their creator.
    pub fn known_user_id(&self) -> Option<u64> {
        self.authz().user_id.filter(|&id| id != 0)
    }

    /// The locale sent with every request (`es_ES`...).
    pub fn locale(&self) -> &str {
        &self.locale
    }

    pub fn country_code(&self) -> String {
        self.authz()
            .country_code
            .clone()
            .unwrap_or_else(|| "US".to_owned())
    }

    pub fn mark_signed_out(&self) {
        self.signed_out.store(true, Ordering::SeqCst);
    }

    pub fn is_signed_out(&self) -> bool {
        self.signed_out.load(Ordering::SeqCst)
    }

    /// Marks the session signed out and waits for a refresh in flight to
    /// finish saving its tokens. Clear the stored tokens while holding the
    /// returned guard: no refresh can store them again afterwards.
    pub async fn begin_sign_out(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mark_signed_out();
        self.refreshing.lock().await
    }

    /// Refreshes the token unless another request already did since `stale`
    /// was read.
    async fn refresh(&self, stale: &Arc<Authz>) -> Result<()> {
        let _guard = self.refreshing.lock().await;
        if self.is_signed_out() {
            anyhow::bail!("signed out");
        }
        if !Arc::ptr_eq(&self.authz(), stale) {
            return Ok(());
        }
        let fresh = auth::refresh(&self.http, self.flow.oauth_client(), stale).await?;
        log::info!("access token refreshed");
        *self.authz.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(fresh.clone());
        // Saved under the lock: sign-out waits for it before clearing.
        if !self.is_signed_out() {
            (self.on_refresh)(fresh).await;
        }
        Ok(())
    }

    /// Sends `call`, refreshing and retrying as described in the module docs.
    pub async fn send(&self, call: &Call) -> Result<Response> {
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            let authz = self.authz();
            if authz.expires_within(REFRESH_MARGIN) && !refreshed {
                refreshed = true;
                // The token still has a little life left: a hiccup of the
                // auth server must not fail the request.
                if let Err(err) = self.refresh(&authz).await {
                    let fatal = self.is_signed_out()
                        || matches!(err.downcast_ref::<Error>(), Some(Error::SessionExpired))
                        || authz.expires_within(Duration::ZERO);
                    if fatal {
                        return Err(err);
                    }
                    log::warn!("proactive token refresh failed, using the current token: {err:#}");
                }
                continue;
            }
            let url = format!("{}{}", call.base.url(), call.path);
            let mut request = self
                .http
                .request(call.method.clone(), &url)
                .bearer_auth(&authz.access_token)
                .header("x-tidal-client-version", CLIENT_VERSION)
                .query(&[
                    ("countryCode", authz.country_code.as_deref().unwrap_or("US")),
                    ("locale", &self.locale),
                    ("deviceType", "BROWSER"),
                ])
                .query(&call.query);
            for (key, value) in &call.headers {
                request = request.header(*key, value);
            }
            if let Some(form) = &call.form {
                request = request.form(form);
            }

            let retry = attempt < MAX_RETRIES && call.idempotent();
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) if retry && (err.is_connect() || err.is_timeout() || err.is_request()) => {
                    attempt += 1;
                    log::debug!("{} {url}: {err}; retrying", call.method);
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(err) => return Err(anyhow::Error::new(err).context("could not reach TIDAL")),
            };
            let status = response.status();
            let headers = response.headers().clone();
            let body = response
                .bytes()
                .await
                .context("TIDAL's answer was cut off")?;

            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                return Ok(Response {
                    status,
                    headers,
                    body,
                });
            }
            let error = api_error(status, &body);
            if status == StatusCode::UNAUTHORIZED
                && !refreshed
                && matches!(&error, Error::Api { sub_status, .. } if (11000..12000).contains(sub_status))
            {
                refreshed = true;
                self.refresh(&authz).await?;
                continue;
            }
            if retry && (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) {
                attempt += 1;
                let wait = headers
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|s| Duration::from_secs(s.min(10)))
                    .unwrap_or_else(|| backoff(attempt));
                log::debug!("{} {url}: {status}; retrying in {wait:?}", call.method);
                tokio::time::sleep(wait).await;
                continue;
            }
            log::debug!("{} {url}: {error}", call.method);
            return Err(error.into());
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, call: Call) -> Result<T> {
        self.send(&call).await?.json()
    }

    /// Sends a write and ignores the body.
    pub async fn write(&self, call: Call) -> Result<Response> {
        self.send(&call).await
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250 * 2u64.pow(attempt.saturating_sub(1)))
}

/// TIDAL's locale (`es_CL`, `en_US`...): the UI's language, so content
/// and interface never mix languages.
fn locale() -> String {
    crate::i18n::tidal_locale()
}
