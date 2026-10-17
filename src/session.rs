//! The signed-in session, persisted in the desktop keyring.
//!
//! Tokens go to the Secret Service (KWallet, GNOME Keyring, KeePassXC…) or,
//! inside Flatpak, to the secret portal. They are never written to disk in
//! plain text.

use crate::tidal::{Authz, Quality};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::config::{self, OAuthClient};

/// How the user signed in. It decides the OAuth client used to refresh the
/// tokens, and with it the best quality TIDAL will stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginFlow {
    /// Browser login with PKCE: Hi-Res Lossless.
    Pkce,
    /// Device-code login (enter a code at link.tidal.com): Lossless at most.
    DeviceCode,
}

impl LoginFlow {
    pub fn oauth_client(self) -> &'static OAuthClient {
        match self {
            Self::Pkce => &config::PKCE_CLIENT,
            Self::DeviceCode => &config::DEVICE_CLIENT,
        }
    }

    pub fn max_quality(self) -> Quality {
        match self {
            Self::Pkce => Quality::HiResLossless,
            Self::DeviceCode => Quality::Lossless,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredSession {
    pub flow: LoginFlow,
    pub authz: Authz,
}

const LABEL: &str = "Swell: TIDAL session";
const ATTRIBUTES: [(&str, &str); 2] = [("application", config::APP_ID), ("kind", "tidal-session")];

async fn keyring() -> Result<&'static oo7::Keyring> {
    static KEYRING: OnceCell<oo7::Keyring> = OnceCell::const_new();
    KEYRING
        .get_or_try_init(|| async {
            let keyring = oo7::Keyring::new()
                .await
                .context("no Secret Service available")?;
            keyring
                .unlock()
                .await
                .context("the keyring could not be unlocked")?;
            Ok(keyring)
        })
        .await
}

pub async fn load() -> Result<Option<StoredSession>> {
    let items = keyring().await?.search_items(&ATTRIBUTES).await?;
    let Some(item) = items.first() else {
        return Ok(None);
    };
    if item.is_locked().await? {
        item.unlock().await?;
    }
    let secret = item.secret().await?;
    match serde_json::from_slice(&secret) {
        Ok(session) => Ok(Some(session)),
        Err(err) => {
            log::warn!("ignoring an unreadable stored session: {err}");
            Ok(None)
        }
    }
}

pub async fn save(session: &StoredSession) -> Result<()> {
    let secret = serde_json::to_vec(session)?;
    keyring()
        .await?
        .create_item(LABEL, &ATTRIBUTES, secret, true)
        .await?;
    Ok(())
}

pub async fn clear() -> Result<()> {
    keyring().await?.delete(&ATTRIBUTES).await?;
    Ok(())
}
