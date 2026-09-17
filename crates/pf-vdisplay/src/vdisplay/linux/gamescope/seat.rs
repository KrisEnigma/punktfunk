//! A Steam home per seat: the `HOME` a dedicated Steam launch runs under.
//!
//! Steam's single instance is `$HOME/.steam/steam.{pid,pipe}`, so a seat with its own home is a
//! second, independent client beside the box's — no sandbox, no `steam -shutdown`, and two seats
//! can play at once. [`ensure_home`] provisions it once by reflinking the box's install without
//! its account or its library; [`env`] is what the nested command is given.
//!
//! The clone carries no credentials, so every seat signs in to Steam once.
//! Evidence: `design/steam-seats-warm-launch-implementation-plan.md`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Steam's install under a seat home, which is also what `XDG_DATA_HOME` points at.
const STEAM_REL: &str = ".local/share/Steam";

/// What a seat must not inherit: the box's account, its library, and state that names either.
/// Top-level `*.vdf` (`local.vdf`, `loginusers.vdf`) goes with them.
const CLONE_SKIP: &[&str] = &["config", "userdata", "appcache", "steamapps", "logs"];

/// A reflink clone is metadata only. Anything slower is a filesystem without reflink support,
/// where `cp` refuses outright and Steam bootstraps itself instead.
const CLONE_BUDGET: Duration = Duration::from_secs(60);

/// Is there a Steam under this seat home for a launch to reach? A home nothing ever provisioned
/// (no native Steam on the box) has none, and a forwarder pointed at it would cold-start a second
/// Steam in an empty directory instead of talking to the one the session is showing.
pub(super) fn has_steam(home: &Path) -> bool {
    home.join(STEAM_REL).is_dir()
}

/// The seat's `HOME`, provisioned from the box's Steam the first time it is used.
///
/// `None` when the box has no native Steam to clone (Flatpak keeps its own root): the launch then
/// falls back to sharing the box's Steam, as it did before seats.
pub(super) fn ensure_home(home: &Path) -> Option<PathBuf> {
    let steam = home.join(STEAM_REL);
    let mut provisioned = false;
    if !steam.exists() {
        let Some(src) = box_steam_root() else {
            tracing::info!(
                "gamescope: this box has no native Steam install to clone (a Flatpak Steam keeps \
                 its own root), so this session shares the box's Steam"
            );
            return None;
        };
        if let Err(e) = pf_paths::create_private_dir(home) {
            tracing::warn!(seat_home = %home.display(), error = %e,
                "gamescope: seat home not created — this session shares the box's Steam");
            return None;
        }
        match clone_install(&src, &steam) {
            Ok(()) => {
                provisioned = true;
                if let Err(e) = write_library_folders(&src, &steam) {
                    tracing::warn!(error = %format!("{e:#}"),
                        "gamescope: the seat did not inherit the box's library folders — its Steam \
                         offers to download games the box already has");
                }
            }
            Err(e) => {
                // A half-copied tree would read as an install. Empty, Steam downloads itself once.
                let _ = std::fs::remove_dir_all(&steam);
                tracing::warn!(error = %format!("{e:#}"),
                    "gamescope: the box's Steam was not cloned into the seat, so the seat downloads \
                     Steam once by itself — a filesystem without reflink support always does");
            }
        }
    }
    tracing::info!(seat_home = %home.display(), provisioned,
        "gamescope: this session's Steam runs under its own home");
    Some(home.to_path_buf())
}

/// The env a seat's nested Steam runs under.
///
/// `XDG_RUNTIME_DIR` is deliberately absent: PipeWire, Wayland and the EIS relay live there and
/// belong to the session, not to the seat. The rest would drag Steam back to the box's home.
pub(super) fn env(home: &Path) -> Vec<(&'static str, String)> {
    let under = |rel: &str| home.join(rel).to_string_lossy().into_owned();
    vec![
        ("HOME", home.to_string_lossy().into_owned()),
        ("XDG_DATA_HOME", under(".local/share")),
        ("XDG_CONFIG_HOME", under(".config")),
        ("XDG_CACHE_HOME", under(".cache")),
        ("XDG_STATE_HOME", under(".local/state")),
    ]
}

/// The box's Steam install: the `~/.steam/steam` link a native install keeps, else the XDG data
/// dir. `None` on a box with neither.
fn box_steam_root() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let linked = home.join(".steam/steam");
    if linked.is_dir() {
        return Some(std::fs::canonicalize(&linked).unwrap_or(linked));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("Steam");
    data.is_dir().then_some(data)
}

/// Top-level entries the seat does not get.
fn excluded_from_clone(name: &str) -> bool {
    CLONE_SKIP.contains(&name) || name.ends_with(".vdf")
}

/// Reflink the box's install into the seat. `--reflink=always` is the point: a real 4 GB copy per
/// seat is not worth it, and refusing leaves Steam to download itself once.
fn clone_install(src: &Path, dst: &Path) -> Result<()> {
    let mut cmd = Command::new("cp");
    cmd.args(["-a", "--reflink=always"]);
    let mut any = false;
    for entry in std::fs::read_dir(src).context("read the box's Steam install")? {
        let entry = entry.context("read the box's Steam install")?;
        if excluded_from_clone(&entry.file_name().to_string_lossy()) {
            continue;
        }
        cmd.arg(entry.path());
        any = true;
    }
    if !any {
        bail!("the box's Steam install holds nothing a seat can use");
    }
    std::fs::create_dir_all(dst).context("create the seat's Steam directory")?;
    let status = crate::proc::status_within(
        cmd.arg(dst).stdout(Stdio::null()).stderr(Stdio::null()),
        CLONE_BUDGET,
    )?;
    if !status.success() {
        bail!("reflink the box's Steam install into the seat: {status}");
    }
    Ok(())
}

/// Hand the seat the box's library folders, so its Steam finds the installed games instead of
/// offering to download them again. Only ever written once; Steam owns the file afterwards.
fn write_library_folders(src: &Path, dst: &Path) -> Result<()> {
    let dir = dst.join("steamapps");
    let seat = dir.join("libraryfolders.vdf");
    if seat.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(src.join("steamapps/libraryfolders.vdf"))
        .context("read the box's library folders")?;
    let folders = library_folder_paths(&text);
    if folders.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).context("create the seat's steamapps directory")?;
    std::fs::write(&seat, library_folders_vdf(&folders)).context("write the seat's library folders")
}

/// Every `"path"` value of a text-VDF `libraryfolders`, in file order and without duplicates.
fn library_folder_paths(vdf: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in vdf.lines() {
        let mut fields = line.split('"').skip(1);
        if fields.next() != Some("path") {
            continue;
        }
        let Some(path) = fields.nth(1).filter(|p| !p.is_empty()) else {
            continue;
        };
        if !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

/// The smallest `libraryfolders` Steam accepts: one numbered entry per folder. Steam fills in
/// `contentid`, `totalsize` and the app list itself on first scan.
fn library_folders_vdf(paths: &[String]) -> String {
    let mut out = String::from("\"libraryfolders\"\n{\n");
    for (i, path) in paths.iter().enumerate() {
        out.push_str(&format!(
            "\t\"{i}\"\n\t{{\n\t\t\"path\"\t\t\"{path}\"\n\t}}\n"
        ));
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account and the library stay on the box; everything else is what makes Steam run.
    #[test]
    fn a_seat_clone_leaves_the_box_its_account_and_its_library() {
        for skipped in [
            "config",
            "userdata",
            "appcache",
            "steamapps",
            "logs",
            "local.vdf",
            "loginusers.vdf",
        ] {
            assert!(excluded_from_clone(skipped), "{skipped} must not be cloned");
        }
        for kept in [
            "ubuntu12_32",
            "steam.sh",
            "linux64",
            "package",
            "compatibilitytools.d",
        ] {
            assert!(!excluded_from_clone(kept), "{kept} is part of the install");
        }
    }

    /// `XDG_RUNTIME_DIR` must not be in the list, and every other XDG dir must be.
    #[test]
    fn the_seat_env_moves_the_xdg_dirs_but_never_the_runtime_dir() {
        let env = env(Path::new("/seats/cafe0123"));
        let named: Vec<&str> = env.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            named,
            [
                "HOME",
                "XDG_DATA_HOME",
                "XDG_CONFIG_HOME",
                "XDG_CACHE_HOME",
                "XDG_STATE_HOME"
            ]
        );
        assert!(env.iter().all(|(_, v)| v.starts_with("/seats/cafe0123")));
        let data = &env.iter().find(|(k, _)| *k == "XDG_DATA_HOME").unwrap().1;
        assert_eq!(
            Path::new(data).join("Steam"),
            Path::new("/seats/cafe0123").join(STEAM_REL),
            "the clone lands where XDG_DATA_HOME sends Steam"
        );
    }

    /// Round-trip: what the box lists is what the seat is handed.
    #[test]
    fn the_seat_inherits_every_library_folder_the_box_lists() {
        let box_vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t\t\"totalsize\"\t\t\"0\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"\n\t}\n}\n";
        let folders = library_folder_paths(box_vdf);
        assert_eq!(
            folders,
            ["/home/u/.local/share/Steam", "/mnt/games/SteamLibrary"]
        );
        assert_eq!(
            library_folder_paths(&library_folders_vdf(&folders)),
            folders,
            "what we write must parse back"
        );
        assert!(library_folder_paths("\"libraryfolders\"\n{\n}\n").is_empty());
        assert!(
            library_folder_paths("\t\t\"pathological\"\t\t\"/nope\"\n").is_empty(),
            "only the `path` key counts"
        );
    }
}
