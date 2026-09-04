//! [`Transport`] over one browser's WebTransport datagrams, host side.
//!
//! The session pump is synchronous and runs on its own thread — `send_gso` is called from the
//! pacer, `recv_batch` from the read side — while `wtransport`'s connection is async and lives on
//! the tokio runtime. This is the seam between them, and it is deliberately thin: a bounded
//! inbound queue the connection task fills, and a send that hands straight to
//! `Connection::send_datagram`, which is itself non-blocking.
//!
//! Nothing here knows about video. The whole point of `Transport` being a trait is that
//! `punktfunk_core::session::Session` cannot tell this from a UDP socket.

use punktfunk_core::transport::Transport;
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use wtransport::Connection;

/// Inbound datagrams waiting for the pump. Bounded: a pump that has stopped draining must lose
/// packets rather than grow, exactly as a full socket buffer would. FEC covers the gap.
const INBOX_CAP: usize = 512;

/// Shared between the connection task (which pushes) and the session pump (which pops).
#[derive(Default)]
pub(crate) struct Inbox {
    queue: Mutex<VecDeque<Vec<u8>>>,
}

impl Inbox {
    /// Called from the connection task for each datagram the browser sent.
    pub(crate) fn push(&self, datagram: Vec<u8>) {
        let Ok(mut q) = self.queue.lock() else { return };
        if q.len() >= INBOX_CAP {
            q.pop_front();
        }
        q.push_back(datagram);
    }
}

/// The pump's view of one browser connection.
pub(crate) struct WebTransportPlane {
    conn: Connection,
    inbox: Arc<Inbox>,
}

impl WebTransportPlane {
    pub(crate) fn new(conn: Connection, inbox: Arc<Inbox>) -> WebTransportPlane {
        WebTransportPlane { conn, inbox }
    }
}

impl Transport for WebTransportPlane {
    /// `Ok(false)` for a datagram the connection would not take — the same lossy contract a full
    /// UDP send buffer has, which the caller counts and FEC covers.
    fn send(&self, packet: &[u8]) -> io::Result<bool> {
        Ok(self.conn.send_datagram(packet).is_ok())
    }

    /// `Ok(None)` when nothing is queued: the non-blocking contract the pump relies on.
    fn recv(&self) -> io::Result<Option<Vec<u8>>> {
        let Ok(mut q) = self.inbox.queue.lock() else {
            return Ok(None);
        };
        Ok(q.pop_front())
    }

    fn recv_batch(&self, out: &mut [Vec<u8>], lens: &mut [usize]) -> io::Result<usize> {
        let Ok(mut q) = self.inbox.queue.lock() else {
            return Ok(0);
        };
        let mut filled = 0;
        while filled < out.len() {
            let Some(datagram) = q.pop_front() else { break };
            let n = datagram.len().min(out[filled].len());
            out[filled][..n].copy_from_slice(&datagram[..n]);
            lens[filled] = n;
            filled += 1;
        }
        Ok(filled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_inbox_drops_the_oldest_rather_than_growing() {
        let inbox = Inbox::default();
        for i in 0..INBOX_CAP + 10 {
            inbox.push(vec![i as u8]);
        }
        let q = inbox.queue.lock().unwrap();
        assert_eq!(q.len(), INBOX_CAP, "bounded, whatever the sender does");
        // The oldest went, so what is left is the most recent window — the useful end for a
        // live stream, where a stale datagram is worth less than a fresh one.
        assert_eq!(q.front().map(|d| d[0]), Some(10u8 as u8));
    }
}
