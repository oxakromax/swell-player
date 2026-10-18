//! How long a resolved stream can be played: TIDAL's stream URLs are signed
//! and stop working after a while. Pure functions, unit-tested.
//!
//! The expiry is read from the URL when it says (CloudFront's `Expires=` or
//! the `DateLessThan` of its `Policy=`, an Akamai-style `exp=` token);
//! otherwise a conservative default applies. A stream counts as fresh until
//! a margin before that, so it is never handed to GStreamer about to expire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;

/// For URLs that do not say when they expire.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(20 * 60);
/// A stream stops counting as fresh this long before its URLs expire:
/// enough to load it and play into it.
pub const MARGIN: Duration = Duration::from_secs(5 * 60);
/// Never trust a stated lifetime beyond this.
const MAX_LIFETIME: Duration = Duration::from_secs(6 * 60 * 60);

/// Seconds since the Unix epoch.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// When `url` stops working (seconds since the epoch), if it says.
pub fn url_expiry(url: &str) -> Option<u64> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    let mut earliest: Option<u64> = None;
    let mut keep = |at: u64| earliest = Some(earliest.map_or(at, |e| e.min(at)));
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "Expires" | "expires" => {
                if let Ok(at) = value.parse() {
                    keep(at);
                }
            }
            "Policy" => {
                if let Some(at) = policy_expiry(value) {
                    keep(at);
                }
            }
            _ => {
                // Akamai tokens: `exp=…~acl=…~hmac=…`, possibly escaped.
                let value = value.replace("%3D", "=").replace("%3d", "=");
                for part in value.split('~') {
                    if let Some(at) = part.strip_prefix("exp=").and_then(|v| v.parse().ok()) {
                        keep(at);
                    }
                }
            }
        }
    }
    earliest
}

/// CloudFront's custom policy: URL-safe base64 of JSON whose statements
/// carry `Condition.DateLessThan["AWS:EpochTime"]`.
fn policy_expiry(policy: &str) -> Option<u64> {
    let standard: String = policy
        .chars()
        .filter(|c| *c != '=' && *c != '_')
        .map(|c| match c {
            '-' => '+',
            '~' => '/',
            c => c,
        })
        .collect();
    let json = STANDARD_NO_PAD.decode(standard.as_bytes()).ok()?;
    let policy: serde_json::Value = serde_json::from_slice(&json).ok()?;
    policy
        .get("Statement")?
        .as_array()?
        .iter()
        .filter_map(|statement| {
            statement
                .pointer("/Condition/DateLessThan/AWS:EpochTime")?
                .as_u64()
        })
        .min()
}

/// How long a stream whose URLs are `urls`, resolved at `now` (seconds
/// since the epoch), counts as fresh.
pub fn lifetime<'a>(urls: impl IntoIterator<Item = &'a str>, now: u64) -> Duration {
    let expiry = urls.into_iter().filter_map(url_expiry).min();
    match expiry {
        Some(at) => Duration::from_secs(at.saturating_sub(now))
            .min(MAX_LIFETIME)
            .saturating_sub(MARGIN),
        None => DEFAULT_LIFETIME,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    /// A CloudFront policy the way it appears in URLs.
    fn policy(epoch: u64) -> String {
        let json = format!(
            r#"{{"Statement":[{{"Resource":"https://x/*","Condition":{{"DateLessThan":{{"AWS:EpochTime":{epoch}}}}}}}]}}"#
        );
        STANDARD
            .encode(json)
            .replace('+', "-")
            .replace('=', "_")
            .replace('/', "~")
    }

    #[test]
    fn reads_cloudfront_and_akamai_expiry() {
        let url = format!(
            "https://sp-ad-cf.audio.tidal.com/mediatracks/abc/0.mp4?Policy={}&Signature=s~x&Key-Pair-Id=K",
            policy(1_792_312_665)
        );
        assert_eq!(url_expiry(&url), Some(1_792_312_665));
        assert_eq!(
            url_expiry("https://h/x.flac?Expires=1700000000&Signature=a"),
            Some(1_700_000_000)
        );
        assert_eq!(
            url_expiry("https://h/x.flac?token=exp=1700000123~acl=/*~hmac=ff"),
            Some(1_700_000_123)
        );
        assert_eq!(
            url_expiry("https://h/x.flac?__token__=exp%3D1700000124~hmac%3Dff"),
            Some(1_700_000_124)
        );
        assert_eq!(url_expiry("https://h/x.flac"), None);
        assert_eq!(url_expiry("https://h/x.flac?token=abc"), None);
        assert_eq!(url_expiry("https://h/x?Policy=%%%"), None);
    }

    #[test]
    fn lifetime_keeps_a_margin() {
        let now = 1_000_000;
        let url = format!("https://h/a?Expires={}", now + 3600);
        assert_eq!(
            lifetime([url.as_str()], now),
            Duration::from_secs(3600) - MARGIN
        );
        // The earliest URL decides.
        let soon = format!("https://h/b?Expires={}", now + 600);
        assert_eq!(
            lifetime([url.as_str(), soon.as_str()], now),
            Duration::from_secs(600) - MARGIN
        );
        // Already (nearly) expired: not fresh at all.
        let past = format!("https://h/c?Expires={}", now - 10);
        assert_eq!(lifetime([past.as_str()], now), Duration::ZERO);
        // Unknown: the default.
        assert_eq!(lifetime(["https://h/d?t=1"], now), DEFAULT_LIFETIME);
        assert_eq!(lifetime([], now), DEFAULT_LIFETIME);
        // Implausibly long: capped.
        let far = format!("https://h/e?Expires={}", now + 365 * 86_400);
        assert_eq!(lifetime([far.as_str()], now), MAX_LIFETIME - MARGIN);
    }
}
