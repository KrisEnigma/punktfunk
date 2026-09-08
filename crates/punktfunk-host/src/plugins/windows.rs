//! The Windows plugin runner: the scheduled task, and the ACL policy on what LocalService may
//! read and write.
//!
//! The runner is a separate, unprivileged principal. It reads the scoped `plugin-token` and the
//! TLS pin, never `mgmt-token`, and it writes only the plugin/script units, their state and the
//! ingest drop — each grant is an explicit `icacls` call listed here rather than a directory it
//! inherits. That policy is the whole reason this file exists: it should be readable in one
//! place instead of found by grepping for a `cfg`.

use super::*;

/// `NT AUTHORITY\LocalService` in icacls SID form.
pub(super) const LOCAL_SERVICE_SID: &str = "*S-1-5-19";

/// Secrets the runner may read: scoped `plugin-token` and the TLS-pin cert
/// (`native-cert.pem` after the identity split, else `cert.pem`). Never `mgmt-token`.
/// Absent files are skipped, so listing both certs is safe on either host.
const RUNNER_SECRET_FILES: [&str; 3] = ["plugin-token", "native-cert.pem", "cert.pem"];

/// Unit dirs the runner imports. Inheritable `(RX,WA)`: bun's loader opens unit
/// files with FILE_WRITE_ATTRIBUTES; plain `(RX)` is EPERM on every import. WA
/// can only touch timestamps/readonly bits — `windowsSddlUnsafeReason` treats it
/// as harmless.
const RUNNER_UNIT_DIRS: [&str; 2] = ["plugins", "scripts"];

/// Writable state: `<config_dir>\plugin-state`. Plugins persist under
/// `plugin-state\<name>`, so LocalService needs Modify here — code dirs are
/// (RX,WA), secrets are (R). Inheritable onto per-plugin subdirs. Users stay
/// read-only (config-dir default).
const RUNNER_STATE_DIRS: [&str; 1] = ["plugin-state"];

/// Ingest inbox: `<config_dir>\ingest`. Inverse of `plugin-state`: `BUILTIN\Users`
/// gets Modify so an interactive-user app can drop `ingest\<plugin>\…` for the
/// LocalService runner to read. The rest of the config tree stays Users-read-only.
/// Any local user can drop a file here (trusted-single-user; the reader is LocalService).
const RUNNER_INGEST_DIRS: [&str; 1] = ["ingest"];

/// `BUILTIN\Users` (S-1-5-32-545) in icacls SID form — the ingest inbox's writer.
const USERS_SID: &str = "*S-1-5-32-545";

pub(super) fn enable() -> Result<()> {
    // Converge the principal before start: an older task may still be SYSTEM.
    // Idempotent; `-LogonType ServiceAccount` needs no stored password.
    powershell(&format!(
        "$p = New-ScheduledTaskPrincipal -UserId 'LocalService' -LogonType ServiceAccount; \
         Set-ScheduledTask -TaskName {TASK} -Principal $p -ErrorAction Stop | Out-Null"
    ))?;
    grant_runner_secret_reads();
    powershell(&format!(
        "Enable-ScheduledTask -TaskName {TASK} -ErrorAction Stop | Out-Null; \
         Start-ScheduledTask -TaskName {TASK} -ErrorAction Stop"
    ))?;
    println!("Plugin runner enabled and started ({TASK}, runs as LocalService).");
    Ok(())
}

pub(super) fn disable() -> Result<()> {
    powershell(&format!(
        "Stop-ScheduledTask -TaskName {TASK} -ErrorAction SilentlyContinue; \
         Disable-ScheduledTask -TaskName {TASK} -ErrorAction Stop | Out-Null"
    ))?;
    revoke_runner_secret_reads();
    println!("Plugin runner stopped and disabled ({TASK}).");
    Ok(())
}

/// Grant LocalService read on the runner secrets. `serve` writes them with a
/// SYSTEM/Administrators-only DACL (`pf_paths::write_secret_file`); `/grant:r`
/// replaces only LocalService's ACE. A later rewrite of the file drops the ACE —
/// re-run `plugins enable`. Missing files get a note; the grant retries next enable.
fn grant_runner_secret_reads() {
    let cfg = pf_paths::config_dir();
    for name in RUNNER_SECRET_FILES {
        let path = cfg.join(name);
        if !path.exists() {
            println!(
                "note: {} does not exist yet (the host writes it on first serve). Start the \
                 host once, then run `punktfunk-host plugins enable` again so the runner can \
                 authenticate.",
                path.display()
            );
            continue;
        }
        let ok = Command::new(icacls_path())
            .arg(&path)
            .args(["/grant:r", &format!("{LOCAL_SERVICE_SID}:(R)")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!(
                "warning: could not grant LocalService read on {} - the plugin runner may fail \
                 to authenticate to the management API",
                path.display()
            );
        }
    }
    // Unit dirs: inheritable (RX,WA). Create now so later files inherit rather
    // than needing another `plugins enable`.
    for name in RUNNER_UNIT_DIRS {
        let dir = cfg.join(name);
        if !create_runner_dir(&dir) {
            continue;
        }
        let ok = Command::new(icacls_path())
            .arg(&dir)
            .args(["/grant:r", &format!("{LOCAL_SERVICE_SID}:(OI)(CI)(RX,WA)")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!(
                "warning: could not grant LocalService read on {} - the runner may fail to \
                 import plugins/scripts from it",
                dir.display()
            );
        }
    }
    for name in RUNNER_STATE_DIRS {
        let dir = cfg.join(name);
        if !create_runner_dir(&dir) {
            continue;
        }
        let ok = Command::new(icacls_path())
            .arg(&dir)
            .args(["/grant:r", &format!("{LOCAL_SERVICE_SID}:(OI)(CI)(M)")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!(
                "warning: could not grant LocalService write on {} - state-writing plugins \
                 (config/cache) may fail to persist",
                dir.display()
            );
        }
    }
    for name in RUNNER_INGEST_DIRS {
        let dir = cfg.join(name);
        if !create_runner_dir(&dir) {
            continue;
        }
        let ok = Command::new(icacls_path())
            .arg(&dir)
            .args(["/grant:r", &format!("{USERS_SID}:(OI)(CI)(M)")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!(
                "warning: could not open the ingest inbox {} for writes - a plugin fed by an \
                 interactive-user app (e.g. playnite) may see no data",
                dir.display()
            );
        }
    }
    // `{app}\scripting` is not under the config dir. Same (RX,WA) as the unit
    // dirs: bun opens the entry script with FILE_WRITE_ATTRIBUTES, and the
    // install tree only carries Users:(RX). WA cannot change content.
    if let Some(dir) = runner_bundle_dir() {
        let ok = Command::new(icacls_path())
            .arg(&dir)
            .args(["/grant:r", &format!("{LOCAL_SERVICE_SID}:(OI)(CI)(RX,WA)")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!(
                "warning: could not grant LocalService read on {} - the plugin runner will not \
                 start (bun exits EPERM on its own entry script)",
                dir.display()
            );
        }
    }
}

/// Create a runner directory that the grants below re-ACL, refusing a reparse point.
///
/// `icacls` follows a junction, so a link planted here before the config dir was hardened would
/// move the grant — `BUILTIN\Users:(M)` for the ingest inbox — onto whatever it points at.
/// `create_private_dir` rejects one and locks the DACL first; the grant then adds its own ACE.
fn create_runner_dir(dir: &std::path::Path) -> bool {
    match pf_paths::create_private_dir(dir) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("warning: {} not created: {e}", dir.display());
            false
        }
    }
}

/// `None` if the exe path cannot be resolved; callers skip the grant rather than fail enable.
fn runner_bundle_dir() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("scripting"))
}

/// Drop the LocalService grants when the runner is switched off. `enable` re-grants.
fn revoke_runner_secret_reads() {
    let cfg = pf_paths::config_dir();
    for name in RUNNER_SECRET_FILES
        .iter()
        .chain(RUNNER_UNIT_DIRS.iter())
        .chain(RUNNER_STATE_DIRS.iter())
    {
        let path = cfg.join(name);
        if !path.exists() {
            continue;
        }
        let _ = Command::new(icacls_path())
            .arg(&path)
            .args(["/remove:g", LOCAL_SERVICE_SID])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    // Ingest was granted to Users, not LocalService. Removing that ACE leaves
    // the inherited Users:RX, so the dir reverts to read-only.
    for name in RUNNER_INGEST_DIRS {
        let path = cfg.join(name);
        if !path.exists() {
            continue;
        }
        let _ = Command::new(icacls_path())
            .arg(&path)
            .args(["/remove:g", USERS_SID])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    // Bundle dir is not under `cfg`. Removing the ACE leaves inherited
    // Users:(RX) from Program Files — read-only, not inaccessible.
    if let Some(dir) = runner_bundle_dir().filter(|d| d.exists()) {
        let _ = Command::new(icacls_path())
            .arg(&dir)
            .args(["/remove:g", LOCAL_SERVICE_SID])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// System32 `icacls`, not PATH — same planted-binary rule as [`powershell_path`].
pub(super) fn icacls_path() -> String {
    crate::install::sys32("icacls.exe")
}

/// System32 powershell, not PATH. CreateProcess searches the launching EXE's
/// directory first, so a planted `powershell.exe` beside the host would run
/// with these privileges.
fn powershell_path() -> String {
    crate::install::sys32(r"WindowsPowerShell\v1.0\powershell.exe")
}

pub(super) fn powershell(command: &str) -> Result<()> {
    let status = Command::new(powershell_path())
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .status()
        .context("run powershell")?;
    if !status.success() {
        bail!(
            "the {TASK} scheduled task couldn't be changed — is punktfunk installed with the \
             scripting component, and is this prompt elevated?"
        );
    }
    Ok(())
}

pub(super) fn powershell_output(command: &str) -> Option<String> {
    let out = Command::new(powershell_path())
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

// ---- elevation --------------------------------------------------------------------------------

/// Refuse unelevated admin-only ops. Do not self-elevate via UAC: that opens a
/// new console that closes on exit, hiding bun's output.
pub(super) fn require_elevation(what: &str) -> Result<()> {
    if is_elevated() {
        return Ok(());
    }
    // ASCII only: the default Windows console codepage drops em-dashes and arrows.
    bail!(
        "{what} needs administrator rights (the plugins directory under %ProgramData%\\punktfunk \
         and the runner task are admin-owned).\n\nOpen an elevated prompt: Start -> type \
         \"PowerShell\" -> right-click -> Run as administrator, then run this command again."
    )
}

/// Effective local-Administrator membership via `CheckTokenMembership`.
///
/// Not `TokenElevation`: a restricted/SAFER token from an elevated one
/// (`runas /trustlevel:0x20000`) still reports `TokenIsElevated = 1` while
/// Administrators is deny-only.
fn is_elevated() -> bool {
    use ::windows::Win32::Foundation::HANDLE;
    use ::windows::Win32::Security::{
        AllocateAndInitializeSid, CheckTokenMembership, FreeSid, PSID, SID_IDENTIFIER_AUTHORITY,
    };

    // BUILTIN\Administrators, S-1-5-32-544. Spelled out so this does not depend
    // on which windows crate module exports the RID constants.
    const NT_AUTHORITY: SID_IDENTIFIER_AUTHORITY = SID_IDENTIFIER_AUTHORITY {
        Value: [0, 0, 0, 0, 0, 5],
    };
    const BUILTIN_DOMAIN_RID: u32 = 32;
    const ALIAS_RID_ADMINS: u32 = 544;

    let mut admins = PSID::default();
    // SAFETY: AllocateAndInitializeSid is given a valid authority and exactly the 2 sub-authorities
    // its count argument declares (the remaining 6 are the API's required zero padding). On success
    // it yields a valid PSID that we pass to CheckTokenMembership and free on every path below;
    // `None` for the token means "the calling thread's effective token".
    unsafe {
        if AllocateAndInitializeSid(
            &NT_AUTHORITY,
            2,
            BUILTIN_DOMAIN_RID,
            ALIAS_RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut admins,
        )
        .is_err()
        {
            return false;
        }
        let mut is_member = ::windows::core::BOOL::default();
        let ok = CheckTokenMembership(Some(HANDLE::default()), admins, &mut is_member).is_ok();
        FreeSid(admins);
        ok && is_member.as_bool()
    }
}

/// Whether the process listening on `127.0.0.1:port` runs as LocalService, the runner's
/// principal. A registration outlives its plugin by up to the lease TTL, and a local user who
/// binds the freed port would otherwise receive the UI secret and answer the launch.
#[cfg(not(test))]
pub(crate) fn listener_is_runner(port: u16) -> bool {
    loopback_listener_pid(port).is_some_and(runs_as_local_service)
}

/// Owning pid of the IPv4 listener on `127.0.0.1:port`, if there is one.
#[cfg(not(test))]
fn loopback_listener_pid(port: u16) -> Option<u32> {
    use ::windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    use ::windows::Win32::Networking::WinSock::AF_INET;

    let mut size: u32 = 0;
    // SAFETY: a null table with a zero size is the documented size query; `size` is a live local.
    let _ = unsafe {
        GetExtendedTcpTable(
            None,
            &mut size,
            false,
            u32::from(AF_INET.0),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` is at least the size the query asked for and outlives the call; the table
    // is read back only up to `dwNumEntries`, which the call wrote.
    let rc = unsafe {
        GetExtendedTcpTable(
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            false,
            u32::from(AF_INET.0),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if rc != 0 || (size as usize) > buf.len() {
        return None;
    }
    let table = buf.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
    // SAFETY: the buffer holds a `MIB_TCPTABLE_OWNER_PID` header followed by `dwNumEntries`
    // rows, per the successful call above; the row count is bounded by the buffer length.
    let (entries, rows) = unsafe {
        let n = (*table).dwNumEntries as usize;
        let first = std::ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>();
        let max =
            (buf.len() - std::mem::size_of::<u32>()) / std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
        (n.min(max), first)
    };
    let loopback = u32::from_be(u32::from_be_bytes([127, 0, 0, 1]));
    for i in 0..entries {
        // SAFETY: `i < entries`, and every row up to `entries` lies inside `buf`.
        let row = unsafe { &*rows.add(i) };
        // Both fields are network byte order in the low 16 / 32 bits.
        let local_port = u16::from_be((row.dwLocalPort & 0xffff) as u16);
        if local_port == port && (row.dwLocalAddr == loopback || row.dwLocalAddr == 0) {
            return Some(row.dwOwningPid);
        }
    }
    None
}

/// Whether `pid`'s primary token belongs to `NT AUTHORITY\LocalService`.
#[cfg(not(test))]
fn runs_as_local_service(pid: u32) -> bool {
    use ::windows::Win32::Foundation::{CloseHandle, HANDLE};
    use ::windows::Win32::Security::{
        GetTokenInformation, IsWellKnownSid, TokenUser, WinLocalServiceSid, TOKEN_QUERY, TOKEN_USER,
    };
    use ::windows::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: plain handle plumbing. Every handle opened here is closed on every path, and
    // the token buffer is sized by the first query before the second call fills it.
    unsafe {
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(proc, TOKEN_QUERY, &mut token).is_ok();
        let _ = CloseHandle(proc);
        if !opened {
            return false;
        }
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let read = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if !read || buf.len() < std::mem::size_of::<TOKEN_USER>() {
            return false;
        }
        let user = &*buf.as_ptr().cast::<TOKEN_USER>();
        IsWellKnownSid(user.User.Sid, WinLocalServiceSid).as_bool()
    }
}
