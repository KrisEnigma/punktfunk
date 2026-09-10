//! The desktop's own colours, so the console can look like it belongs to the box it manages
//! (`design/web-console-overhaul.md` §7).
//!
//! One route, read per request. There is no cache: the console already polls `ui-config` every
//! two seconds to catch `omarchy-theme-set`, and the same tick then catches a Windows accent
//! change for free. A registry read or one D-Bus round trip is cheaper than the staleness a
//! cache would buy.
//!
//! Linux reads the XDG portal, which is the only cross-desktop answer — GNOME, Plasma and
//! COSMIC all serve it, and `xdg-desktop-portal-gtk` answers for Hyprland and sway. A box with
//! no portal reports nothing, and the console says so; reading `gtk-4.0/settings.ini` behind
//! its back would be a second, quietly wrong code path.
//!
//! Windows reads the CONSOLE SESSION's hive. The host is SYSTEM in session 0 and has no HKCU
//! of its own, so `HKEY_CURRENT_USER` here would be the service account's empty profile.

use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

/// The desktop's appearance, as far as this host can see it.
#[derive(Serialize, ToSchema, Default)]
pub(crate) struct HostTheme {
    /// Where the answer came from, for the console's "Follow host — Windows" line.
    /// `null` when nothing answered.
    #[schema(example = "gnome")]
    source: Option<String>,
    /// `light` | `dark`, or `null` when the desktop expresses no preference.
    mode: Option<String>,
    /// `#rrggbb`, or `null` when the desktop has no accent to report.
    accent: Option<String>,
}

/// Host desktop theme
///
/// The mode and accent colour of the desktop this host runs on, for a console that follows it.
/// Every field is nullable: "no answer" is a real state, and the console renders it as
/// "none detected" rather than guessing.
#[utoipa::path(
    get,
    path = "/host/theme",
    tag = "host",
    operation_id = "getHostTheme",
    responses(
        (status = OK, description = "The desktop's mode and accent, as far as this host can see", body = HostTheme),
        (status = UNAUTHORIZED, description = "Missing or invalid bearer token", body = crate::mgmt::shared::ApiError),
    )
)]
pub(crate) async fn get_host_theme() -> Json<HostTheme> {
    // `read()` blocks — a D-Bus round trip on Linux, a registry read on Windows — and the
    // console polls `ui-config` every two seconds per open tab. Holding a runtime worker for
    // that is how one unresponsive portal starves every other management request.
    Json(tokio::task::spawn_blocking(read).await.unwrap_or_default())
}

#[cfg(target_os = "linux")]
fn read() -> HostTheme {
    // A desktop running no portal must not stall the request. Anything slower than this is
    // "no answer" as far as the console is concerned.
    const BUDGET: std::time::Duration = std::time::Duration::from_millis(400);
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::debug!("host theme: no runtime for the portal read ({e})");
            return HostTheme::default();
        }
    };
    rt.block_on(async {
        // The budget covers the whole exchange, not each call: a portal that accepts the
        // connection and then never answers is the case worth bounding.
        match tokio::time::timeout(BUDGET, async {
            let Ok(settings) = ashpd::desktop::settings::Settings::new().await else {
                return HostTheme::default();
            };
            // 0 = no preference, 1 = dark, 2 = light (`org.freedesktop.appearance`).
            let mode = match settings.color_scheme().await {
                Ok(ashpd::desktop::settings::ColorScheme::PreferDark) => Some("dark".into()),
                Ok(ashpd::desktop::settings::ColorScheme::PreferLight) => Some("light".into()),
                _ => None,
            };
            let accent = settings
                .accent_color()
                .await
                .ok()
                .map(|c| rgb_hex(c.red(), c.green(), c.blue()));
            // Only claim a source when something actually answered, or a box with no portal
            // would read as "following" a desktop that said nothing.
            let source = (mode.is_some() || accent.is_some()).then(|| "portal".to_string());
            HostTheme {
                source,
                mode,
                accent,
            }
        })
        .await
        {
            Ok(theme) => theme,
            Err(_) => {
                tracing::debug!("host theme: the desktop portal did not answer in time");
                HostTheme::default()
            }
        }
    })
}

/// The portal reports each channel as 0.0..=1.0.
#[cfg(target_os = "linux")]
fn rgb_hex(r: f64, g: f64, b: f64) -> String {
    let ch = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", ch(r), ch(g), ch(b))
}

/// The console session user's SID, as the string `HKEY_USERS` is keyed by.
///
/// The host is SYSTEM in session 0, so `HKEY_CURRENT_USER` is the service account's empty
/// profile — the operator's hive is only reachable through their SID. A locked or signed-out
/// session has no token, and reporting nothing is then correct.
#[cfg(target_os = "windows")]
fn console_session_sid() -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_USER};
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};

    // SAFETY: no arguments; returns 0xFFFFFFFF when no console session is attached.
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    if session == u32::MAX {
        return None;
    }
    let mut token = HANDLE::default();
    // SAFETY: `token` is a live out-param; needs SE_TCB, which SYSTEM has.
    unsafe { WTSQueryUserToken(session, &mut token) }.ok()?;
    // Sized in two calls: the first asks how big the variable-length SID is.
    let mut needed = 0u32;
    // SAFETY: a deliberate size probe — null buffer with zero length is the documented way
    // to ask, and it fails with ERROR_INSUFFICIENT_BUFFER while setting `needed`.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    let mut buf = vec![0u8; needed as usize];
    // SAFETY: `buf` is `needed` bytes, which is what the probe above asked for.
    let got = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    };
    // SAFETY: the token handle is ours and closed exactly once, on every path below.
    let _ = unsafe { CloseHandle(token) };
    got.ok()?;
    // SAFETY: on success the buffer holds a TOKEN_USER whose `Sid` points inside it.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut out = windows::core::PWSTR::null();
    // SAFETY: `sid` is the live SID above; `out` receives a LocalAlloc'd string we free.
    unsafe { ConvertSidToStringSidW(sid, &mut out) }.ok()?;
    // SAFETY: `out` is a NUL-terminated wide string from the successful call above.
    let text = unsafe { out.to_string() }.ok();
    // SAFETY: freeing exactly what ConvertSidToStringSidW allocated.
    unsafe { LocalFree(Some(HLOCAL(out.0.cast()))) };
    text
}

#[cfg(target_os = "windows")]
fn read() -> HostTheme {
    let Some(sid) = console_session_sid() else {
        // Locked, signed out, or no console session: there is no user hive to read, and
        // saying nothing is correct.
        return HostTheme::default();
    };
    let mode = read_dword(
        &format!(r"{sid}\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        "AppsUseLightTheme",
    )
    .map(|v| {
        if v == 0 {
            "dark".to_string()
        } else {
            "light".to_string()
        }
    });
    let accent = read_dword(
        &format!(r"{sid}\Software\Microsoft\Windows\DWM"),
        "AccentColor",
    )
    .map(abgr_hex);
    let source = (mode.is_some() || accent.is_some()).then(|| "windows".to_string());
    HostTheme {
        source,
        mode,
        accent,
    }
}

/// DWM stores the accent as ABGR: the alpha rides the top byte and the channels are reversed
/// from the `#rrggbb` a stylesheet wants.
#[cfg(target_os = "windows")]
fn abgr_hex(v: u32) -> String {
    let [b, g, r] = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(target_os = "windows")]
fn read_dword(subkey: &str, name: &str) -> Option<u32> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_USERS, RRF_RT_REG_DWORD};
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated HSTRINGs; `value`/`size` are live out-params
    // sized for the DWORD the flags demand.
    let rc = unsafe {
        RegGetValueW(
            HKEY_USERS,
            &HSTRING::from(subkey),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::addr_of_mut!(value).cast()),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(value)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn read() -> HostTheme {
    // No host runs here; the console falls back to its own palette.
    HostTheme::default()
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    /// DWM's DWORD is ABGR with alpha on top; a naive read renders every accent's red and
    /// blue swapped, which looks plausible and is wrong.
    #[test]
    fn accent_is_read_as_abgr_not_rgb() {
        // Windows' own default blue, #0078d4, is stored as 0xFFD47800.
        assert_eq!(abgr_hex(0xFFD4_7800), "#0078d4");
    }
}
