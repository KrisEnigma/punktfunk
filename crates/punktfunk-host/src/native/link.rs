//! The session's control connection, whichever transport carries it.
//!
//! The native plane rides quinn; a browser rides WebTransport (`design/web-client.md`). Video was
//! always portable — `Session` holds a `Box<dyn Transport>` — but audio, cursor, rumble and HID
//! out call `send_datagram` on the connection directly, and the handshake and control plane are
//! streams on it. Those call sites named `quinn::Connection`, which is what kept a browser to the
//! video half.
//!
//! **An enum, not a trait.** Two carriers, both known at compile time, so no `dyn` — and, the
//! reason that matters, `async fn` stays an ordinary `async fn`. A `dyn`-compatible trait would
//! box a future on `closed()`, which is on the per-session path.
//!
//! Most of this delegates rather than branches: WebTransport *is* QUIC, and `wtransport` hands
//! out the `quinn::Connection` underneath, so path and lifecycle questions have one answer for
//! both. Only the two genuinely carrier-shaped questions branch.

use std::net::{IpAddr, SocketAddr};

/// One client's control connection. Cheap to clone — both variants are handles.
#[derive(Clone)]
pub(crate) enum SessionLink {
    /// The native plane: `punktfunk/1` over quinn.
    Quic(quinn::Connection),
    /// The browser plane: the same protocol over one WebTransport session.
    Web(wtransport::Connection),
}

impl SessionLink {
    /// The QUIC connection underneath, which both carriers have.
    fn quic(&self) -> &quinn::Connection {
        match self {
            SessionLink::Quic(c) => c,
            SessionLink::Web(c) => c.quic_connection(),
        }
    }

    /// Unreliable datagram: audio, cursor, rumble, HID out. Lossy by contract — a refusal is a
    /// drop to count, never an error to unwind. `false` when the stack would not take it.
    pub(crate) fn send_datagram(&self, payload: Vec<u8>) -> bool {
        match self {
            SessionLink::Quic(c) => c.send_datagram(payload.into()).is_ok(),
            // Not the quinn connection: a WebTransport datagram carries a session-id prefix, so
            // it has to go through the layer that writes one.
            SessionLink::Web(c) => c.send_datagram(payload).is_ok(),
        }
    }

    /// Largest datagram this path will carry. Lower on the browser plane — HTTP/3 framing comes
    /// out of the same budget — which is why callers must ask rather than assume 1500-MTU maths.
    pub(crate) fn max_datagram_size(&self) -> Option<usize> {
        match self {
            SessionLink::Quic(c) => c.max_datagram_size(),
            SessionLink::Web(c) => c.max_datagram_size(),
        }
    }

    pub(crate) fn remote_address(&self) -> SocketAddr {
        self.quic().remote_address()
    }

    /// Local address the connection arrived on, for binding a data socket on the same NIC.
    pub(crate) fn local_ip(&self) -> Option<IpAddr> {
        self.quic().local_ip()
    }

    /// Path MTU as the stack currently believes it.
    pub(crate) fn current_mtu(&self) -> u16 {
        self.quic().stats().path.current_mtu
    }

    /// `Some` once the connection has ended, without awaiting.
    pub(crate) fn close_reason(&self) -> Option<String> {
        self.quic().close_reason().map(|e| e.to_string())
    }

    pub(crate) fn close(&self, code: u32, reason: &[u8]) {
        self.quic().close(code.into(), reason);
    }

    /// Resolves when the peer is gone.
    pub(crate) async fn closed(&self) -> String {
        self.quic().closed().await.to_string()
    }

    /// Whether a browser is on the other end. For the few decisions that really are about the
    /// carrier: there is no second UDP plane to punch, and the capabilities that ride quinn
    /// streams are not on offer.
    pub(crate) fn is_web(&self) -> bool {
        matches!(self, SessionLink::Web(_))
    }
}

impl From<quinn::Connection> for SessionLink {
    fn from(c: quinn::Connection) -> SessionLink {
        SessionLink::Quic(c)
    }
}

impl From<wtransport::Connection> for SessionLink {
    fn from(c: wtransport::Connection) -> SessionLink {
        SessionLink::Web(c)
    }
}
