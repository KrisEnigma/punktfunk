---
title: Install & startup
description: The host won't install or start — missing host.env, stale systemd units, NVIDIA Secure Boot, EGL, and the Windows service.
---

Problems between installing the packages and getting a running host. For everything else, back to
[Troubleshooting](/docs/troubleshooting).

## The Linux host service won't start

`systemctl --user status punktfunk-host` shows it failed. Two common causes:

- **No `host.env` yet.** The packaged unit reads `~/.config/punktfunk/host.env` and won't start
  until it exists — packages ship a template to copy:

  ```sh
  mkdir -p ~/.config/punktfunk
  # /usr/share/punktfunk/ on Fedora/Arch/Bazzite, /usr/share/punktfunk-host/ on Ubuntu
  cp /usr/share/punktfunk/host.env.example ~/.config/punktfunk/host.env
  systemctl --user restart punktfunk-host
  ```

  On Bazzite copy `host.env.bazzite` instead of `host.env.example`.
- **`status=203/EXEC`** means the unit points at a binary that isn't there — usually an old
  hand-copied unit in `~/.config/systemd/user/` shadowing the packaged one. Remove it, run
  `systemctl --user daemon-reload`, and start the packaged unit
  ([details](/docs/running-as-a-service#a-a-desktop-you-log-into)).

## `systemctl --user status punktfunk-web`: unit not found

The web console is its own package, only *recommended* by the host package — an install with weak
deps skipped (`install_weak_deps=False`, `apt --no-install-recommends`) gets a host with no
console. Install it by name:

```sh
rpm -q punktfunk-web || sudo dnf install punktfunk-web punktfunk-scripting   # Fedora
sudo apt install punktfunk-web                                              # Debian/Ubuntu
systemctl --user enable --now punktfunk-web
journalctl --user -u punktfunk-web-init | sed -n 's/.*password generated: //p'
```

`No match for argument` means the repo has no console package: **COPR** builds host and client
only. Use the RPM registry — [Fedora](/docs/fedora#2-install-the-host), step 2.

## pacman: error: could not register 'punktfunk' database (database already registered)

The repo block got appended to `/etc/pacman.conf` twice. Harmless — pacman ignores the duplicate —
but delete the extra `[punktfunk]` block to silence it. The
[current add line](/docs/arch#2-install-the-host) checks first and won't append a second copy.

## `nvidia-smi` says it can't communicate with the driver

The NVIDIA kernel module didn't load. With **Secure Boot** enabled (`mokutil --sb-state`), the
module is signed with a locally generated key that must be enrolled once — or disable Secure Boot
in firmware. Import the key, reboot, and on the blue **MOK Manager** screen (on the machine's own
console, not over SSH) choose *Enroll MOK → Continue → Yes → (the password) → Reboot*:

```sh
sudo mokutil --import /var/lib/shim-signed/mok/MOK.der     # Ubuntu
sudo mokutil --import /var/lib/dkms/mok.pub                # Debian (DKMS-built module)
sudo akmods --force && sudo mokutil --import /etc/pki/akmods/certs/public_key.der   # Fedora (akmod)
```

After a kernel update the module may need a rebuild — reinstall the driver package. Then confirm
`nvidia-smi` loads and `cat /sys/module/nvidia_drm/parameters/modeset` prints `Y` (Wayland needs
KMS; if not: `echo 'options nvidia-drm modeset=1' | sudo tee /etc/modprobe.d/nvidia-drm.conf`,
regenerate the initramfs, reboot).

## The desktop won't start, or "GPU … not supported by EGL"

The NVIDIA **GL/EGL userspace** is missing — the base driver package doesn't always include it.

- **Ubuntu:** `sudo apt install libnvidia-gl-<version>` (matching your driver).
- Confirm `/usr/share/glvnd/egl_vendor.d/10_nvidia.json` exists and `nvidia-drm modeset` is `Y`.

See [GNOME](/docs/gnome) for the GL/EGL userspace details.

## Session fails right after editing host.env

- Keys are **case-sensitive**: `punktfunk_gamescope_attach=1` sets nothing — use the exact
  uppercase names.
- Hardcoded session anchors with the wrong uid (`XDG_RUNTIME_DIR=/run/user/1000` when `id -u`
  isn't 1000) point the host at another user's PipeWire/D-Bus: audio errors, no capture, clients
  reporting the host as unreachable. **Delete both anchor lines** — a `systemctl --user` service
  doesn't need them — or fix the uid.
- `PUNKTFUNK_COMPOSITOR` pins the backend and disables Gaming ↔ Desktop following — remove it on
  any box that switches sessions.
- The env file is read at service start: `systemctl --user restart punktfunk-host` after edits.

## Windows: the host or the web console won't start

The **`PunktfunkHost` service** runs both halves: the streaming host and the web console, and
restarts either if it stops. The service commands need an **elevated** PowerShell.

1. **Is the service running?**

   ```powershell
   punktfunk-host service status
   punktfunk-host service restart
   ```
2. **Two `punktfunk-host.exe` processes in Task Manager is normal — don't kill one.** The service
   runs as SYSTEM in session 0, where it can't capture the screen or inject input, so it launches a
   second copy into the interactive session. One supervises, one streams.
3. **The console page never loads.** Give it a minute — the service restarts it on failure. If it
   stays down, check `%ProgramData%\punktfunk\logs\web.log` (and `service.log` next to it) from an
   **elevated** PowerShell — `Get-Content -Tail 50 $env:ProgramData\punktfunk\logs\web.log` — then
   `punktfunk-host service restart`. Right after a first install the console lags the host by a few
   seconds on purpose: it waits for the host to write its certificate.
4. **The status icon is missing after an update.** Windows only launches the tray at sign-in, and
   an upgrade closes the running ones. From your **normal** (not elevated) shell:

   ```powershell
   punktfunk-host tray start
   ```

   `punktfunk-host tray status` says whether one is running. See
   [Windows Host → Status tray](/docs/windows-host).

## Windows: "Punktfunk Virtual Display" shows Code 10 in Device Manager

Sessions end with *"pf-vdisplay driver interface not found"* and Device Manager shows the
**Punktfunk Virtual Display** failed with **Code 10** (`STATUS_DEVICE_POWER_FAILURE`).

Your Windows version is too old. The driver requires the **IddCx 1.10** framework, first shipped in
**Windows 11 22H2 (build 22621)** — on Windows 10 (including LTSC) and Windows 11 21H2 it installs
but can't start. The fix is updating Windows; reinstalling won't help.
