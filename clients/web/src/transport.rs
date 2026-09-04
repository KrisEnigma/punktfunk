//! [`Transport`] over WebTransport datagrams (`design/web-client-implementation-plan.md` §3).
//!
//! The hot path gets a ring, not an allocation. At roughly 4–5k datagrams a second for a 50 Mbps
//! stream, a `Vec` per packet is garbage the collector on the other side of the boundary has to
//! chase. Instead Rust owns a fixed ring of slots, hands JavaScript a pointer into wasm memory,
//! and JavaScript writes each datagram straight in with `HEAPU8.set`. One memcpy, no allocation,
//! nothing per-packet for either runtime to free.
//!
//! Two copies happen in total — JS into the ring, ring into the caller's buffer in
//! [`Transport::recv_batch`] — because the trait hands out caller-owned buffers. That is the
//! shape core's session pump wants, and neither copy allocates.
//!
//! Single-threaded by construction: emscripten runs this on one thread, and every entry point
//! here is called from it. The ring therefore lives in a `thread_local` behind `RefCell`, and the
//! [`Transport`] implementor is a zero-sized handle — which is what makes `Send + Sync` honest
//! rather than an `unsafe impl` promising something the platform cannot break anyway.

use punktfunk_core::packet::MAX_DATAGRAM_BYTES;
use punktfunk_core::transport::Transport;
use std::cell::RefCell;
use std::io;

/// Slots in the receive ring. 256 × 9216 B ≈ 2.4 MB, which at 5k datagrams/s is ~50 ms of buffer —
/// far more than a `recv_batch` cadence needs, and the point at which a stall should be dropping
/// packets rather than growing memory.
const RING_SLOTS: usize = 256;

/// One slot holds any datagram the protocol allows.
const SLOT_BYTES: usize = MAX_DATAGRAM_BYTES;

/// Fixed slots, written by JavaScript and drained by the session pump.
struct Ring {
    /// `RING_SLOTS * SLOT_BYTES`, allocated once and never moved. JavaScript is handed offsets
    /// into this, so it must not be reallocated while the client runs.
    buf: Vec<u8>,
    /// Bytes written into each slot, valid for indices between `tail` and `head`.
    lens: [u32; RING_SLOTS],
    /// Next slot JavaScript will fill.
    head: usize,
    /// Next slot the pump will read.
    tail: usize,
    /// Datagrams dropped because the pump did not drain in time. FEC covers the gap; the count is
    /// what says whether it had to.
    dropped: u64,
}

impl Ring {
    fn new() -> Ring {
        Ring {
            buf: vec![0; RING_SLOTS * SLOT_BYTES],
            lens: [0; RING_SLOTS],
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }

    fn is_full(&self) -> bool {
        (self.head + 1) % RING_SLOTS == self.tail
    }

    fn pop(&mut self) -> Option<(usize, usize)> {
        if self.tail == self.head {
            return None;
        }
        let slot = self.tail;
        let len = self.lens[slot] as usize;
        self.tail = (self.tail + 1) % RING_SLOTS;
        Some((slot * SLOT_BYTES, len))
    }
}

thread_local! {
    static RING: RefCell<Ring> = RefCell::new(Ring::new());
}

unsafe extern "C" {
    /// Hand one datagram to the browser's WebTransport writer. `1` when it was queued. Defined in
    /// `web/pf-glue.js`, which is the only place a browser object is named.
    fn pf_wt_send(ptr: *const u8, len: u32) -> i32;
}

/// Base of the receive ring, for `HEAPU8.set(bytes, base + slot * stride)`.
#[unsafe(no_mangle)]
pub extern "C" fn pf_rx_base() -> *const u8 {
    RING.with(|r| r.borrow().buf.as_ptr())
}

/// Bytes per slot — the stride JavaScript multiplies a slot index by.
#[unsafe(no_mangle)]
pub extern "C" fn pf_rx_stride() -> u32 {
    SLOT_BYTES as u32
}

/// Claim the slot to write the next datagram into, or `-1` when the ring is full.
///
/// Full means the pump is behind. Dropping here is the right answer and the protocol's: FEC and
/// the next keyframe recover a lost datagram, where growing a queue only adds latency to every
/// packet behind it.
#[unsafe(no_mangle)]
pub extern "C" fn pf_rx_claim() -> i32 {
    RING.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_full() {
            r.dropped += 1;
            return -1;
        }
        r.head as i32
    })
}

/// Publish the slot `pf_rx_claim` handed out. `len` is clamped to the slot.
#[unsafe(no_mangle)]
pub extern "C" fn pf_rx_commit(slot: i32, len: u32) {
    RING.with(|r| {
        let mut r = r.borrow_mut();
        let Ok(slot) = usize::try_from(slot) else {
            return;
        };
        if slot >= RING_SLOTS || slot != r.head {
            return;
        }
        r.lens[slot] = len.min(SLOT_BYTES as u32);
        r.head = (r.head + 1) % RING_SLOTS;
    });
}

/// Datagrams dropped because the ring was full. Read by the page for the §5.4 measurement.
#[unsafe(no_mangle)]
pub extern "C" fn pf_rx_dropped() -> u32 {
    RING.with(|r| u32::try_from(r.borrow().dropped).unwrap_or(u32::MAX))
}

/// The browser's datagram plane, as core's session pump sees it.
///
/// Zero-sized: the ring is a `thread_local`, so there is no state to share and nothing for
/// `Send + Sync` to lie about.
#[derive(Clone, Copy, Default)]
pub struct WebTransportDatagrams;

impl Transport for WebTransportDatagrams {
    fn send(&self, packet: &[u8]) -> io::Result<bool> {
        if packet.len() > SLOT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "datagram over the protocol's maximum",
            ));
        }
        // SAFETY: `pf_wt_send` reads `len` bytes at `ptr` and returns before this call does; the
        // slice outlives it. It writes nothing through the pointer.
        let queued = unsafe { pf_wt_send(packet.as_ptr(), packet.len() as u32) };
        Ok(queued == 1)
    }

    /// `Ok(None)` when the ring is empty — the contract the pump relies on to never block.
    fn recv(&self) -> io::Result<Option<Vec<u8>>> {
        RING.with(|r| {
            let mut r = r.borrow_mut();
            match r.pop() {
                Some((offset, len)) => Ok(Some(r.buf[offset..offset + len].to_vec())),
                None => Ok(None),
            }
        })
    }

    /// The path the pump actually takes: drain into caller-owned buffers, no allocation.
    fn recv_batch(&self, out: &mut [Vec<u8>], lens: &mut [usize]) -> io::Result<usize> {
        RING.with(|r| {
            let mut r = r.borrow_mut();
            let mut filled = 0;
            while filled < out.len() {
                let Some((offset, len)) = r.pop() else { break };
                let n = len.min(out[filled].len());
                out[filled][..n].copy_from_slice(&r.buf[offset..offset + n]);
                lens[filled] = n;
                filled += 1;
            }
            Ok(filled)
        })
    }
}

/// Send `count` datagrams of `size` bytes through the seam, returning how many the browser
/// queued. Each carries its sequence number in the first four bytes so the page can tell a
/// round-trip from an echo of something older.
///
/// This exists for plan §5.4 — "datagram crossing cost at 4–5k/s, the ring's actual overhead" —
/// which is a number Phase 2 has to produce before the tier framing survives contact. It is also
/// what makes the whole path live before the session pump lands on top of it.
#[unsafe(no_mangle)]
pub extern "C" fn pf_net_blast(count: u32, size: u32) -> u32 {
    let size = (size as usize).clamp(4, SLOT_BYTES);
    let mut packet = vec![0u8; size];
    let mut queued = 0;
    for seq in 0..count {
        packet[..4].copy_from_slice(&seq.to_le_bytes());
        match WebTransportDatagrams.send(&packet) {
            Ok(true) => queued += 1,
            Ok(false) => break,
            Err(_) => break,
        }
    }
    queued
}

/// Drain whatever the browser has written into the ring, the way the pump will. Returns how many
/// datagrams came out, so the page can watch the echo come back without touching the ring itself.
#[unsafe(no_mangle)]
pub extern "C" fn pf_net_drain() -> u32 {
    // The batch the pump would use — sized to the ring so one call empties a burst.
    thread_local! {
        static OUT: RefCell<(Vec<Vec<u8>>, Vec<usize>)> = RefCell::new((
            vec![vec![0u8; SLOT_BYTES]; 32],
            vec![0usize; 32],
        ));
    }
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        let (out, lens) = &mut *o;
        let mut total = 0;
        loop {
            match WebTransportDatagrams.recv_batch(out, lens) {
                Ok(0) | Err(_) => break,
                Ok(n) => total += n as u32,
            }
        }
        total
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain() {
        let t = WebTransportDatagrams;
        while t.recv().unwrap().is_some() {}
        RING.with(|r| r.borrow_mut().dropped = 0);
    }

    /// Write through the same entry points JavaScript uses, then read through the trait.
    fn push(bytes: &[u8]) -> bool {
        let slot = pf_rx_claim();
        if slot < 0 {
            return false;
        }
        RING.with(|r| {
            let mut r = r.borrow_mut();
            let offset = slot as usize * SLOT_BYTES;
            r.buf[offset..offset + bytes.len()].copy_from_slice(bytes);
        });
        pf_rx_commit(slot, bytes.len() as u32);
        true
    }

    #[test]
    fn datagrams_come_back_in_order_and_the_ring_wraps() {
        drain();
        let t = WebTransportDatagrams;
        assert!(t.recv().unwrap().is_none(), "empty ring yields Ok(None)");

        // More than one lap, so the wrap is exercised rather than assumed.
        for lap in 0..3u8 {
            for i in 0..(RING_SLOTS - 1) as u8 {
                assert!(push(&[lap, i]), "slot should be free after draining");
            }
            for i in 0..(RING_SLOTS - 1) as u8 {
                assert_eq!(t.recv().unwrap().as_deref(), Some(&[lap, i][..]));
            }
        }
        assert!(t.recv().unwrap().is_none());
    }

    #[test]
    fn a_full_ring_drops_rather_than_grows() {
        drain();
        for i in 0..(RING_SLOTS - 1) {
            assert!(push(&[i as u8]), "slot {i} should be free");
        }
        assert!(!push(&[0xff]), "the ring is full and must refuse");
        assert_eq!(pf_rx_dropped(), 1, "a refusal is counted, not silent");
    }

    #[test]
    fn recv_batch_drains_without_allocating_and_truncates_to_the_caller() {
        drain();
        for i in 0..4u8 {
            assert!(push(&[i; 8]));
        }
        let t = WebTransportDatagrams;
        let mut out = vec![vec![0u8; 8], vec![0u8; 8], vec![0u8; 3]];
        let mut lens = [0usize; 3];
        assert_eq!(t.recv_batch(&mut out, &mut lens).unwrap(), 3);
        assert_eq!(&out[0][..lens[0]], &[0u8; 8]);
        assert_eq!(&out[1][..lens[1]], &[1u8; 8]);
        // A short caller buffer truncates rather than overruns.
        assert_eq!(lens[2], 3);
        assert_eq!(&out[2][..3], &[2u8; 3]);
        // The fourth is still queued: a batch stops at the caller's capacity.
        assert_eq!(t.recv().unwrap().as_deref(), Some(&[3u8; 8][..]));
    }

    #[test]
    fn commit_ignores_a_slot_it_did_not_hand_out() {
        drain();
        let slot = pf_rx_claim();
        assert!(slot >= 0);
        // Out of range, and a valid-but-wrong slot: both must leave the ring where it was.
        pf_rx_commit(RING_SLOTS as i32 + 5, 4);
        pf_rx_commit(slot + 1, 4);
        pf_rx_commit(-1, 4);
        let t = WebTransportDatagrams;
        assert!(
            t.recv().unwrap().is_none(),
            "nothing was published, so nothing is readable"
        );
    }
}
