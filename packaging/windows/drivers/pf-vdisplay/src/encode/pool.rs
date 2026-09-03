//! The pool: three slots per monitor in the opened backend's input format, filled by the drain
//! worker's fused pass ([`Pool::offer`]) inside the acquire window and drained by the encode
//! thread. The monitor owns it, not the session: between sessions the newest frame keeps
//! landing in it, so a new `SET_ENCODE` on an idle desktop encodes the retained slot as its
//! first IDR and needs no compose. A pool built for another device epoch, size or format is
//! replaced by the next session; nothing rebuilds in place.
//!
//! [`Attached`] is the drain worker's cached view of the monitor's pool and session,
//! re-read only when `Monitor::encode_gen` moved, so the steady state takes no lock.

use std::collections::VecDeque;
use std::mem::offset_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pf_driver_proto::encode::au::AuHeader;
use pf_frame::CapturedFrame;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::System::Threading::SetEvent;
use windows62::Win32::Graphics::Direct3D11 as d3d;

use super::convert::{Fail, InputKind, Targets, bridge, source_format};
use super::section::EncodeSession;
use super::thread::qpc_now;
use crate::cursor_cell::CursorCell;
use crate::direct_3d_device::Direct3DDevice;
use crate::monitor::Monitor;
use crate::registry::lock;
use crate::worker::OwnedHandle;

/// Three slots: the encoder holds up to two in flight while the drain worker fills one.
pub const SLOTS: usize = 3;

/// What [`Pool::offer`] did with a surface.
pub enum Offer {
    /// In a slot; the frame's source sequence.
    Taken(u64),
    /// Counted; the new drop total.
    Dropped(u64),
    /// Not this pool's surface (device epoch, size or format) — nothing counted.
    Refused,
}

struct State {
    targets: Targets,
    free: Vec<usize>,
    /// `(slot, PresentDisplayQPCTime, source_seq)` in acquire order.
    full: VecDeque<(usize, u64, u64)>,
    /// Handed to the encoder, AU still owed.
    encoding: Vec<usize>,
    /// An encode thread is consuming. Without one the newest frame recycles the oldest full
    /// slot, so the retained image is always the current desktop.
    live: bool,
}

/// One monitor's pool. See the module docs.
pub struct Pool {
    device_epoch: u32,
    width: u32,
    height: u32,
    kind: InputKind,
    source_format: DXGI_FORMAT,
    state: Mutex<State>,
    /// Auto-reset, signalled once per filled slot.
    event: OwnedHandle,
    /// The monitor's source sequence, advanced per frame handed to the pool.
    source_seq: Arc<AtomicU64>,
    /// Frames dropped at the pool or skipped by the encode thread for a full slot table.
    dropped: AtomicU64,
    /// The monitor's cursor: read at every pass for the blend decision, at every frame for
    /// the shape.
    cursor: Arc<CursorCell>,
}

impl Pool {
    /// Build the targets for `kind` at `size` on `device`.
    pub fn build(
        device: &Direct3DDevice,
        kind: InputKind,
        size: (u32, u32),
        source_seq: Arc<AtomicU64>,
        cursor: Arc<CursorCell>,
    ) -> Result<Arc<Self>, Fail> {
        let dev62: d3d::ID3D11Device = bridge(&device.device)?;
        let ctx62: d3d::ID3D11DeviceContext = bridge(&device.device_context)?;
        let targets = Targets::new(kind, &dev62, &ctx62, size, SLOTS)?;
        let event = OwnedHandle::event(false).ok_or((-2, "event"))?;
        Ok(Arc::new(Self {
            device_epoch: device.epoch(),
            width: size.0,
            height: size.1,
            kind,
            source_format: DXGI_FORMAT(source_format(kind).0),
            state: Mutex::new(State {
                targets,
                free: (0..SLOTS).collect(),
                full: VecDeque::new(),
                encoding: Vec::new(),
                live: false,
            }),
            event,
            source_seq,
            dropped: AtomicU64::new(0),
            cursor,
        }))
    }

    /// Whether a session opening `kind` at `size` on `device` can reuse this pool — and with
    /// it the retained slot.
    pub fn matches(&self, device: &Direct3DDevice, kind: InputKind, size: (u32, u32)) -> bool {
        self.device_epoch == device.epoch()
            && self.kind == kind
            && (self.width, self.height) == size
    }

    /// The filled-slot event, for the encode thread's wait.
    pub fn event(&self) -> HANDLE {
        self.event.as_raw()
    }

    /// The drain worker's pass: one GPU pass from the acquired surface into a free slot, then
    /// the event. Never blocks — a contended lock is a counted drop, as is a full pool with a
    /// live consumer.
    pub fn offer(&self, device: &Direct3DDevice, tex: &ID3D11Texture2D, qpc: u64) -> Offer {
        if device.epoch() != self.device_epoch {
            return Offer::Refused;
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `tex` is the live acquired surface; `desc` is a valid local out-param.
        unsafe { tex.GetDesc(&mut desc) };
        if (desc.Width, desc.Height, desc.Format) != (self.width, self.height, self.source_format) {
            return Offer::Refused;
        }
        let Ok(mut st) = self.state.try_lock() else {
            return self.drop_one();
        };
        let recycled = (!st.live)
            .then(|| st.full.pop_front().map(|f| f.0))
            .flatten();
        let Some(i) = st.free.pop().or(recycled) else {
            return self.drop_one();
        };
        let blend = self.cursor.blend.load(Ordering::Relaxed);
        let passed =
            bridge::<d3d::ID3D11Texture2D>(tex).and_then(|src| st.targets.pass(&src, i, blend));
        if passed.is_err() {
            st.free.push(i);
            return self.drop_one();
        }
        let seq = self.source_seq.fetch_add(1, Ordering::Relaxed) + 1;
        st.full.push_back((i, qpc, seq));
        drop(st);
        // SAFETY: our own event, alive as long as `self`.
        unsafe {
            let _ = SetEvent(self.event.as_raw());
        }
        Offer::Taken(seq)
    }

    /// One more frame dropped; the new total, for the header.
    pub fn drop_one(&self) -> Offer {
        Offer::Dropped(self.dropped.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// The encode thread starts (`true`: only the newest full slot is kept, the stash) or
    /// stops (`false`: the pool goes back to recycling).
    pub fn set_live(&self, live: bool) {
        let mut st = lock(&self.state);
        st.live = live;
        if live {
            while st.full.len() > 1 {
                let (i, ..) = st.full.pop_front().expect("len > 1");
                st.free.push(i);
            }
        }
    }

    /// The oldest full slot, now the encoder's.
    pub fn take_full(&self) -> Option<(usize, u64, u64)> {
        let mut st = lock(&self.state);
        let f = st.full.pop_front()?;
        st.encoding.push(f.0);
        Some(f)
    }

    /// Hand a slot back, whether its AU was published or it was skipped.
    pub fn release(&self, slot: usize) {
        let mut st = lock(&self.state);
        st.encoding.retain(|&s| s != slot);
        if !st.free.contains(&slot) {
            st.free.push(slot);
        }
    }

    /// Every slot a departed encoder still held, freed — after a detach, whose encoder may be
    /// mid-read on the GPU; a torn first frame is the price of not waiting for it.
    pub fn reclaim(&self) {
        let mut st = lock(&self.state);
        let held = core::mem::take(&mut st.encoding);
        for slot in held {
            if !st.free.contains(&slot) {
                st.free.push(slot);
            }
        }
    }

    /// Wake the encode thread without a frame — a control op landed in its mailbox.
    pub fn wake(&self) {
        // SAFETY: our own event, alive as long as `self`.
        unsafe {
            let _ = SetEvent(self.event.as_raw());
        }
    }

    /// Wrap slot `slot` as the frame `submit` takes, the pointer blended in when the client
    /// draws none (the planar pair signals its fence here).
    pub fn frame(&self, slot: usize, pts_ns: u64) -> Result<CapturedFrame, Fail> {
        let cursor = self.cursor.to_blend();
        lock(&self.state).targets.frame(slot, pts_ns, cursor)
    }
}

/// The drain worker's view of its monitor's pool and session (see the module docs).
pub struct Attached {
    pool: Option<Arc<Pool>>,
    session: Option<Arc<EncodeSession>>,
    seen_gen: u32,
}

impl Attached {
    pub fn new() -> Self {
        Self {
            pool: None,
            session: None,
            seen_gen: u32::MAX,
        }
    }

    /// Re-read the monitor's slots when its encode generation moved since the last pass.
    pub fn refresh(&mut self, monitor: &Monitor) {
        let generation = monitor.encode_gen.load(Ordering::Acquire);
        if generation == self.seen_gen {
            return;
        }
        self.seen_gen = generation;
        self.pool = monitor.pool();
        self.session = monitor.encode();
    }

    /// The hook, per acquired surface (see [`Pool::offer`]). A pool on another device epoch
    /// marks the session stale — logged once — and the frame goes nowhere.
    pub fn offer(&self, device: &Direct3DDevice, tex: &ID3D11Texture2D, display_qpc: u64) {
        let Some(pool) = &self.pool else {
            return;
        };
        let session = self.session.as_deref();
        match pool.offer(device, tex, display_qpc) {
            Offer::Dropped(n) => {
                if let Some(s) = session {
                    s.section.store_u64(offset_of!(AuHeader, dropped_total), n);
                }
            }
            Offer::Taken(seq) => {
                if let Some(s) = session {
                    s.section.store_u64(offset_of!(AuHeader, source_seq), seq);
                }
            }
            Offer::Refused => {
                if let Some(s) = session
                    && !s.stale.swap(true, Ordering::AcqRel)
                {
                    dbglog!(
                        "[pf-vd] encode: pool cannot take the surface (device epoch {} vs pool {}) — session stale until the next SET_ENCODE",
                        device.epoch(),
                        pool.device_epoch
                    );
                }
            }
        }
    }

    /// The drain heartbeat, stamped after `FinishedProcessingFrame` — never inside the window.
    pub fn note_drain(&self) {
        if let Some(s) = &self.session {
            s.section
                .store_u64(offset_of!(AuHeader, drain_heartbeat_qpc), qpc_now());
        }
    }
}
