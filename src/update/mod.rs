//! Update checks against Swell's releases on GitHub.
//!
//! [`latest`] asks GitHub for the newest published release (drafts and
//! pre-releases are never listed there); [`Release::is_newer`] compares
//! its tag with this build's version. What a newer release means depends
//! on how Swell was installed ([`channel`]): the Windows installer and the
//! macOS app download its package and replace themselves when Swell quits
//! ([`install`]); every other install only says that it is out (the UI
//! side is `ui/updates.rs`).
//!
//! Distributions that deliver updates themselves build with
//! `SWELL_DISABLE_UPDATE_CHECK=1`, which turns all of this off.
//! Development builds (run from a Cargo target directory) never check
//! unless `SWELL_UPDATE_CHECK=1` is set when they run.

pub mod channel;
pub mod install;

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// GitHub's newest published release of Swell.
const LATEST_URL: &str = "https://api.github.com/repos/oxakromax/swell-player/releases/latest";
/// Where a release's files are downloaded from: `<this><tag>/<name>`.
const DOWNLOAD_URL: &str = "https://github.com/oxakromax/swell-player/releases/download/";
/// The releases' pages (notes and downloads): `<this>tag/<tag>`.
const RELEASES_URL: &str = "https://github.com/oxakromax/swell-player/releases/";
/// GitHub's answer is a few kilobytes per asset.
const MAX_RELEASE_JSON: u64 = 1024 * 1024;

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Whether this build checks for updates: not when a packager turned it
/// off at build time, nor from a development build (unless asked).
pub fn enabled() -> bool {
    let disabled =
        option_env!("SWELL_DISABLE_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0");
    if disabled {
        return false;
    }
    let forced = std::env::var_os("SWELL_UPDATE_CHECK").is_some_and(|v| v == "1");
    forced || !channel::development_build()
}

/// A release's version: major, minor and patch numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
    /// `1.2.3`, or a tag `v1.2.3`. Anything else (pre-releases such as
    /// `1.2.0-rc.1` included) is not a version, so it is never newer.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|part| {
            let digits =
                !part.is_empty() && part.len() <= 9 && part.bytes().all(|b| b.is_ascii_digit());
            digits.then(|| part.parse::<u32>().ok()).flatten()
        });
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Its tag (`v1.2.0`), which its download addresses use.
    pub tag: String,
    /// Its page on GitHub: release notes and every package.
    pub page: String,
    pub assets: Vec<Asset>,
}

/// A file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(rename = "browser_download_url")]
    pub url: String,
    /// In bytes.
    pub size: u64,
}

impl Release {
    /// Whether this release is newer than the running build.
    pub fn is_newer(&self) -> bool {
        self.is_newer_than(CURRENT)
    }

    fn is_newer_than(&self, current: &str) -> bool {
        Version::parse(current).is_some_and(|current| self.version > current)
    }
}

/// The fields of GitHub's release object that are used.
#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// Reads GitHub's answer for a release. Only this repository's release
/// pages are kept as its page (it is opened in the browser).
fn parse_release(json: &[u8]) -> Result<Release> {
    let release: ReleaseJson = serde_json::from_slice(json).context("unexpected release data")?;
    if release.draft || release.prerelease {
        bail!("release {} is not a published one", release.tag_name);
    }
    let version = Version::parse(&release.tag_name)
        .with_context(|| format!("release tag {:?} is not a version", release.tag_name))?;
    let page = if release.html_url.starts_with(RELEASES_URL) {
        release.html_url
    } else {
        format!("{RELEASES_URL}tag/{}", release.tag_name)
    };
    Ok(Release {
        version,
        tag: release.tag_name,
        page,
        assets: release.assets,
    })
}

/// The HTTP client for GitHub. No overall deadline, since a package takes
/// as long as it takes to download; stalled reads fail.
fn client() -> Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("swell-player/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

/// The newest published release of Swell.
pub async fn latest() -> Result<Release> {
    let response = client()?
        .get(LATEST_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?;
    parse_release(&read_all(response, MAX_RELEASE_JSON).await?)
}

/// The body of `response`, refused when it is longer than `limit` bytes.
async fn read_all(mut response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        bail!("{} is larger than expected", response.url());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if (body.len() + chunk.len()) as u64 > limit {
            bail!("{} is larger than expected", response.url());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("v10.0.12"), Some(Version(10, 0, 12)));
        for odd in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.3-rc.1",
            "v1.2.0-beta",
            "1..3",
            "1.2.x",
            "+1.2.3",
            " 1.2.3",
            "vv1.2.3",
            "1.2.9999999999",
        ] {
            assert_eq!(Version::parse(odd), None, "{odd:?}");
        }
        assert!(Version(1, 10, 0) > Version(1, 9, 9));
        assert!(Version(2, 0, 0) > Version(1, 99, 99));
        assert_eq!(Version(1, 2, 0).to_string(), "1.2.0");
    }

    fn release(tag: &str) -> Release {
        Release {
            version: Version::parse(tag).unwrap(),
            tag: tag.to_owned(),
            page: String::new(),
            assets: Vec::new(),
        }
    }

    #[test]
    fn newer_releases() {
        assert!(release("v1.1.1").is_newer_than("1.1.0"));
        assert!(release("v1.10.0").is_newer_than("1.9.0"));
        assert!(!release("v1.1.0").is_newer_than("1.1.0"));
        assert!(!release("v1.0.9").is_newer_than("1.1.0"));
        // A build whose own version is odd is never told about updates.
        assert!(!release("v9.0.0").is_newer_than("1.2.0-dev"));
        assert!(Version::parse(CURRENT).is_some(), "Cargo.toml's version");
    }

    #[test]
    fn parses_github_releases() {
        let json = br#"{
            "url": "https://api.github.com/repos/oxakromax/swell-player/releases/1",
            "html_url": "https://github.com/oxakromax/swell-player/releases/tag/v1.2.0",
            "tag_name": "v1.2.0",
            "name": "Swell 1.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "SHA256SUMS",
                    "size": 812,
                    "content_type": "application/octet-stream",
                    "browser_download_url": "https://github.com/oxakromax/swell-player/releases/download/v1.2.0/SHA256SUMS"
                },
                {
                    "name": "swell-player-1.2.0-windows-x86_64-setup.exe",
                    "size": 73400320,
                    "browser_download_url": "https://github.com/oxakromax/swell-player/releases/download/v1.2.0/swell-player-1.2.0-windows-x86_64-setup.exe"
                }
            ]
        }"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, Version(1, 2, 0));
        assert_eq!(release.tag, "v1.2.0");
        assert_eq!(
            release.page,
            "https://github.com/oxakromax/swell-player/releases/tag/v1.2.0"
        );
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[1].size, 73_400_320);
        assert_eq!(release.assets[0].name, "SHA256SUMS");

        // A page elsewhere is replaced by the tag's page here.
        let elsewhere = br#"{"tag_name":"v1.2.0","html_url":"https://example.com/x","assets":[]}"#;
        assert_eq!(
            parse_release(elsewhere).unwrap().page,
            "https://github.com/oxakromax/swell-player/releases/tag/v1.2.0"
        );
        // Odd tags, pre-releases, drafts and other data are refused.
        for json in [
            br#"{"tag_name":"nightly","assets":[]}"#.as_slice(),
            br#"{"tag_name":"v1.3.0-rc.1","assets":[]}"#,
            br#"{"tag_name":"v1.3.0","prerelease":true,"assets":[]}"#,
            br#"{"tag_name":"v1.3.0","draft":true,"assets":[]}"#,
            br#"{"message":"API rate limit exceeded"}"#,
            b"<html>",
        ] {
            assert!(
                parse_release(json).is_err(),
                "{}",
                String::from_utf8_lossy(json)
            );
        }
    }
}
