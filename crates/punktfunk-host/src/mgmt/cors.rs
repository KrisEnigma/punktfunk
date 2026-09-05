//! Cross-origin access for the browser client.
//!
//! The page is not served by the host — it is a static build that can live anywhere — so every
//! call it makes is cross-origin, and without these headers the browser refuses to let it read
//! the response. That is the second wall after the certificate one; both have to come down
//! before a browser can list a library.
//!
//! **`Access-Control-Allow-Credentials` is never sent, and that is what makes this safe.** The
//! management API has no cookies and no ambient session: authority comes from a bearer token a
//! caller must have earned by signing a nonce with a paired device key, or from a client
//! certificate the browser cannot present at all. So allowing an origin to *read* a response
//! grants nothing it could not already have — it must still hold a credential to get one worth
//! reading, and CORS never stopped a page from *sending* a request in the first place.
//!
//! `PUNKTFUNK_WEBTRANSPORT_ORIGINS` narrows it, the same list that confines the browser plane:
//! one setting for "which pages may talk to this host". Empty means any, which is what a host
//! with no configured origins has to mean until the console can offer the choice.

use axum::extract::Request;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Headers a browser may send us. `authorization` is the reason a preflight happens at all —
/// it is not on the CORS safelist, so every authenticated call is preceded by an `OPTIONS`.
const ALLOW_HEADERS: &str = "authorization, content-type";

/// Answer the preflight, then stamp the actual response.
///
/// A request with no `Origin` is not from a browser's cross-origin path and is left untouched:
/// native clients and the tray must not have headers grown around them.
pub(crate) async fn cors(req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        // Peer-chosen and echoed back into a header, so it is length-capped and stripped of
        // anything that could split one.
        .filter(|o| o.len() <= 256 && !o.chars().any(|c| c.is_control()))
        .map(str::to_owned);
    let Some(origin) = origin else {
        return next.run(req).await;
    };
    if !allowed(&origin, &pf_host_config::config().webtransport_origins) {
        // No headers: the browser blocks the read. The request is still served, because refusing
        // it would be a different answer to a non-browser caller that happened to send an Origin.
        return next.run(req).await;
    }

    // The preflight never reaches a handler — it carries no credential, and `require_auth` would
    // correctly refuse it.
    if req.method() == Method::OPTIONS {
        let mut res = Response::new(axum::body::Body::empty());
        *res.status_mut() = StatusCode::NO_CONTENT;
        stamp(res.headers_mut(), &origin, true);
        return res;
    }
    let mut res = next.run(req).await;
    stamp(res.headers_mut(), &origin, false);
    res
}

fn stamp(headers: &mut axum::http::HeaderMap, origin: &str, preflight: bool) {
    let Ok(value) = HeaderValue::from_str(origin) else {
        return;
    };
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    // The response varies by Origin, so a cache must not serve one origin's copy to another.
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    if preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOW_HEADERS),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
    }
    // Deliberately absent: `Access-Control-Allow-Credentials`. See the module docs — nothing
    // here is authorised by anything the browser attaches on its own.
}

/// Exact match, or anything when the operator has configured no list.
fn allowed(origin: &str, configured: &[String]) -> bool {
    configured.is_empty() || configured.iter().any(|a| a == origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_gate_matches_exactly() {
        let list = vec!["https://web.punktfunk.io".to_string()];
        assert!(allowed("https://web.punktfunk.io", &list));
        assert!(!allowed("https://evil.example", &list));
        // A prefix or a suffix is a different origin, which is the classic way this is got wrong.
        assert!(!allowed("https://web.punktfunk.io.evil.example", &list));
        assert!(!allowed("https://evil.web.punktfunk.io", &list));
        // Unconfigured means any, as it does for the browser plane itself.
        assert!(allowed("https://anything", &[]));
    }

    /// The one header that would turn this into a hole. Pinned so a future edit has to argue
    /// with a test rather than quietly add it.
    #[test]
    fn credentials_are_never_allowed() {
        let mut headers = axum::http::HeaderMap::new();
        stamp(&mut headers, "https://web.punktfunk.io", true);
        assert!(headers
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_none());
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "https://web.punktfunk.io"
        );
        assert_eq!(headers.get(header::VARY).unwrap(), "Origin");
    }

    /// A header value cannot be split by what a peer sent.
    #[test]
    fn a_control_character_never_reaches_a_header() {
        assert!(HeaderValue::from_str("https://x\r\nX-Evil: 1").is_err());
    }
}
