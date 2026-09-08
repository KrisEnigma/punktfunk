//! What the HID minidrivers could and could not use to answer the **channel proof**
//! (`pf_driver_proto::gamepad::ChannelProof`) — the measured record, so nobody re-derives it.
//!
//! The proof tells the host which process to duplicate a pad's DATA section into. It cannot come
//! from the bootstrap mailbox: that must be openable by LocalService (the driver's own WUDFHost runs
//! as LocalService), so any LocalService principal could name a process of its own and be handed the
//! pad's live input surface (security-review 2026-07-28). Only the driver actually bound to the
//! devnode the host created can answer that devnode's I/O — hence "ask the device stack".
//!
//! ## Measured on .173 (Win11 26200), 2026-07-28 — three attempts, one survivor
//!
//! * ❌ **`IOCTL_HID_GET_INDEXED_STRING`** (`HidD_GetIndexedString`). Fails for EVERY index — 1, 2,
//!   4, 0x0E, 0x5046 — on the same zero-access handle where the NAMED string calls succeed and
//!   return the driver's own text. hidclass does not carry an arbitrary indexed-string request to a
//!   UMDF HID minidriver at all.
//! * ❌ **A private device interface** (`WdfDeviceCreateDeviceInterface` + a private IOCTL, the shape
//!   `pf_xusb` uses). The interface REGISTERS and enumerates fine —
//!   `\\?\SWD#punktfunk#pf_mouse_0#{32244793-…}` was present — but `CreateFile` on it is refused:
//!   `ERROR_GEN_FAILURE` (31) for access 0 and GENERIC_READ, `ERROR_FILE_NOT_FOUND` (2) for
//!   GENERIC_READ|WRITE. hidclass owns `IRP_MJ_CREATE` on a devnode it is the FDO for, so a second
//!   interface on that devnode cannot be opened. This is why `pf_xusb` is fine and these two are not:
//!   it is NOT a HID minidriver, so nothing sits above it.
//! * ✅ **The serial-number string.** `HidD_GetSerialNumberString` succeeds on a zero-access handle
//!   and returns driver-authored text. Verified end to end: `pf_mouse` answered `PFCP:3:0:7296`, and
//!   pid 7296 was a genuine service-spawned `WUDFHost.exe`.
//!
//! So `pf_mouse` serves its proof AS its serial (its `PFMOUSE00` was inert). The pads deliberately do
//! NOT: their serials are what SDL and Steam dedup controllers on, and Steam is already known to
//! mangle a pad's displayed name over serial FORMAT alone. `pf_gamepad` therefore has no working
//! device-stack transport on this Windows build — see `ProofTransport::MailboxUnproven` on the host
//! side for the residual that leaves, and option B (a vendor feature report, which costs a
//! report-descriptor change) if it ever needs closing.

/// `CTL_CODE(FILE_DEVICE_KEYBOARD, id, METHOD_NEITHER, FILE_ANY_ACCESS)`: the HID minidriver
/// IOCTL numbering hidclass and mshidumdf speak.
#[must_use]
pub const fn ioctl(id: u32) -> u32 {
    (0x0000_000b << 16) | (id << 2) | 3
}
pub const IOCTL_HID_GET_DEVICE_DESCRIPTOR: u32 = ioctl(0);
pub const IOCTL_HID_GET_REPORT_DESCRIPTOR: u32 = ioctl(1);
pub const IOCTL_HID_READ_REPORT: u32 = ioctl(2);
pub const IOCTL_HID_WRITE_REPORT: u32 = ioctl(3);
pub const IOCTL_HID_GET_STRING: u32 = ioctl(4);
pub const IOCTL_HID_GET_DEVICE_ATTRIBUTES: u32 = ioctl(9);
pub const IOCTL_UMDF_HID_SET_FEATURE: u32 = ioctl(20);
pub const IOCTL_UMDF_HID_GET_FEATURE: u32 = ioctl(21);
pub const IOCTL_UMDF_HID_SET_OUTPUT_REPORT: u32 = ioctl(22);
pub const IOCTL_UMDF_HID_GET_INPUT_REPORT: u32 = ioctl(23);

/// The report-descriptor length a 9-byte HID descriptor declares (bytes 7..9, little-endian).
/// Every driver pins `declared_len(&HID_DESC) == RDESC.len()` at compile time.
#[must_use]
pub const fn declared_len(hid_desc: &[u8; 9]) -> usize {
    (hid_desc[7] as usize) | ((hid_desc[8] as usize) << 8)
}

/// `IOCTL_HID_GET_STRING`'s input: the low 16 bits of a little-endian u32 name the string
/// (`0`/`0x0E` manufacturer, `2`/`0x10` serial, else product). `(raw, id)`; a short input is `0`.
#[must_use]
pub fn string_request_id(bytes: &[u8]) -> (u32, u32) {
    let raw = match bytes {
        [a, b, c, d, ..] => u32::from_le_bytes([*a, *b, *c, *d]),
        _ => 0,
    };
    (raw, raw & 0xFFFF)
}

/// `s` as the NUL-terminated little-endian UTF-16 bytes a `GET_STRING` reply carries.
#[must_use]
pub fn utf16z(s: &str) -> Vec<u8> {
    let mut wide: Vec<u8> = Vec::with_capacity(s.len() * 2 + 2);
    for u in s.encode_utf16() {
        wide.extend_from_slice(&u.to_le_bytes());
    }
    wide.extend_from_slice(&[0, 0]);
    wide
}

/// The first `max` bytes as `"01 02 03 "` for a log line. Build it inside the log gate: a
/// release driver used to format 48 bytes per output report for a line it never wrote.
#[must_use]
pub fn hex_dump(bytes: &[u8], max: usize) -> String {
    let mut hex = String::with_capacity(bytes.len().min(max) * 3);
    for b in bytes.iter().take(max) {
        hex.push_str(&format!("{b:02x} "));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_ioctls_are_the_keyboard_device_neither_method_codes() {
        assert_eq!(IOCTL_HID_GET_DEVICE_DESCRIPTOR, 0x000B_0003);
        assert_eq!(IOCTL_HID_GET_REPORT_DESCRIPTOR, 0x000B_0007);
        assert_eq!(IOCTL_HID_GET_STRING, 0x000B_0013);
        assert_eq!(IOCTL_UMDF_HID_GET_INPUT_REPORT, 0x000B_005F);
    }

    #[test]
    fn declared_len_reads_the_little_endian_tail() {
        assert_eq!(
            declared_len(&[0x09, 0x21, 0x00, 0x01, 0x00, 0x01, 0x22, 0x11, 0x01]),
            273
        );
        assert_eq!(
            declared_len(&[0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x74, 0x01]),
            372
        );
    }

    #[test]
    fn get_string_input_is_the_low_word_and_short_input_is_zero() {
        assert_eq!(
            string_request_id(&[0x10, 0x00, 0x09, 0x04]),
            (0x0409_0010, 0x10)
        );
        assert_eq!(string_request_id(&[0x02]), (0, 0));
    }

    #[test]
    fn utf16z_is_little_endian_with_a_double_nul() {
        assert_eq!(utf16z("A"), vec![0x41, 0x00, 0x00, 0x00]);
        assert_eq!(utf16z(""), vec![0x00, 0x00]);
        assert_eq!(utf16z("é").len(), 4);
    }

    #[test]
    fn hex_dump_caps_and_spaces() {
        assert_eq!(hex_dump(&[0x01, 0xAB, 0xFF], 8), "01 ab ff ");
        assert_eq!(hex_dump(&[1, 2, 3], 2), "01 02 ");
        assert_eq!(hex_dump(&[], 4), "");
    }
}
