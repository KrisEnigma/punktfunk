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

/// Mint a fresh short-lived identity and publish its hash. Returns the identity for the endpoint.
fn mint(port: u16, sans: &[String]) -> Result<Identity> {
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
    if let Ok(mut g) = PUBLISHED.write() {
        *g = Some(Published {
            port,
            cert_hash: cert_hash.clone(),
            expires_at,
        });
    }
    tracing::info!(
        port,
        cert_hash = %cert_hash,
        validity_days = VALIDITY_DAYS,
        "WebTransport identity minted (in memory, never persisted)"
    );
    Ok(identity)
}

/// Run the plane until the process ends, re-minting the identity as it ages out.
///
/// A rotation rebuilds the endpoint, which drops whatever is connected. Once every twelve days,
/// and a browser reconnects on its own — cheaper than teaching quinn to swap a certificate under
/// live sessions, and the reconnect path has to work anyway.
pub async fn serve(port: u16, sans: Vec<String>) -> Result<()> {
    loop {
        let identity = mint(port, &sans)?;
        let config = ServerConfig::builder()
            .with_bind_default(port)
            .with_identity(identity)
            // A browser tab that is throttled in the background must not look like a dead peer.
            .keep_alive_interval(Some(Duration::from_secs(3)))
            .build();
        let endpoint = Endpoint::server(config).context("bind the WebTransport endpoint")?;
        tracing::info!(port, "WebTransport plane listening");
        // Accept until the certificate is due for replacement, then fall out and re-mint.
        tokio::select! {
            _ = accept_loop(endpoint) => {}
            () = tokio::time::sleep(ROTATE_AFTER) => {
                tracing::info!("WebTransport certificate due for rotation");
            }
        }
    }
}

async fn accept_loop(endpoint: Endpoint<wtransport::endpoint::endpoint_side::Server>) {
    loop {
        let incoming = endpoint.accept().await;
        tokio::spawn(async move {
            if let Err(e) = session(incoming).await {
                tracing::debug!(error = %e, "WebTransport session ended");
            }
        });
    }
}

/// One browser session. Phase 1 echoes: the plan's exit criterion is a page that connects and
/// sends datagrams both ways, and everything real waits for the `Transport` impl in Phase 2.
async fn session(incoming: wtransport::endpoint::IncomingSession) -> Result<()> {
    let request = incoming.await.context("await session request")?;
    let path = request.path().to_string();
    let connection = request.accept().await.context("accept session")?;
    tracing::info!(path = %path, "WebTransport session accepted");
    // One control stream and datagrams, mirroring the native plane's split.
    let mut control = [0u8; 4096];
    loop {
        tokio::select! {
            datagram = connection.receive_datagram() => {
                let datagram = datagram.context("receive datagram")?;
                connection.send_datagram(&*datagram).context("echo datagram")?;
            }
            stream = connection.accept_bi() => {
                let (mut tx, mut rx) = stream.context("accept control stream")?;
                while let Some(n) = rx.read(&mut control).await.context("read control")? {
                    tx.write_all(&control[..n]).await.context("echo control")?;
                }
            }
        }
    }
}
