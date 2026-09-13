---
title: Picture & session
description: The stream connects but the picture is wrong — black screen, stutter, rhythmic freezes, Game Mode problems, and games landing on the wrong monitor.
---

Problems once a session connects — the picture, the session, the windows on it. For everything
else, back to [Troubleshooting](/docs/troubleshooting).

## Black screen / no picture, but the client connects

- You must be on a **Wayland** session, not X11 (check the login-screen session picker).
- KWin must be **≥ 6.5.6** (`kwin_wayland --version`) for the *headless* appliance session
  (`kwin_wayland --virtual`); a normal Plasma 6 login needs only the screencast grant. GNOME
  **≥ 48**; gamescope **≥ 3.16.22**. See [KDE](/docs/kde) and [gamescope](/docs/gamescope).
- If [`host.env`](/docs/configuration) sets `PUNKTFUNK_COMPOSITOR`, **remove it** — the host
  auto-detects the live compositor, and the pin points it at one backend even when a different
  session is live (it also disables Gaming ↔ Desktop following).

## Capture fails: "Session creation inhibited" (GNOME)

A **locked** GNOME session blocks screen capture. On an always-on/headless host, disable the lock:

```sh
gsettings set org.gnome.desktop.screensaver lock-enabled false
gsettings set org.gnome.desktop.session idle-delay 0
```

See [GNOME → Headless session](/docs/gnome#headless-session) and
[Running as a Service](/docs/running-as-a-service).

## A library launch says "Unable to open a connection to X" (Linux)

Steam — or Lutris, or a native X11 game — opens that dialog instead of the game, and the same title
launches fine once you've started Steam yourself. The host had no `DISPLAY` to hand the child: a
systemd `--user` host starts before the session exports one. The second attempt works because a
Steam client that's already up takes the `steam://` URL over its own pipe and never needs X.

Hosts after **0.35.0** read the session's display at each launch, so [update](/docs/updating)
first. On an older one, add `DISPLAY=:0` to `~/.config/punktfunk/host.env` (`ls /tmp/.X11-unix/`
names the number) and `systemctl --user restart punktfunk-host`.

## Games from my library open on a physical monitor, not on the stream (Hyprland / sway)

The stream shows your bare desktop while the game runs on a screen at the machine. On **Hyprland**
and **sway** the virtual display is an *extend* output, and neither compositor moves anything onto
a new output by itself — new windows open on whatever monitor holds **focus**.

The host claims focus for the streamed display at session start and before each library launch —
if you're seeing this, [update](/docs/updating) first: hosts up to **0.29.0** never claimed it.
The host log says which head it took (`focused the streamed headless output`) and warns when it
couldn't.

If it still happens on a current host:

- **Are you also using the machine in person?** Clicking on a physical monitor while the game is
  starting pulls the new window over. Launch, then leave the host's keyboard and mouse alone until
  the game is up.
- **A launcher that opens a second window later** (Steam Big Picture, Heroic) places it wherever
  focus is *at that moment*. If that's your normal way to play, set **Virtual displays → Dedicated
  game sessions** to **Dedicated** — every launch gets its own headless gamescope with only the
  game inside (needs `gamescope` installed).
- **Primary won't do it** — Wayland has no primary output for these two backends, so Primary
  behaves as Extend. **Exclusive** *is* implemented: it switches your physical monitors off for the
  session, which puts every window on the stream. See
  [Virtual displays → Topology](/docs/virtual-displays#topology).

## The screen stays black after switching to Game Mode (Nobara)

On distros whose Game Mode is display-manager autologin under **plasmalogin** (Nobara), a managed
takeover from a host **0.19.1 or older** could kill the display manager. Recover from a VT
(Ctrl+Alt+F3) or SSH:

```sh
systemctl --user unmask --runtime 'gamescope-session-plus@*.service'
sudo systemctl reset-failed plasmalogin && sudo systemctl restart plasmalogin
```

Current hosts detect the display-manager flavor and never mask the session unit there — see
[gamescope → autologin display managers](/docs/gamescope#nobara-and-other-autologin-display-managers).

## Game Mode: black screen on connect, or the stream is stuck at the box's resolution

You connect to a box that autologins into Steam **Gaming Mode** and get a black picture — or a
picture at the box's own resolution instead of the one your client asked for, with the box's
monitor still lit. The managed takeover is being refused and the host is mirroring the box's own
session instead; on a box whose panel is off there is nothing to mirror.

Almost always the cause is **group membership**: the takeover stops the display manager through a
root helper that serves members of the `punktfunk` group only.

```sh
id -nG | tr ' ' '\n' | grep -x punktfunk      # are you in it?
journalctl --user -u punktfunk-host | grep -iE "punktfunk. group|takeover unavailable"
```

The host checks at startup on any box that needs the takeover, so a fresh
`systemctl --user restart punktfunk-host` puts the answer at the top of the log. The fix:

```sh
sudo usermod -aG punktfunk "$USER"   # then log out and back in
```

Read the reason the log quotes before anything else — the takeover has three other ways to be
refused (no packaged helper, no polkit, polkit denying the action) and the host prints which one it
hit. A host with no login session of its own also enables lingering through the same helper, so an
unjoined user often sees "enabling lingering failed" first. Both are covered in
[gamescope → autologin display managers](/docs/gamescope#nobara-and-other-autologin-display-managers).

## The picture freezes for a moment, over and over (Windows)

A freeze on a **rhythm** — every few seconds, every minute, always the same gap — is not a
bandwidth problem; lowering the bitrate won't touch it.

Start on the console's **Status** page: while a session runs, its card shows **Capture health** —
`healthy` and `idle` need no action; `stalled (worker)` / `(transport)` / `(conversion)` /
`(presentation)` names the leg that lost the frames, and **Last recovery** shows what the host did
about it.

If that doesn't explain the rhythm, open the **Troubleshooting** page and search for `METRONOMIC`:

- **…and coincide with Windows monitor hot-plug/re-enumeration events** — a display (or its cable,
  switch or AVR) is re-probing its link on a timer and Windows reacts every time. Cures, best
  first: turn that display's **auto input scan/detect** off in its OSD (on TVs also *instant-on*
  and CEC), unplug its cable at the GPU, fit an HPD-holding adapter or dummy plug, or keep the
  display active while you stream. The console's **Virtual displays** page has a *Disable monitor
  devices while streaming (PnP)* toggle that suppresses the Windows-side reaction; the log line's
  `connected_inactive` field names the displays it suspects.
- **…with NO coinciding OS display event** — the disturbance is below Windows: a connected but
  sleeping screen serviced by the GPU driver, display-poller software (SteelSeries GG / SignalRGB
  class), or the desktop present clock — try a different refresh rate. On a laptop panel the host
  deactivated, keeping it active with the **primary** topology usually settles it
  ([Virtual displays → Topology](/docs/virtual-displays#topology)). Last resort: lowering the
  GPU-priority defaults (`setx /M PFVD_NO_RT_GPU 1` + a device restart, and
  `PUNKTFUNK_GPU_PRIORITY_CLASS=high`; the line's `rt_gpu_driver` / `rt_gpu_host` fields show
  what's engaged) has quieted this pattern on some AMD machines — but it masks the disturbance
  and costs stream latency under load.

Freezes that repeat *without* a steady rhythm are caught too: search the log for
`REPEATING without a stable period` — same fields, same cure list. Every session also stamps one
`GPU-priority posture for this capture session` line near its start, so a log shows the levers
before any stall fires.

## Stutter, drops, or high latency

- Lower the **bitrate** — on a busy or Wi-Fi link the requested bitrate may be too high. The native
  clients' [speed test](/docs/configuration#bitrate) picks a safe value; with Moonlight, set it
  manually.
- Prefer a **wired** connection or 5 GHz Wi-Fi between host and client.
- Streaming to **many devices at once** shares the GPU encoder — lower the bitrate first.

If the stream is *wrong* rather than late — a codec you didn't pick, 8-bit where you expected HDR —
the host likely declined the request and told your client so.
[When the client and the host disagree](/docs/client-settings#when-the-client-and-the-host-disagree)
lists what it does with each one.
