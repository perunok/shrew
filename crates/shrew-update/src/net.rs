//! Minimal HTTP(S) GETs on ureq + rustls/RustCrypto, plus `file://` URLs for local feeds and tests.

use std::{
    fs::File,
    io::{self, Read},
    sync::{Arc, OnceLock},
    time::Duration,
};

use ureq::{
    Agent,
    tls::{RootCerts, TlsConfig, TlsProvider},
};

use crate::{Error, Result, USER_AGENT};

/// A response body still being streamed, with its length when the server sent one.
pub(crate) struct Body {
    pub reader: Box<dyn Read + Send>,
    pub len: Option<u64>,
}

/// GETs `url`. `Ok(None)` means 404 (a repo with no releases yet, a missing asset).
pub(crate) fn get(url: &str, accept: &str, timeout: Option<Duration>) -> Result<Option<Body>> {
    if let Some(path) = url.strip_prefix("file://") {
        // `file:///C:/feed.json` (the standard form) names `C:/feed.json` on Windows
        let path = match path.as_bytes() {
            [b'/', d, b':', ..] if d.is_ascii_alphabetic() => &path[1..],
            _ => path,
        };
        return match File::open(path) {
            Ok(file) => Ok(Some(Body { len: file.metadata().ok().map(|m| m.len()), reader: Box::new(file) })),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        };
    }
    let host = host(url);
    let response = agent()
        .get(url)
        .header("Accept", accept)
        .config()
        // Never let an https feed be redirected to plain http.
        .https_only(url.starts_with("https://"))
        .timeout_global(timeout)
        .build()
        .call()
        .map_err(|err| Error::Http(format!("cannot reach {host}: {err}")))?;
    let status = response.status().as_u16();
    match status {
        200..=299 => {}
        404 => return Ok(None),
        403 | 429 if response.headers().get("x-ratelimit-remaining").is_some_and(|v| v == "0") => {
            return Err(Error::Http(format!("{host} rate limit reached, try again later")));
        }
        _ => return Err(Error::Http(format!("{host} answered HTTP {status}"))),
    }
    let len = response.body().content_length();
    Ok(Some(Body { reader: Box::new(response.into_body().into_reader()), len }))
}

/// GETs a small text document, refusing anything over `limit` bytes.
pub(crate) fn get_text(url: &str, accept: &str, limit: u64) -> Result<Option<String>> {
    let Some(body) = get(url, accept, Some(Duration::from_secs(30)))? else { return Ok(None) };
    let mut text = String::new();
    body.reader
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|err| Error::Http(format!("reading {}: {err}", host(url))))?;
    if text.len() as u64 > limit {
        return Err(Error::Feed(format!("{} sent more than {limit} bytes", host(url))));
    }
    Ok(Some(text))
}

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::WebPki)
            .unversioned_rustls_crypto_provider(Arc::new(rustls_rustcrypto::provider()))
            .build();
        Agent::config_builder()
            .tls_config(tls)
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build()
            .new_agent()
    })
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}
