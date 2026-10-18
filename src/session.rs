//! The signed-in session, persisted in the system's secret store.
//!
//! On Linux and the BSDs, tokens go to the Secret Service (KWallet, GNOME
//! Keyring, KeePassXC…) or, inside Flatpak, to the secret portal (oo7). On
//! Windows they go to the Credential Manager and on macOS to the login
//! Keychain (the `keyring` crate). They are never written to disk in plain
//! text.

use crate::tidal::{Authz, Quality};
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::{self, OAuthClient};
use crate::i18n::tr;

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

pub async fn load() -> Result<Option<StoredSession>> {
    let Some(secret) = store::load().await? else {
        return Ok(None);
    };
    match serde_json::from_slice(&secret) {
        Ok(session) => Ok(Some(session)),
        Err(err) => {
            log::warn!("ignoring an unreadable stored session: {err}");
            Ok(None)
        }
    }
}

pub async fn save(session: &StoredSession) -> Result<()> {
    store::save(serde_json::to_vec(session)?).await
}

pub async fn clear() -> Result<()> {
    store::clear().await
}

/// Shown when the secret store cannot be opened at startup.
pub fn open_error() -> &'static str {
    if cfg!(any(windows, target_os = "macos")) {
        tr("Couldn't open the system's credential store.")
    } else {
        tr(
            "Couldn't open the keyring. Make sure a keyring service (GNOME Keyring, KWallet) is running.",
        )
    }
}

/// Shown when signing in worked but the tokens could not be stored.
pub fn save_error() -> &'static str {
    if cfg!(any(windows, target_os = "macos")) {
        tr("Couldn't save the session in the system's credential store.")
    } else {
        tr(
            "Couldn't save the session in the keyring. Make sure a keyring \
            service (GNOME Keyring, KWallet) is running.",
        )
    }
}

/// The Secret Service, through oo7.
#[cfg(all(unix, not(target_os = "macos")))]
mod store {
    use anyhow::{Context, Result};
    use tokio::sync::OnceCell;

    use crate::config;

    const LABEL: &str = "Swell: TIDAL session";
    const ATTRIBUTES: [(&str, &str); 2] =
        [("application", config::APP_ID), ("kind", "tidal-session")];

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

    pub async fn load() -> Result<Option<Vec<u8>>> {
        let items = keyring().await?.search_items(&ATTRIBUTES).await?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        if item.is_locked().await? {
            item.unlock().await?;
        }
        Ok(Some(item.secret().await?.to_vec()))
    }

    pub async fn save(secret: Vec<u8>) -> Result<()> {
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
}

/// The Windows Credential Manager or the macOS Keychain, through the
/// `keyring` crate (blocking calls, run on Tokio's blocking pool).
#[cfg(any(windows, target_os = "macos"))]
mod store {
    use anyhow::{Result, anyhow};

    use super::parts::{self, Parts};
    use crate::config;

    struct Keyring;

    fn entry(part: usize) -> Result<keyring::Entry> {
        keyring::Entry::new(config::APP_ID, &parts::name("tidal-session", part))
            .map_err(|err| anyhow!("the credential store is unavailable: {err}"))
    }

    impl Parts for Keyring {
        fn get(&mut self, part: usize) -> Result<Option<Vec<u8>>> {
            match entry(part)?.get_secret() {
                Ok(secret) => Ok(Some(secret)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(anyhow!("could not read a credential: {err}")),
            }
        }

        fn set(&mut self, part: usize, data: &[u8]) -> Result<()> {
            entry(part)?
                .set_secret(data)
                .map_err(|err| anyhow!("could not store a credential: {err}"))
        }

        fn delete(&mut self, part: usize) -> Result<()> {
            match entry(part)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(anyhow!("could not delete a credential: {err}")),
            }
        }
    }

    async fn blocking<T: Send + 'static>(
        work: impl FnOnce(&mut Keyring) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        tokio::task::spawn_blocking(move || work(&mut Keyring)).await?
    }

    pub async fn load() -> Result<Option<Vec<u8>>> {
        blocking(|store| parts::read(store)).await
    }

    pub async fn save(secret: Vec<u8>) -> Result<()> {
        blocking(move |store| parts::write(store, &secret)).await
    }

    pub async fn clear() -> Result<()> {
        blocking(|store| parts::clear(store)).await
    }
}

/// A secret split over several store entries: the Windows Credential
/// Manager takes at most 2560 bytes per credential, and a TIDAL session
/// (two JWTs) can come close. Entry 0 holds the whole secret when it fits,
/// or `parts:N` with the secret in entries 1 to N.
#[cfg(any(windows, target_os = "macos", test))]
mod parts {
    use anyhow::Result;

    /// Bytes per entry, well under the Credential Manager's limit.
    const PART_SIZE: usize = 2048;
    const HEADER: &[u8] = b"parts:";

    pub trait Parts {
        fn get(&mut self, part: usize) -> Result<Option<Vec<u8>>>;
        fn set(&mut self, part: usize, data: &[u8]) -> Result<()>;
        /// Deleting an entry that does not exist is not an error.
        fn delete(&mut self, part: usize) -> Result<()>;
    }

    /// The store name of entry `part`.
    pub fn name(base: &str, part: usize) -> String {
        if part == 0 {
            base.to_owned()
        } else {
            format!("{base}.{part}")
        }
    }

    /// The number of extra entries entry 0 announces.
    fn count(first: &[u8]) -> Option<usize> {
        std::str::from_utf8(first.strip_prefix(HEADER)?)
            .ok()?
            .parse()
            .ok()
    }

    pub fn read(store: &mut impl Parts) -> Result<Option<Vec<u8>>> {
        let Some(first) = store.get(0)? else {
            return Ok(None);
        };
        let Some(count) = count(&first) else {
            return Ok(Some(first));
        };
        let mut secret = Vec::new();
        for part in 1..=count {
            let Some(data) = store.get(part)? else {
                log::warn!("the stored session is incomplete (part {part} of {count} is missing)");
                return Ok(None);
            };
            secret.extend_from_slice(&data);
        }
        Ok(Some(secret))
    }

    pub fn write(store: &mut impl Parts, secret: &[u8]) -> Result<()> {
        let old = store.get(0)?.as_deref().and_then(count).unwrap_or(0);
        let new = if secret.len() <= PART_SIZE {
            store.set(0, secret)?;
            0
        } else {
            let chunks: Vec<&[u8]> = secret.chunks(PART_SIZE).collect();
            for (index, chunk) in chunks.iter().enumerate() {
                store.set(index + 1, chunk)?;
            }
            store.set(0, format!("parts:{}", chunks.len()).as_bytes())?;
            chunks.len()
        };
        for part in new + 1..=old {
            store.delete(part)?;
        }
        Ok(())
    }

    pub fn clear(store: &mut impl Parts) -> Result<()> {
        let count = store.get(0)?.as_deref().and_then(count).unwrap_or(0);
        for part in 1..=count {
            store.delete(part)?;
        }
        store.delete(0)
    }

    #[cfg(test)]
    mod tests {
        use std::collections::BTreeMap;

        use super::*;

        #[derive(Default)]
        struct Memory(BTreeMap<usize, Vec<u8>>);

        impl Parts for Memory {
            fn get(&mut self, part: usize) -> Result<Option<Vec<u8>>> {
                Ok(self.0.get(&part).cloned())
            }
            fn set(&mut self, part: usize, data: &[u8]) -> Result<()> {
                assert!(data.len() <= 2560, "over the Credential Manager's limit");
                self.0.insert(part, data.to_vec());
                Ok(())
            }
            fn delete(&mut self, part: usize) -> Result<()> {
                self.0.remove(&part);
                Ok(())
            }
        }

        #[test]
        fn small_secrets_use_one_entry() {
            let mut store = Memory::default();
            assert_eq!(read(&mut store).unwrap(), None);
            write(&mut store, b"{\"short\":1}").unwrap();
            assert_eq!(store.0.len(), 1);
            assert_eq!(read(&mut store).unwrap().unwrap(), b"{\"short\":1}");
            clear(&mut store).unwrap();
            assert!(store.0.is_empty());
        }

        #[test]
        fn long_secrets_are_split_and_shrink_cleanly() {
            let mut store = Memory::default();
            let long: Vec<u8> = (0..5000).map(|i| b'a' + (i % 26) as u8).collect();
            write(&mut store, &long).unwrap();
            assert_eq!(store.0.len(), 4, "header and three parts");
            assert_eq!(read(&mut store).unwrap().unwrap(), long);

            let medium = &long[..3000];
            write(&mut store, medium).unwrap();
            assert_eq!(store.0.len(), 3, "the third part is gone");
            assert_eq!(read(&mut store).unwrap().unwrap(), medium);

            write(&mut store, b"{}").unwrap();
            assert_eq!(store.0.len(), 1);
            assert_eq!(read(&mut store).unwrap().unwrap(), b"{}");

            write(&mut store, &long).unwrap();
            clear(&mut store).unwrap();
            assert!(store.0.is_empty());
        }

        #[test]
        fn a_missing_part_reads_as_signed_out() {
            let mut store = Memory::default();
            write(&mut store, &[b'x'; 4100]).unwrap();
            store.0.remove(&2);
            assert_eq!(read(&mut store).unwrap(), None);
        }

        #[test]
        fn entry_names() {
            assert_eq!(name("tidal-session", 0), "tidal-session");
            assert_eq!(name("tidal-session", 2), "tidal-session.2");
        }
    }
}
