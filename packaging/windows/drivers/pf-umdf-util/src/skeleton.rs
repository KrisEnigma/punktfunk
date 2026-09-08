//! The WDF skeleton every punktfunk HID driver stands on: `DriverEntry`, the default parallel
//! queue, the manual queue pended reads wait in, and the periodic timer that completes them.
//! Three drivers carried a copy each; the zeroed-config traps (a parallel queue presents ZERO
//! requests unless told otherwise, a timer's attributes must inherit or WDF refuses it) now
//! land once.

use wdk_sys::{
    NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT, PFN_WDF_DRIVER_DEVICE_ADD,
    PFN_WDF_IO_QUEUE_IO_DEVICE_CONTROL, PFN_WDF_TIMER, ULONG, WDF_DRIVER_CONFIG,
    WDF_IO_QUEUE_CONFIG, WDF_NO_HANDLE, WDF_NO_OBJECT_ATTRIBUTES, WDF_OBJECT_ATTRIBUTES,
    WDF_TIMER_CONFIG, WDFDEVICE, WDFDRIVER, WDFOBJECT, WDFQUEUE, WDFTIMER,
    call_unsafe_wdf_function_binding,
};

use crate::nt_success;

pub const STATUS_SUCCESS: NTSTATUS = 0;
pub const STATUS_NOT_IMPLEMENTED: NTSTATUS = 0xC000_0002u32 as NTSTATUS;
pub const STATUS_INVALID_PARAMETER: NTSTATUS = 0xC000_000Du32 as NTSTATUS;
pub const STATUS_INVALID_DEVICE_REQUEST: NTSTATUS = 0xC000_0010u32 as NTSTATUS;

// WDF enum values the bindings do not re-export; stable across WDF versions.
/// `WDF_IO_QUEUE_DISPATCH_TYPE`.
pub const WDF_IO_QUEUE_DISPATCH_PARALLEL: i32 = 2;
pub const WDF_IO_QUEUE_DISPATCH_MANUAL: i32 = 3;
/// `WDF_TRI_STATE::WdfUseDefault`.
pub const WDF_USE_DEFAULT: i32 = 2;
/// `WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent`.
pub const WDF_EXECUTION_LEVEL_INHERIT: i32 = 1;
/// `WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent`.
pub const WDF_SYNCHRONIZATION_SCOPE_INHERIT: i32 = 1;

/// `WdfDriverCreate` with `evt_device_add` as the one callback a punktfunk driver registers.
///
/// # Safety
/// `driver` and `registry_path` must be the loader-provided `DriverEntry` arguments.
pub unsafe fn driver_create(
    driver: PDRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
    evt_device_add: PFN_WDF_DRIVER_DEVICE_ADD,
) -> NTSTATUS {
    // SAFETY: a zeroed WDF_DRIVER_CONFIG is a valid all-null config; Size + the callback follow.
    let mut config: WDF_DRIVER_CONFIG = unsafe { core::mem::zeroed() };
    config.Size = core::mem::size_of::<WDF_DRIVER_CONFIG>() as ULONG;
    config.EvtDriverDeviceAdd = evt_device_add;
    // SAFETY: `driver`/`registry_path` are the loader's pointers per this fn's contract; the
    // config is valid and outlives the call.
    unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut config,
            WDF_NO_HANDLE.cast::<WDFDRIVER>()
        )
    }
}

/// The device's default queue, parallel, every IOCTL to `evt_io_device_control`.
///
/// # Safety
/// `device` must be the live device `WdfDeviceCreate` just returned in this `EvtDeviceAdd`.
pub unsafe fn create_default_queue(
    device: WDFDEVICE,
    evt_io_device_control: PFN_WDF_IO_QUEUE_IO_DEVICE_CONTROL,
) -> Result<WDFQUEUE, NTSTATUS> {
    // SAFETY: zeroed config then fields set; Size matches the struct.
    let mut qcfg: WDF_IO_QUEUE_CONFIG = unsafe { core::mem::zeroed() };
    qcfg.Size = core::mem::size_of::<WDF_IO_QUEUE_CONFIG>() as ULONG;
    qcfg.DispatchType = WDF_IO_QUEUE_DISPATCH_PARALLEL;
    qcfg.PowerManaged = WDF_USE_DEFAULT;
    qcfg.DefaultQueue = 1;
    qcfg.EvtIoDeviceControl = evt_io_device_control;
    // WDF_IO_QUEUE_CONFIG_INIT sets this to (ULONG)-1 (unlimited); mem::zeroed left it 0,
    // which on a parallel queue means present ZERO requests → EvtIoDeviceControl never fires.
    qcfg.Settings.Parallel.NumberOfPresentedRequests = u32::MAX;
    let mut queue: WDFQUEUE = core::ptr::null_mut();
    // SAFETY: `device` is live per the contract; config valid; attributes null; queue receives
    // the handle.
    let st = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &mut qcfg,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut queue
        )
    };
    if nt_success(st) { Ok(queue) } else { Err(st) }
}

/// A manual queue: requests parked here are completed by the driver's timer, not the framework.
///
/// # Safety
/// `device` must be the live device of the current `EvtDeviceAdd`.
pub unsafe fn create_manual_queue(device: WDFDEVICE) -> Result<WDFQUEUE, NTSTATUS> {
    // SAFETY: zeroed config then fields set.
    let mut mcfg: WDF_IO_QUEUE_CONFIG = unsafe { core::mem::zeroed() };
    mcfg.Size = core::mem::size_of::<WDF_IO_QUEUE_CONFIG>() as ULONG;
    mcfg.DispatchType = WDF_IO_QUEUE_DISPATCH_MANUAL;
    mcfg.PowerManaged = WDF_USE_DEFAULT;
    let mut queue: WDFQUEUE = core::ptr::null_mut();
    // SAFETY: `device` is live per the contract; config valid; attributes null; queue receives
    // the handle.
    let st = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &mut mcfg,
            WDF_NO_OBJECT_ATTRIBUTES,
            &mut queue
        )
    };
    if nt_success(st) { Ok(queue) } else { Err(st) }
}

/// A started periodic timer parented to `parent` (a queue, so the timer dies with it), firing
/// `evt_timer` every `period_ms`, serialized as UMDF requires (the vhidmini2 pattern). The
/// attributes inherit execution level and scope from the parent: zeroed (`Invalid`) is refused
/// with 0xc0200209 / 0xc00000bb.
///
/// # Safety
/// `parent` must be a live WDF object of the current `EvtDeviceAdd`.
pub unsafe fn create_periodic_timer(
    parent: WDFOBJECT,
    evt_timer: PFN_WDF_TIMER,
    period_ms: u32,
) -> Result<WDFTIMER, NTSTATUS> {
    // SAFETY: zeroed config then fields set.
    let mut tcfg: WDF_TIMER_CONFIG = unsafe { core::mem::zeroed() };
    tcfg.Size = core::mem::size_of::<WDF_TIMER_CONFIG>() as ULONG;
    tcfg.EvtTimerFunc = evt_timer;
    tcfg.Period = period_ms;
    tcfg.AutomaticSerialization = 1;
    // SAFETY: a zeroed WDF_OBJECT_ATTRIBUTES is a valid all-null attributes struct; Size + the
    // fields used follow.
    let mut tattr: WDF_OBJECT_ATTRIBUTES = unsafe { core::mem::zeroed() };
    tattr.Size = core::mem::size_of::<WDF_OBJECT_ATTRIBUTES>() as ULONG;
    tattr.ParentObject = parent;
    tattr.ExecutionLevel = WDF_EXECUTION_LEVEL_INHERIT;
    tattr.SynchronizationScope = WDF_SYNCHRONIZATION_SCOPE_INHERIT;
    let mut timer: WDFTIMER = core::ptr::null_mut();
    // SAFETY: config + attributes valid; timer receives the handle.
    let st = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerCreate, &mut tcfg, &mut tattr, &mut timer)
    };
    if !nt_success(st) {
        return Err(st);
    }
    // The due time in 100 ns units, negative = relative: the first tick is one period out.
    let due = -(i64::from(period_ms)) * 10_000;
    // SAFETY: `timer` was just created and is live.
    let _started = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStart, timer, due) };
    Ok(timer)
}
