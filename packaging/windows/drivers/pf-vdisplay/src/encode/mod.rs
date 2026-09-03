//! In-driver encode (`--features driver-encode`, design/windows-video-plane-overhaul.md §2):
//! the encoder backends opened and driven inside WUDFHost. [`convert`] bridges the driver's
//! `windows` 0.58 objects to the backends' 0.62 and owns the input targets a pool slot is
//! written in; [`thread`] opens a backend and keeps the QPC clock the frames are stamped with.
//! The S5 probe (`encode_probe.rs`) is a thin client of the same pieces.

pub mod convert;
pub mod thread;

/// Backend names in the order the addresses below pin them, for the load-time log.
pub fn backends_linked() -> &'static [&'static str] {
    &["nvenc", "amf", "qsv", "pyrowave", "convert"]
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
