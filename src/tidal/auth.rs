//! TIDAL's OAuth endpoints: the PKCE browser login, the device-code login
//! and token refresh.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::OAuthClient;

use super::Error;

const AUTHORIZE_URL: &str = "https://login.tidal.com/authorize";
const TOKEN_URL: &str = "https://auth.tidal.com/v1/oauth2/token";
const DEVICE_URL: &str = "https://auth.tidal.com/v1/oauth2/device_authorization";
const SCOPE: &str = "r_usr w_usr";

/// The tokens of a signed-in session, as stored in the keyring. The field
/// names match what earlier versions stored, so existing sessions load.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Authz {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub user_id: Option<u64>,
    #[serde(default)]
    pub country_code: Option<String>,
    /// Unix time at which the access token expires.
    #[serde(default)]
    pub expires_timestamp: Option<u64>,
}

impl Authz {
    /// Whether the access token expires within `margin`.
    pub fn expires_within(&self, margin: Duration) -> bool {
        self.expires_timestamp
            .is_some_and(|at| now() + margin.as_secs() >= at)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    user_id: Option<u64>,
    #[serde(default)]
    user: Option<TokenUser>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenUser {
    #[serde(default)]
    country_code: Option<String>,
}

#[derive(Deserialize)]
struct OAuthError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

impl TokenResponse {
    fn into_authz(self, previous: Option<&Authz>) -> Authz {
        Authz {
            access_token: self.access_token,
            refresh_token: self
                .refresh_token
                .or_else(|| previous.and_then(|p| p.refresh_token.clone())),
            user_id: self.user_id.or_else(|| previous.and_then(|p| p.user_id)),
            country_code: self
                .user
                .and_then(|u| u.country_code)
                .or_else(|| previous.and_then(|p| p.country_code.clone())),
            expires_timestamp: self.expires_in.map(|secs| now() + secs),
        }
    }
}

/// Posts to the token endpoint. Returns the raw status and body so callers
/// can tell OAuth errors apart.
async fn post_token(
    http: &reqwest::Client,
    client: &OAuthClient,
    form: &[(&str, &str)],
) -> Result<(reqwest::StatusCode, bytes::Bytes)> {
    let response = http
        .post(TOKEN_URL)
        .basic_auth(client.id, Some(client.secret))
        .form(form)
        .send()
        .await
        .context("could not reach TIDAL's sign-in server")?;
    let status = response.status();
    Ok((status, response.bytes().await?))
}

fn oauth_error(status: reqwest::StatusCode, body: &[u8]) -> anyhow::Error {
    match serde_json::from_slice::<OAuthError>(body) {
        Ok(err) if !err.error.is_empty() => anyhow!(
            "{} ({})",
            err.error_description.unwrap_or_else(|| err.error.clone()),
            status
        ),
        _ => anyhow!("sign-in failed with HTTP {status}"),
    }
}

/// Exchanges the refresh token for a new access token. A rejected refresh
/// token means the session is over: [`Error::SessionExpired`].
pub async fn refresh(http: &reqwest::Client, client: &OAuthClient, authz: &Authz) -> Result<Authz> {
    let refresh_token = authz
        .refresh_token
        .as_deref()
        .ok_or(Error::SessionExpired)?;
    let (status, body) = post_token(
        http,
        client,
        &[
            ("client_id", client.id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
            ("scope", SCOPE),
        ],
    )
    .await?;
    if status.is_success() {
        let token: TokenResponse = serde_json::from_slice(&body)?;
        return Ok(token.into_authz(Some(authz)));
    }
    if matches!(status.as_u16(), 400 | 401 | 403) {
        log::warn!("token refresh rejected: {:#}", oauth_error(status, &body));
        return Err(Error::SessionExpired.into());
    }
    Err(oauth_error(status, &body))
}

/// A browser sign-in in progress.
pub struct Pkce {
    pub url: String,
    verifier: String,
    unique_key: String,
    redirect_uri: String,
}

impl Pkce {
    pub fn start(client: &OAuthClient, redirect_uri: &str) -> Result<Self> {
        let mut random = [0u8; 40];
        getrandom::fill(&mut random).map_err(|err| anyhow!("no randomness: {err}"))?;
        let verifier = URL_SAFE_NO_PAD.encode(&random[..32]);
        let unique_key = random[32..]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = url::Url::parse(AUTHORIZE_URL)?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("client_id", client.id)
            .append_pair("lang", "EN")
            .append_pair("appMode", "android")
            .append_pair("client_unique_key", &unique_key)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("restrict_signup", "true");
        Ok(Self {
            url: url.into(),
            verifier,
            unique_key,
            redirect_uri: redirect_uri.to_owned(),
        })
    }

    /// `redirect_url` is the address the browser landed on after signing in.
    pub async fn finish(
        &self,
        http: &reqwest::Client,
        client: &OAuthClient,
        redirect_url: &str,
    ) -> Result<Authz> {
        let url = url::Url::parse(redirect_url.trim())
            .context("that is not the address of the page TIDAL opened")?;
        let code = url
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned())
            .context("the address has no sign-in code; copy it after signing in")?;
        let (status, body) = post_token(
            http,
            client,
            &[
                ("code", &code),
                ("client_id", client.id),
                ("grant_type", "authorization_code"),
                ("redirect_uri", &self.redirect_uri),
                ("scope", SCOPE),
                ("code_verifier", &self.verifier),
                ("client_unique_key", &self.unique_key),
            ],
        )
        .await?;
        if !status.is_success() {
            return Err(oauth_error(status, &body));
        }
        let token: TokenResponse = serde_json::from_slice(&body)?;
        Ok(token.into_authz(None))
    }
}

/// A device-code sign-in in progress.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    #[serde(rename = "verificationUriComplete")]
    pub url: String,
    pub device_code: String,
    pub user_code: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

impl DeviceCode {
    pub async fn start(http: &reqwest::Client, client: &OAuthClient) -> Result<Self> {
        let response = http
            .post(DEVICE_URL)
            .form(&[("client_id", client.id), ("scope", SCOPE)])
            .send()
            .await
            .context("could not reach TIDAL's sign-in server")?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(oauth_error(status, &body));
        }
        let mut device: Self = serde_json::from_slice(&body)?;
        if !device.url.starts_with("http") {
            device.url = format!("https://{}", device.url);
        }
        Ok(device)
    }

    /// Polls until the user approves the code, or fails once it expires.
    pub async fn wait(&self, http: &reqwest::Client, client: &OAuthClient) -> Result<Authz> {
        let mut interval = self.interval.max(1);
        let deadline = now() + self.expires_in;
        while now() < deadline {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            let (status, body) = post_token(
                http,
                client,
                &[
                    ("client_id", client.id),
                    ("client_secret", client.secret),
                    ("device_code", &self.device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("scope", SCOPE),
                ],
            )
            .await?;
            if status.is_success() {
                let token: TokenResponse = serde_json::from_slice(&body)?;
                return Ok(token.into_authz(None));
            }
            match serde_json::from_slice::<OAuthError>(&body) {
                Ok(err) if err.error == "authorization_pending" => {}
                Ok(err) if err.error == "slow_down" => interval += 5,
                _ => return Err(oauth_error(status, &body)),
            }
        }
        bail!("the code expired before it was approved")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_url_has_challenge() {
        let pkce = Pkce::start(&crate::config::PKCE_CLIENT, "https://example.com/cb").unwrap();
        assert!(pkce.url.starts_with(AUTHORIZE_URL));
        assert!(pkce.url.contains("code_challenge_method=S256"));
        assert_eq!(pkce.verifier.len(), 43);
        assert_eq!(pkce.unique_key.len(), 16);
    }

    #[test]
    fn stored_session_format_still_loads() {
        let authz: Authz = serde_json::from_str(
            r#"{"access_token":"a","refresh_token":"r","user_id":1,"country_code":"CL","expires_timestamp":5}"#,
        )
        .unwrap();
        assert_eq!(authz.user_id, Some(1));
        assert!(authz.expires_within(Duration::ZERO));
    }
}
