//! The browser plane: a WebTransport endpoint for `clients/web`
//! (`design/web-client-implementation-plan.md` Phase 1). Datagrams carry media, one bidirectional
//! stream carries control — the same split the native plane uses, because WebTransport *is* QUIC
//! and a browser cannot open ours.
//!
//! **Off unless asked for** (`--webtransport` / `PUNKTFUNK_WEBTRANSPORT=1`), the stance
//! `--gamestream` takes: a new externally-reachable transport does not appear on an upgrade.
//!
//! This plane mints its OWN ECDSA P-256 certificate, valid 13 days, and rotates it.
//! `serverCertificateHashes` refuses anything over two weeks, and the native identity
//! ([`crate::identity`]) is deliberately the opposite — long-lived, because clients pin it. The
//! key is never written to disk: minted at start-up, held in memory, replaced on rotation.
//!
//! [`published`] feeds an UNAUTHENTICATED management route, because a browser that has never
//! paired holds no client certificate. That route proves nothing on its own — substitute both
//! hash and certificate and the browser connects to you. PAKE pairing over the control stream is
//! what proves the peer, and it does not need the transport authenticated. Until that lands
//! (Phase 3) this plane echoes and carries no session.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};
use wtransport::{Endpoint, Identity, ServerConfig};

/// Default listen port. Distinct from the native plane's 9777: this one speaks HTTP/3, and a
/// browser and a native client can be connected at the same time.
pub const DEFAULT_PORT: u16 = 9778;

/// Certificate lifetime. The spec's ceiling is two weeks and it is a hard refusal, so 13 days
/// leaves a day of margin for a client whose clock runs fast.
const VALIDITY_DAYS: u32 = 13;

/// Rotate a day before expiry, so a browser that fetched the hash a moment ago still finds the
/// certificate that hash names.
const ROTATE_AFTER: Duration = Duration::from_secs((VALIDITY_DAYS as u64 - 1) * 24 * 60 * 60);

/// What a browser needs before it can dial: the port, the hash to pin, and when that stops being
/// true. Published for the management route; contains nothing secret — a certificate hash is
/// learned by anyone who connects.
#[derive(Clone, Debug)]
pub struct Published {
    pub port: u16,
    /// Lowercase hex SHA-256 of the leaf DER, the form `serverCertificateHashes` wants.
    pub cert_hash: String,
    /// Unix seconds. A client past this must re-fetch before dialling.
    pub expires_at: u64,
}

static PUBLISHED: RwLock<Option<Published>> = RwLock::new(None);

/// The live certificate's details, or `None` when the plane is not running.
pub fn published() -> Option<Published> {
    PUBLISHED.read().ok().and_then(|g| g.clone())
}

/// Stop advertising. The route answers 404 again, which is what a caller should see when the
/// plane is not listening.
fn withdraw() {
    if let Ok(mut g) = PUBLISHED.write() {
        *g = None;
    }
}

/// Mint a fresh short-lived identity. Returns it with what a browser would need to dial it —
/// the caller publishes that only once the endpoint is actually bound, so the route never
/// advertises a plane that is not listening.
fn mint(port: u16, sans: &[String]) -> Result<(Identity, Published)> {
    let identity = Identity::self_signed_builder()
        .subject_alt_names(sans)
        .from_now_utc()
        .validity_days(VALIDITY_DAYS)
        .build()
        .context("mint the WebTransport identity")?;
    let leaf = identity
        .certificate_chain()
        .as_slice()
        .first()
        .context("minted identity has no leaf certificate")?;
    // `Sha256DigestFmt::DottedHex` is `aa:bb:…`; the browser wants raw bytes and our route
    // publishes plain hex, so strip the separators rather than hand-roll the digest.
    let cert_hash = leaf
        .hash()
        .fmt(wtransport::tls::Sha256DigestFmt::DottedHex)
        .replace(':', "");
    let expires_at =
        SystemTime::now() + Duration::from_secs(u64::from(VALIDITY_DAYS) * 24 * 60 * 60);
    let expires_at = expires_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    tracing::info!(
        port,
        cert_hash = %cert_hash,
        validity_days = VALIDITY_DAYS,
        "WebTransport identity minted (in memory, never persisted)"
    );
    Ok((
        identity,
        Published {
            port,
            cert_hash,
            expires_at,
        },
    ))
}

/// Run the plane until the process ends, re-minting the identity as it ages out.
///
/// A rotation rebuilds the endpoint, which drops whatever is connected. Once every twelve days,
/// and a browser reconnects on its own — cheaper than teaching quinn to swap a certificate under
/// live sessions, and the reconnect path has to work anyway.
pub async fn serve(bind: SocketAddr, sans: Vec<String>, origins: Vec<String>) -> Result<()> {
    let port = bind.port();
    loop {
        let (identity, publish) = mint(port, &sans)?;
        let config = ServerConfig::builder()
            .with_bind_address(bind)
            .with_identity(identity)
            // A browser tab that is throttled in the background must not look like a dead peer.
            .keep_alive_interval(Some(Duration::from_secs(3)))
            .build();
        // Bind first, publish second. The other order leaves the route advertising a hash for a
        // plane that never came up, which reads as the API lying.
        let endpoint = match Endpoint::server(config).context("bind the WebTransport endpoint") {
            Ok(endpoint) => endpoint,
            Err(e) => {
                withdraw();
                return Err(e);
            }
        };
        if let Ok(mut g) = PUBLISHED.write() {
            *g = Some(publish);
        }
        tracing::info!(%bind, "WebTransport plane listening");
        // Accept until the certificate is due for replacement, then fall out and re-mint.
        tokio::select! {
            _ = accept_loop(endpoint, origins.clone()) => {}
            () = tokio::time::sleep(ROTATE_AFTER) => {
                tracing::info!("WebTransport certificate due for rotation");
            }
        }
    }
}

async fn accept_loop(
    endpoint: Endpoint<wtransport::endpoint::endpoint_side::Server>,
    origins: Vec<String>,
) {
    loop {
        let incoming = endpoint.accept().await;
        let origins = origins.clone();
        tokio::spawn(async move {
            if let Err(e) = session(incoming, &origins).await {
                tracing::debug!(error = %e, "WebTransport session ended");
            }
        });
    }
}

/// Is this page allowed to open a session?
///
/// WebTransport is **not** subject to CORS, so without this any page the user happens to have
/// open can reach the plane — the browser will not stop it. An empty allowlist means "anything",
/// which is what a host with no configured origins has to mean until the console can offer the
/// choice; the log line below is what tells an operator which value to set.
fn origin_allowed(origin: Option<&str>, allowed: &[String]) -> bool {
    allowed.is_empty() || origin.is_some_and(|o| allowed.iter().any(|a| a == o))
}

/// One browser session. Phase 1 echoes: the plan's exit criterion is a page that connects and
/// sends datagrams both ways, and everything real waits for the `Transport` impl in Phase 2.
async fn session(
    incoming: wtransport::endpoint::IncomingSession,
    origins: &[String],
) -> Result<()> {
    let request = incoming.await.context("await session request")?;
    // The peer picks `:path` and nothing upstream value-checks it, so control characters would
    // reach the log ring verbatim and let an unauthenticated peer forge log lines.
    let path: String = request
        .path()
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect();
    // Same treatment as `:path`: peer-chosen, and it reaches a log an operator reads back.
    let origin: Option<String> = request.origin().map(|o| {
        o.chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect::<String>()
    });
    if !origin_allowed(origin.as_deref(), origins) {
        request.forbidden().await;
        tracing::warn!(
            origin = origin.as_deref().unwrap_or("<none>"),
            "WebTransport session refused: origin not in PUNKTFUNK_WEBTRANSPORT_ORIGINS"
        );
        return Ok(());
    }
    let connection = request.accept().await.context("accept session")?;
    tracing::info!(
        path = %path,
        origin = origin.as_deref().unwrap_or("<none>"),
        "WebTransport session accepted"
    );
    // One control stream and datagrams, mirroring the native plane's split. The control stream
    // gets its own task: it lives as long as the session, and reading it here would park the
    // media arm for that whole time — media and control have to run at once.
    loop {
        tokio::select! {
            datagram = connection.receive_datagram() => {
                let datagram = datagram.context("receive datagram")?;
                connection.send_datagram(&*datagram).context("echo datagram")?;
            }
            stream = connection.accept_bi() => {
                let (mut tx, mut rx) = stream.context("accept control stream")?;
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    while let Ok(Some(n)) = rx.read(&mut buf).await {
                        if tx.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two-week ceiling is a hard refusal in the browser, and a certificate that misses it
    /// fails at connect time with nothing useful to read. Check what we publish, and that the hash
    /// is the 64 lowercase hex characters `serverCertificateHashes` parses.
    #[test]
    fn minted_identity_is_short_lived_and_publishes_a_usable_hash() {
        let before = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (_identity, p) = mint(9778, &["localhost".to_string()]).expect("mint");

        assert_eq!(p.port, 9778);
        assert_eq!(p.cert_hash.len(), 64, "SHA-256 as hex");
        assert!(
            p.cert_hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "lowercase hex only: {}",
            p.cert_hash
        );
        let lifetime = p.expires_at - before;
        assert!(
            lifetime < 14 * 24 * 60 * 60,
            "must stay under the spec's two weeks, got {lifetime}s"
        );
        assert!(lifetime > 12 * 24 * 60 * 60, "and not be pointlessly short");
        assert!(
            ROTATE_AFTER.as_secs() < lifetime,
            "rotation must come before expiry"
        );
    }

    /// The gate that stands in for the same-origin policy WebTransport does not get.
    #[test]
    fn origin_gate_admits_only_what_was_configured() {
        let allowed = vec!["https://host.local:47990".to_string()];
        assert!(origin_allowed(Some("https://host.local:47990"), &allowed));
        assert!(!origin_allowed(Some("https://evil.example"), &allowed));
        // A peer that sends no Origin at all must not slip past a configured list.
        assert!(!origin_allowed(None, &allowed));
        // Exact match only: a prefix or a suffix is a different origin.
        assert!(!origin_allowed(
            Some("https://host.local:47990.evil.example"),
            &allowed
        ));
        assert!(!origin_allowed(
            Some("https://evil.host.local:47990"),
            &allowed
        ));
        // Unconfigured means "any", which is what a host with no console setting has to mean.
        assert!(origin_allowed(Some("https://anything"), &[]));
        assert!(origin_allowed(None, &[]));
    }
}
