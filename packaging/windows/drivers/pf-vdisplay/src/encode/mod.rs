//! In-driver encode (`--features driver-encode`, design/windows-video-plane-overhaul.md §2):
//! the encoder backends opened and driven inside WUDFHost. [`convert`] bridges the driver's
//! `windows` 0.58 objects to the backends' 0.62 and owns the input targets a pool slot is
//! written in; [`section`] is the host's AU section and the session installed on a monitor;
//! [`thread`] opens a backend, reports, and publishes. The S5 probe (`encode_probe.rs`) is a
//! thin client of the same pieces. [`set_encode`] is the control-plane verb.

pub mod convert;
pub mod drive;
pub mod pool;
pub mod section;
pub mod thread;

use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::time::Duration;

use pf_driver_proto::encode::{self as wire, SetEncodeReply, SetEncodeRequest};
use wdk_sys::NTSTATUS;

use self::section::{AuSection, EncodeSession};
use self::thread::{EncodeThread, ThreadCtx, fail_reply};
use crate::{STATUS_INVALID_PARAMETER, STATUS_NOT_FOUND, registry};

/// How long `SET_ENCODE` waits for the thread's open. NVENC opens in under a millisecond, AMF
/// in tens; a backend that takes seconds is stuck in a driver, and the host's watchdog window
/// (10 s) must still see the IOCTL return.
const OPEN_BOUND: Duration = Duration::from_secs(5);

/// Backend names in the order the addresses below pin them, for the load-time log.
pub fn backends_linked() -> &'static [&'static str] {
    &["nvenc", "amf", "qsv", "pyrowave", "convert"]
}

/// `IOCTL_SET_ENCODE`: open an encoder for the monitor with `req.target_id` on the AU section
/// the host delivered, replacing any session it already has.
///
/// `Err` is an NTSTATUS for a malformed request or an unknown target, with nothing adopted.
/// `Ok` completes the IOCTL successfully whatever `status` says; from the map on, the driver
/// owns the handles (`AuSection`). The open runs on the new encode thread and this call
/// waits [`OPEN_BOUND`] for its reply. A displaced session's thread stops with no lock held.
pub fn set_encode(req: &SetEncodeRequest) -> Result<SetEncodeReply, NTSTATUS> {
    let listed = req.backends[0] != 0 && req.backends.iter().all(|&b| b <= 4);
    let valid = req.target_id != 0
        && req.section != 0
        && req.event != 0
        && (1..=4).contains(&req.codec)
        && req.width != 0
        && req.height != 0
        && listed;
    if !valid {
        return Err(STATUS_INVALID_PARAMETER);
    }
    let Some(monitor) = registry::find(|m| m.target_id() == req.target_id) else {
        return Err(STATUS_NOT_FOUND);
    };
    let section = match AuSection::map(req.section, req.event, req.section_bytes) {
        Ok(s) => s,
        Err(_) => return Err(STATUS_INVALID_PARAMETER),
    };
    let Some(luid) = monitor.render_luid() else {
        return Ok(fail_reply(wire::SET_ENCODE_NO_DEVICE, (-5, "noswap")));
    };
    let Some(device) = crate::direct_3d_device::pooled_device(luid) else {
        return Ok(fail_reply(wire::SET_ENCODE_NO_DEVICE, (-5, "device")));
    };
    let generation = monitor.next_encode_generation();
    let session = Arc::new(EncodeSession::new(*req, section, generation));
    let (tx, rx) = sync_channel(1);
    let Some(thread) = EncodeThread::spawn(ThreadCtx {
        session: session.clone(),
        monitor: Arc::downgrade(&monitor),
        device,
        opened: tx,
    }) else {
        return Ok(fail_reply(wire::SET_ENCODE_THREAD, (-7, "spawn")));
    };
    let reply = match rx.recv_timeout(OPEN_BOUND) {
        Ok(reply) => reply,
        Err(RecvTimeoutError::Timeout) => fail_reply(wire::SET_ENCODE_TIMEOUT, (-3, "open")),
        Err(RecvTimeoutError::Disconnected) => fail_reply(wire::SET_ENCODE_THREAD, (-7, "exit")),
    };
    if reply.status != wire::SET_ENCODE_OK {
        // The thread has exited (or is stuck in the open and gets detached); the session's
        // handles close with the last `Arc`.
        thread.stop(&session.section);
        return Ok(reply);
    }
    drop(session.set_thread(thread));
    match monitor.set_encode(session) {
        Ok(displaced) => {
            if let Some(old) = displaced {
                old.stop();
            }
            Ok(reply)
        }
        Err(session) => {
            // Torn down while opening: stop what was just started, the handles close with it.
            session.stop();
            Ok(fail_reply(wire::SET_ENCODE_NO_MONITOR, (-6, "gone")))
        }
    }
}

// `nvidia-video-codec-sdk` (feature `ci-check`) links no import library, and the DLL still pulls
// its `EncodeAPI` object, which names these two entry points. The NVENC backend resolves both
// from `nvEncodeAPI64.dll` at runtime and never calls these; they only satisfy the linker.
// Not `pub`: internal linkage only, nothing is exported from the DLL.
#[unsafe(no_mangle)]
extern "C" fn NvEncodeAPICreateInstance(_list: *mut core::ffi::c_void) -> u32 {
    // NV_ENC_ERR_NO_ENCODE_DEVICE
    1
}

#[unsafe(no_mangle)]
extern "C" fn NvEncodeAPIGetMaxSupportedVersion(_version: *mut u32) -> u32 {
    // NV_ENC_ERR_NO_ENCODE_DEVICE
    1
}
