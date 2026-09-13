---
title: Troubleshooting
description: Find your symptom — each entry is the check and the fix in a line or two, with a link to the full story when you need it.
---

Find your symptom below. Each entry is the likely cause and the fix in a line or two; the **→**
link opens the full write-up on its detail page. Nothing here matches?
[Get the logs](#still-stuck).

## Install & startup

### The Linux host service won't start

Usually no `~/.config/punktfunk/host.env` yet (copy the shipped template), or a stale hand-copied
unit shadowing the packaged one (`status=203/EXEC`).
→ [Install & startup](/docs/troubleshooting-startup#the-linux-host-service-wont-start)

### `systemctl --user status punktfunk-web`: unit not found

The console is its own package, only *recommended* by the host — an install that skipped weak deps
has no console to enable. `sudo apt install punktfunk-web` / `sudo dnf install punktfunk-web`.
→ [Install & startup](/docs/troubleshooting-startup#systemctl---user-status-punktfunk-web-unit-not-found)

### pacman: error: could not register 'punktfunk' database (database already registered)

The repo block was appended to `/etc/pacman.conf` twice — harmless; delete the extra `[punktfunk]`
block. → [Install & startup](/docs/troubleshooting-startup#pacman-error-could-not-register-punktfunk-database-database-already-registered)

### `nvidia-smi` says it can't communicate with the driver

The kernel module didn't load — usually Secure Boot blocking the unsigned module. Enroll the MOK
key (per-distro commands) or disable Secure Boot.
→ [Install & startup](/docs/troubleshooting-startup#nvidia-smi-says-it-cant-communicate-with-the-driver)

### The desktop won't start, or "GPU … not supported by EGL"

The NVIDIA GL/EGL userspace is missing — install `libnvidia-gl-<version>` matching your driver.
→ [Install & startup](/docs/troubleshooting-startup#the-desktop-wont-start-or-gpu--not-supported-by-egl)

### Session fails right after editing host.env

Keys are case-sensitive; a wrong-uid `XDG_RUNTIME_DIR` anchor or a stale `PUNKTFUNK_COMPOSITOR` pin
breaks the session. The env file is only read at service start.
→ [Install & startup](/docs/troubleshooting-startup#session-fails-right-after-editing-hostenv)

### Windows: the host or the web console won't start

`punktfunk-host service status` / `restart` from an elevated PowerShell. Two `punktfunk-host.exe`
processes are normal; the console's own log is `web.log`.
→ [Install & startup](/docs/troubleshooting-startup#windows-the-host-or-the-web-console-wont-start)

### Windows: "Punktfunk Virtual Display" shows Code 10 in Device Manager

The display driver needs **Windows 11 22H2 or newer** — on Windows 10 or 11 21H2 it installs but
can't start. The fix is updating Windows.
→ [Install & startup](/docs/troubleshooting-startup#windows-punktfunk-virtual-display-shows-code-10-in-device-manager)

## Connection & discovery

### Another streaming host (Sunshine, Apollo, …) is installed

Both sides want the same ports and the same mDNS name — `punktfunk-host detect-conflicts` tells
you; stop and uninstall the other host, or move a port to coexist.
→ [Connection & discovery](/docs/troubleshooting-connect#another-streaming-host-sunshine-apollo--is-installed)

### The host isn't found on the network

Check the host is running, the app's local-network permission (Android 17+), that both ends share a
subnet (mDNS doesn't cross routers), and the firewall — `punktfunk-native` opens UDP 9777 + 5353.
→ [Connection & discovery](/docs/troubleshooting-connect#the-host-isnt-found-on-the-network)

### Windows firewall

The installer opens ports on **Private** and **Domain** networks only — a LAN marked **Public**
makes the host unreachable. Set the network to Private, or re-scope with
`punktfunk-host service install --allow-public-network=on`.
→ [Connection & discovery](/docs/troubleshooting-connect#windows-firewall)

### Video is slow to start, or fails across subnets

The data plane rides a random per-session UDP port the client hole-punches; a deny-inbound firewall
makes every session start ~2.5 s late. Pin `PUNKTFUNK_DATA_PORT` and open that one port.
→ [Connection & discovery](/docs/troubleshooting-connect#video-is-slow-to-start-or-fails-across-subnets)

### The host is asleep and won't wake

The machine isn't armed for the magic packet — a Linux host's **Troubleshooting** page prints the exact
command for your interface under `Wake-on-`.
→ [Connection & discovery](/docs/troubleshooting-connect#the-host-is-asleep-and-wont-wake) ·
[Wake-on-LAN](/docs/wake-on-lan)

### Pairing is rejected / the client can't connect

Arm pairing in the web console, then enter the PIN on the client. Re-installed the host? Its
identity changed — re-pair. → [Pairing & Trust](/docs/pairing)

### A plugin's interface doesn't load

Plugin UIs are served on **TCP 47993**, a port a saved firewall rule doesn't know — re-expand the
`punktfunk-web` rule, or accept the certificate for that port once.
→ [Connection & discovery](/docs/troubleshooting-connect#a-plugins-interface-doesnt-load)

## Picture & session

### Black screen / no picture, but the client connects

Check you're on Wayland (not X11), the compositor floors (KWin ≥ 6.5.6 headless, GNOME ≥ 48,
gamescope ≥ 3.16.22), and remove any `PUNKTFUNK_COMPOSITOR` pin.
→ [Picture & session](/docs/troubleshooting-stream#black-screen--no-picture-but-the-client-connects)

### Capture fails: "Session creation inhibited" (GNOME)

A locked GNOME session blocks capture — disable the lock on an always-on host.
→ [Picture & session](/docs/troubleshooting-stream#capture-fails-session-creation-inhibited-gnome)

### A library launch says "Unable to open a connection to X" (Linux)

The host had no `DISPLAY` to hand the child — fixed in 0.35.0, so update; older hosts need
`DISPLAY=:0` in `host.env`.
→ [Picture & session](/docs/troubleshooting-stream#a-library-launch-says-unable-to-open-a-connection-to-x-linux)

### Games from my library open on a physical monitor, not on the stream (Hyprland / sway)

New windows follow focus on these compositors — current hosts claim focus for the streamed display;
update first, or use **Dedicated** game sessions.
→ [Picture & session](/docs/troubleshooting-stream#games-from-my-library-open-on-a-physical-monitor-not-on-the-stream-hyprland--sway)

### The screen stays black after switching to Game Mode (Nobara)

A pre-0.19.1 takeover could trip the display manager's start limit — unmask the session unit and
restart plasmalogin.
→ [Picture & session](/docs/troubleshooting-stream#the-screen-stays-black-after-switching-to-game-mode-nobara)

### Game Mode: black screen on connect, or the stream is stuck at the box's resolution

The managed takeover was refused — almost always because you're not in the `punktfunk` group:
`sudo usermod -aG punktfunk "$USER"`, then log out and back in.
→ [Picture & session](/docs/troubleshooting-stream#game-mode-black-screen-on-connect-or-the-stream-is-stuck-at-the-boxs-resolution)

### The picture freezes for a moment, over and over (Windows)

A rhythmic freeze isn't bandwidth — it's a display re-probing its link or something below Windows.
The **Status** card's Capture health and the `METRONOMIC` log line name the leg and the cure.
→ [Picture & session](/docs/troubleshooting-stream#the-picture-freezes-for-a-moment-over-and-over-windows)

### Stutter, drops, or high latency

Lower the bitrate first, prefer wired or 5 GHz Wi-Fi, and remember many sessions share one encoder.
→ [Picture & session](/docs/troubleshooting-stream#stutter-drops-or-high-latency)

## Input & controllers

### My mouse and keyboard are stuck in the stream

That's capture, not a bug — **Ctrl+Alt+Shift+Q** (⌃⌥⇧Q on macOS) or **L1+R1+Start+Select** hands
them back. → [Input & controllers](/docs/troubleshooting-input#my-mouse-and-keyboard-are-stuck-in-the-stream)

### My keyboard types the wrong characters (`#` comes out as `\`)

A host layout mismatch — Punktfunk sends physical keys, and the host session's layout decides what
they type. Set it with `localectl` and reconnect.
→ [Input & controllers](/docs/troubleshooting-input#my-keyboard-types-the-wrong-characters--comes-out-as-)

### A controller is detected but games don't see it

Linux: join the `input` group and re-login. Windows: a host that ever ran 0.22.0/0.22.1 must update
**through the installer**, not by swapping the exe.
→ [Input & controllers](/docs/troubleshooting-input#a-controller-is-detected-but-games-dont-see-it)

### The pad works, but arrives as an Xbox 360 controller instead of a Steam Deck

The virtual Deck pad needs the `punktfunk` group plus `vhci_hcd` — four checks, then re-login.
→ [Input & controllers](/docs/troubleshooting-input#the-pad-works-but-arrives-as-an-xbox-360-controller-instead-of-a-steam-deck)

### Stream lags, then freezes, with a DualSense pad (Bazzite, SELinux)

`steamos-manager`'s ds_inhibit trips SELinux into an AVC storm — punktfunk ships a `dontaudit`
drop-in (`sudo punktfunk-sysext reapply`), and masking `setroubleshootd` hardens any host.
→ [Input & controllers](/docs/troubleshooting-input#stream-lags-then-freezes-with-a-dualsense-pad-bazzite-selinux)

### A Steam Controller 2 is captured, but Steam's controller list stays empty

Only Steam reads that pad, and it needs the `hidraw` node — pre-0.30.0 Linux hosts need a udev
rule; on Windows reinstall the gamepad driver.
→ [Input & controllers](/docs/troubleshooting-input#a-steam-controller-2-is-captured-but-steams-controller-list-stays-empty)

### Copy and paste between host and client does nothing

Two switches, both needed: `PUNKTFUNK_CLIPBOARD` on the host, and the per-host toggle in the
client's Edit sheet. → [Shared clipboard](/docs/clipboard#why-the-toggle-does-nothing-or-is-greyed-out)

## Audio

### Audio stutters, and only the audio (Linux)

Something linked the host's virtual output to another device and handed it the clock — the log's
`clocked by another node` warning names it. Remove the loopback or that card's profile.
→ [Audio](/docs/troubleshooting-audio#audio-stutters-and-only-the-audio-linux)

### Streamed audio sounds worse than the host does

The default endpoint is a *silent* one — possibly a narrow voice device like Steam's Streaming
Microphone. `PUNKTFUNK_AUDIO_OUTPUT_MODE=host_and_client` is both the fix and the A/B test.
→ [Audio](/docs/troubleshooting-audio#streamed-audio-sounds-worse-than-the-host-does)

### Audio lags behind the picture

The jitter buffer self-corrects — reconnect once, then check the client's `underruns` count.
→ [Audio](/docs/troubleshooting-audio#audio-lags-behind-the-picture)

## Still stuck?

Read the host's log around the failed connect or capture. To hand everything to us instead — host
log, health checks and your client's log in one file — follow [Reporting an Issue](/docs/report-an-issue).

1. Open the web console's **Troubleshooting** page. Its **Logs** card always holds the host's recent
   output at *debug* detail, whatever the log level is set to — there's nothing to switch on and no
   restart needed.
2. Filter it down to the level or the text you're after. The **Sources** chips pick the producer:
   your [plugins](/docs/plugins) log to the same card, tagged `plugin:<name>`, so a misbehaving
   plugin is one click away rather than a separate hunt through the journal.
3. Use **Download logs** to save exactly what you're filtering on as a timestamped `.log` file you
   can attach to a bug report. The button beside it hands the same text to your phone or tablet's
   share sheet, or copies it to the clipboard on a desktop.

The same output lands outside the console — `journalctl --user -u punktfunk-host` (Linux),
`%ProgramData%\punktfunk\logs\host.log` (Windows, elevated-readable only; `service.log` sits next to
it). Those follow the log level: `RUST_LOG=debug` in [`host.env`](/docs/configuration) + a restart.

On the **client** side, the Windows client keeps `%LOCALAPPDATA%\punktfunk\logs\client.log` —
the only place a receive, decode or present failure is recorded.

For a performance problem, attach a **recording** instead of a log:
[Recording a capture for a bug report](/docs/stats#recording-a-capture-for-a-bug-report).
