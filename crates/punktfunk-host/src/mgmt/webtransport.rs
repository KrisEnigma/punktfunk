//! What a browser must know before it can dial the WebTransport plane.
//!
//! Unauthenticated on purpose, and `require_auth` exempts the path: a browser that has never
//! paired holds no client certificate, so there is no credential it could present here. The
//! response carries nothing secret — a certificate hash is what any peer learns by connecting —
//! and it authorises nothing. An on-path attacker who substitutes both the hash and the
//! certificate still cannot complete PAKE pairing, which is where the peer is actually proven
//! (`design/web-client.md` §4).
//!
//! `404` when the plane is off, so a page can tell "this host does not offer it" from "this host
//! is unreachable".

use crate::mgmt::shared::ApiError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

/// Everything `new WebTransport(url, { serverCertificateHashes })` needs.
#[derive(Serialize, ToSchema)]
pub(crate) struct WebTransportInfo {
    /// UDP port the plane listens on. Not the management port, and not the native plane's.
    port: u16,
    /// Lowercase hex SHA-256 of the leaf certificate DER — the bytes that go in
    /// `serverCertificateHashes[0].value`.
    cert_hash_sha256: String,
    /// Unix seconds. Past this the certificate has rotated and the hash above is stale; fetch
    /// again rather than cache. Always under two weeks out — the spec refuses anything longer.
    expires_at: u64,
    /// `allowPooling: true` is a `TypeError` when combined with `serverCertificateHashes`, so a
    /// client must pass this. Stated here because a browser that ignores it fails at Web PKI
    /// validation with no useful error.
    allow_pooling: bool,
}

/// Where to reach the browser plane
#[utoipa::path(
    get,
    path = "/webtransport",
    tag = "host",
    operation_id = "getWebTransport",
    // Override the document-global bearerAuth: a browser has no credential before pairing.
    security(()),
    responses(
        (status = OK, description = "The live certificate hash and port", body = WebTransportInfo),
        (status = NOT_FOUND, description = "The browser plane is not enabled on this host", body = ApiError),
    )
)]
pub(crate) async fn get_webtransport() -> Response {
    match crate::webtransport::published() {
        Some(p) => Json(WebTransportInfo {
            port: p.port,
            cert_hash_sha256: p.cert_hash,
            expires_at: p.expires_at,
            allow_pooling: false,
        })
        .into_response(),
        None => crate::mgmt::shared::api_error(
            StatusCode::NOT_FOUND,
            "the WebTransport plane is not enabled (--webtransport / PUNKTFUNK_WEBTRANSPORT)",
        ),
    }
}
