//! Thread + handle ownership for the driver's background workers.
//!
//! Two long-lived threads live in this driver: the cursor query→publish loop and the swap-chain
//! drain loop. Both used to hand raw `HANDLE` values to their thread and let the THREAD close
//! them at exit, which puts a handle's lifetime somewhere no owner can observe: a caller that
//! copied the value out under the registry lock can hand a closed — and by then possibly reused
//! — handle to a DDI. Everything here states the opposite rule: a handle or mapping belongs to
//! one value, its `Drop` closes it exactly once, and a thread only ever borrows what its owner
//! outlives.
//!
//! [`Worker`] owns the stop event and the join handle, so dropping it signals, joins, and only
//! then closes. [`OwnedHandle`] and [`OwnedView`] are the RAII wrappers for `CloseHandle` and
//! `MapViewOfFile`/`UnmapViewOfFile`.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

/// Carries a raw handle or pointer across a thread spawn.
///
/// Win32 and IddCx handles are `*mut c_void` and so `!Send`, but they are plain process-wide
/// values whose lifetime the framework — not the compiler — governs. Rebind the WHOLE wrapper
/// inside the closure (`let x = x;`) before touching `.0`: disjoint closure captures would
/// otherwise capture the `!Send` field itself and defeat the wrapper.
pub struct Sendable<T>(pub T);
// SAFETY: see the type doc — the wrapped raw value has one user at a time, and the owner that
// handed it over outlives the thread borrowing it.
unsafe impl<T> Send for Sendable<T> {}

/// A Win32 handle this process owns; `Drop` closes it exactly once.
pub struct OwnedHandle(HANDLE);
// SAFETY: a Win32 handle is a process-wide token, not thread-affine; this wrapper hands out only
// borrowed copies and is the handle's sole closer, so moving it between threads is sound.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    /// An unnamed event owned by the returned value. `manual_reset` keeps it signalled until
    /// something resets it (a stop flag); auto-reset releases one waiter per signal (a data
    /// event). `None` if the OS refused, which every caller treats as "run without it".
    pub fn event(manual_reset: bool) -> Option<Self> {
        // SAFETY: plain event creation — unsignalled, unnamed, no security descriptor.
        let h = unsafe { CreateEventW(None, manual_reset, false, None) }.ok()?;
        Some(Self(h))
    }

    /// Adopt `h`: from here on this value alone decides when the handle closes.
    ///
    /// # Safety
    /// `h` must be a live handle this process owns, and no other owner may close it.
    pub unsafe fn from_raw(h: HANDLE) -> Self {
        Self(h)
    }

    /// A borrowed copy for a DDI call or a thread wait — valid only while `self` lives.
    pub fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we adopted this handle and only ever hand out borrowed copies of it, so this
        // is its sole close.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// A mapped section view and the mapping handle behind it.
///
/// One value because the two have one lifetime: `Drop` unmaps the view, then field order drops
/// `mapping` and closes it. Move it into the thread that reads the view and the mapping is
/// released when that thread returns — never earlier, and never twice.
pub struct OwnedView {
    base: *mut core::ffi::c_void,
    /// Never read: held so it closes AFTER the unmap in `Drop` (field order).
    _mapping: OwnedHandle,
}

impl OwnedView {
    /// Adopt the view at `base` over `mapping`.
    ///
    /// # Safety
    /// `base` must be a live `MapViewOfFile` result for `mapping`, mapped exactly once, and
    /// `mapping` must be a section handle this process owns.
    pub unsafe fn from_raw(base: *mut core::ffi::c_void, mapping: OwnedHandle) -> Self {
        Self {
            base,
            _mapping: mapping,
        }
    }

    /// The view's base address — valid only while `self` lives.
    pub fn base(&self) -> *mut core::ffi::c_void {
        self.base
    }
}

impl Drop for OwnedView {
    fn drop(&mut self) {
        // SAFETY: our own single mapping of `self.base`; the mapping handle closes immediately
        // after, when the `mapping` field drops.
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base });
        }
    }
}

/// A background thread and the manual-reset event that stops it.
///
/// [`spawn`](Self::spawn) hands the thread a BORROWED stop handle; this value closes it only
/// after the join, so the thread can never wait on a handle its owner already closed. Dropping
/// the `Worker` is the only way the thread ends, which makes the owner's lifetime the thread's
/// lifetime — the property every caller reasons from.
pub struct Worker {
    stop: OwnedHandle,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    /// Start `body` on a thread called `name`, passing it the stop event to wait on.
    ///
    /// `None` when the event or the thread could not be created. On that path `body` is
    /// dropped, so whatever it captured — a mapping, a view — is released by its own `Drop`
    /// instead of leaking into the host process's handle table; a caller that must undo more
    /// than that gets the `None` to do it with.
    pub fn spawn(name: &str, body: impl FnOnce(HANDLE) + Send + 'static) -> Option<Worker> {
        let stop = OwnedHandle::event(true)?;
        let raw = Sendable(stop.as_raw());
        let join = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let raw = raw; // capture the wrapper, not the `!Send` handle inside it
                body(raw.0);
            })
            .ok()?;
        Some(Worker {
            stop,
            join: Some(join),
        })
    }

    /// Signal the stop event and join the thread. Idempotent — the stop handle stays open until
    /// `self` drops, which is strictly after this join.
    pub fn stop(&mut self) {
        let Some(join) = self.join.take() else {
            return;
        };
        let name = join.thread().name().unwrap_or("?").to_string();
        // SAFETY: our own manual-reset event, alive until `self` drops (after this join).
        unsafe {
            let _ = SetEvent(self.stop.as_raw());
        }
        let started = Instant::now();
        let _ = join.join();
        let took = started.elapsed();
        if took > Duration::from_millis(250) {
            dbglog!("[pf-vd] worker {name} join took {} ms", took.as_millis());
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}
