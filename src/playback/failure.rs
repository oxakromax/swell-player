//! What a playback failure means: the network is gone (keep the track and
//! retry), the track itself cannot be played (count it towards skipping),
//! or the session is over. Plus log hygiene for error texts that may carry
//! signed URLs.

use std::time::Duration;

use crate::tidal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No connection, a timeout or a server-side (5xx/429) error: retry the
    /// same track at the same position.
    Network,
    /// The track cannot be played (4xx, unsupported manifest, decoding).
    Track,
    /// The TIDAL session has expired: nothing will play until sign-in.
    Session,
}

/// Classifies an error from resolving a stream (`Api::stream*`).
pub fn classify(err: &anyhow::Error) -> Failure {
    if tidal::is_auth_error(err) {
        return Failure::Session;
    }
    for cause in err.chain() {
        if let Some(err) = cause.downcast_ref::<reqwest::Error>() {
            return match err.status() {
                Some(status) => by_status(status.as_u16()),
                // Connect, timeout, request, body: the transport failed.
                None => Failure::Network,
            };
        }
        if let Some(tidal::Error::Api { status, .. }) = cause.downcast_ref::<tidal::Error>() {
            return by_status(*status);
        }
    }
    Failure::Track
}

fn by_status(status: u16) -> Failure {
    match status {
        408 | 429 => Failure::Network,
        500..=599 => Failure::Network,
        _ => Failure::Track,
    }
}

/// The parts of a GStreamer error message that matter for [`classify_engine`].
#[derive(Clone, Debug, Default)]
pub struct EngineError {
    /// A `GST_RESOURCE_ERROR` that is about reading or opening (read,
    /// open-read, open, seek, failed), not "not found"/"not authorised".
    pub resource_io: bool,
    /// The element that posted it fetches over HTTP (`souphttpsrc`, an
    /// adaptive demuxer...).
    pub from_http: bool,
    /// `http-status-code` from the message details (0: no answer at all).
    pub http_status: Option<u32>,
    /// An adaptive demuxer gave up downloading fragments.
    pub download_error: bool,
}

/// Classifies a GStreamer error. Never [`Failure::Session`].
pub fn classify_engine(error: &EngineError) -> Failure {
    match error.http_status {
        // 0: the request got no answer (DNS, connect, reset).
        Some(0) => Failure::Network,
        Some(status) => by_status(u16::try_from(status).unwrap_or(u16::MAX)),
        None if error.download_error => Failure::Network,
        None if error.resource_io && error.from_http => Failure::Network,
        None => Failure::Track,
    }
}

/// Whether an element (by factory name) fetches over the network.
pub fn is_http_element(factory: &str) -> bool {
    factory.contains("http") || factory.contains("soup") || factory.contains("demux2")
}

/// Wait before network retry number `attempt` (1-based): 1, 2, 4, 8, 16,
/// then 30 s.
pub fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.saturating_sub(1).min(5);
    Duration::from_secs(secs.min(30))
}

/// `text` with the query string of every URL in it removed: signed stream
/// URLs carry their token there.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = find_url(rest) {
        out.push_str(&rest[..start]);
        let url = &rest[start..];
        let end = url
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>' | ',' | ';'))
            .unwrap_or(url.len());
        let (url, after) = url.split_at(end);
        match url.find(['?', '#']) {
            Some(query) => {
                out.push_str(&url[..query]);
                out.push_str("?…");
            }
            None => out.push_str(url),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn find_url(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    ["https://", "http://", "file://"]
        .iter()
        .filter_map(|scheme| lower.find(scheme))
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16, sub_status: u32) -> anyhow::Error {
        tidal::Error::Api {
            status,
            sub_status,
            message: "x".into(),
        }
        .into()
    }

    /// A real transport error: nothing listens on port 9 of localhost.
    fn connect_error() -> anyhow::Error {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = runtime
            .block_on(
                reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap()
                    .get("http://127.0.0.1:9/")
                    .send(),
            )
            .unwrap_err();
        assert!(err.is_connect() || err.is_request());
        anyhow::Error::new(err).context("could not reach TIDAL")
    }

    #[test]
    fn resolve_errors() {
        assert_eq!(classify(&connect_error()), Failure::Network);
        assert_eq!(
            classify(&connect_error().context("resolving the stream")),
            Failure::Network
        );
        assert_eq!(classify(&api(503, 0)), Failure::Network);
        assert_eq!(classify(&api(429, 0)), Failure::Network);
        assert_eq!(classify(&api(404, 2001)), Failure::Track);
        assert_eq!(classify(&api(401, 4005)), Failure::Track);
        assert_eq!(classify(&api(401, 11003)), Failure::Session);
        assert_eq!(
            classify(&tidal::Error::SessionExpired.into()),
            Failure::Session
        );
        assert_eq!(
            classify(&anyhow::anyhow!("unsupported manifest type x")),
            Failure::Track
        );
    }

    #[test]
    fn engine_errors() {
        let soup = |status| EngineError {
            resource_io: true,
            from_http: true,
            http_status: status,
            download_error: false,
        };
        assert_eq!(classify_engine(&soup(Some(0))), Failure::Network);
        assert_eq!(classify_engine(&soup(Some(502))), Failure::Network);
        assert_eq!(classify_engine(&soup(Some(403))), Failure::Track);
        assert_eq!(classify_engine(&soup(None)), Failure::Network);
        let decode = EngineError::default();
        assert_eq!(classify_engine(&decode), Failure::Track);
        let local_read = EngineError {
            resource_io: true,
            ..EngineError::default()
        };
        assert_eq!(classify_engine(&local_read), Failure::Track);
        let demux = EngineError {
            download_error: true,
            ..EngineError::default()
        };
        assert_eq!(classify_engine(&demux), Failure::Network);
        assert!(is_http_element("souphttpsrc"));
        assert!(is_http_element("dashdemux2"));
        assert!(!is_http_element("flacdec"));
    }

    #[test]
    fn backoff_doubles_then_caps() {
        let secs: Vec<u64> = (1..=8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn redacts_query_strings() {
        assert_eq!(
            redact(
                "Not Found (404), URL: https://sp-ad.audio.tidal.com/x/1.flac?token=abc~sig, Redirect to: (NULL)"
            ),
            "Not Found (404), URL: https://sp-ad.audio.tidal.com/x/1.flac?…, Redirect to: (NULL)"
        );
        assert_eq!(
            redact(
                "error sending request for url (HTTPS://api.tidal.com/v1/tracks/1/playbackinfo?countryCode=ES)"
            ),
            "error sending request for url (HTTPS://api.tidal.com/v1/tracks/1/playbackinfo?…)"
        );
        assert_eq!(redact("no url here"), "no url here");
        assert_eq!(redact("a http://h/p b"), "a http://h/p b");
    }
}
