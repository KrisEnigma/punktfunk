---
title: Input & controllers
description: Stuck keys, wrong characters, controllers games can't see, the virtual Steam Deck pad, and clipboard problems.
---

Keyboard, mouse, pad and clipboard problems. For everything else, back to
[Troubleshooting](/docs/troubleshooting).

## My mouse and keyboard are stuck in the stream

Nothing is broken — the stream captures them on purpose so your keys and pointer go to the host.
**Ctrl+Alt+Shift+Q** hands them back (**⌃⌥⇧Q** on macOS); with a pad, **L1+R1+Start+Select** does
the same on the Linux, Windows and Steam Deck clients. The other in-stream chords are in
[Getting your input back](/docs/input#getting-your-input-back).

## My keyboard types the wrong characters (`#` comes out as `\`)

A German keyboard giving `\` for `#`, `'` for `ä` or swapped `z`/`y` is a **host** layout mismatch —
the client is fine. Punktfunk sends the *physical key you pressed*, not the character; what it
types is decided by the layout the **host session** runs.

On Linux, set the layout the normal way and reconnect:

```sh
sudo localectl set-x11-keymap de pc105 nodeadkeys   # your layout, model, variant
```

Punktfunk reads that setting and hands it to the session on the next connect. Two things to know:

- **Wayland desktops don't read it by themselves** — `localectl` writes a file only Xorg opens. If
  your compositor is already set to the right layout in its own settings, nothing changes.
- **Game Mode needs a current `punktfunk-gamescope`.** Gamescope publishes no keyboard layout to
  the apps it runs, so Steam and games saw US whatever the box was set to. Our build fixes that
  from `+pfhdr8` on — check with `punktfunk-gamescope --version` and [update](/docs/updating) if
  it's older.

Nothing here changes which physical key does what in a game: `WASD` stays under the same fingers on
every layout.

## A controller is detected but games don't see it

- **Linux.** The host user needs to be in the `input` group — `sudo usermod -aG input $USER`, then
  log out and back in (on Bazzite: `ujust add-user-to-input-group`). See [Bazzite](/docs/bazzite).
- **Windows, if this PC ever ran 0.22.0 or 0.22.1.** Those releases bound one of Windows' own
  drivers to the emulated controller instead of Punktfunk's. Update **through the installer** — the
  setup `.exe`, `winget upgrade`, or the console's **Update now** button
  ([Updating](/docs/updating)). Swapping `punktfunk-host.exe` by hand doesn't fix it: the stale
  controller device keeps the driver it was already bound to.

## The pad works, but arrives as an Xbox 360 controller instead of a Steam Deck

Only the **virtual Steam Deck controller** (paddles, trackpads, gyro) is missing — ordinary gamepad
input is fine. That pad reaches games as a real USB device over usbip, through sysfs files owned by
a group called `punktfunk` (separate from `input`). Four things have to line up on the Linux host:

```sh
getent group punktfunk                         # the group exists at all
id -nG | tr ' ' '\n' | grep -x punktfunk       # ...and you are in it
ls -l /sys/devices/platform/vhci_hcd.0/attach  # owned by punktfunk, mode 0660
lsmod | grep vhci_hcd                          # the transport module is loaded
```

If the group is missing entirely, the udev rule tried to `chgrp` to a group nobody created —
re-running your package manager's upgrade creates it now, or `sudo groupadd --system punktfunk` by
hand. Then `sudo usermod -aG punktfunk "$USER"` and **log out and back in** — group changes only
reach the host's `systemd --user` service on a fresh login.

Joining is optional on purpose: writing that `attach` file materialises an arbitrary emulated USB
device. Skip it on a machine you share — but the same group authorizes the helper behind the
managed **Gaming Mode** takeover, so on an autologin box skipping it also costs you
[the takeover](/docs/troubleshooting-stream#game-mode-black-screen-on-connect-or-the-stream-is-stuck-at-the-boxs-resolution).

## Stream lags, then freezes, with a DualSense pad (Bazzite, SELinux)

On Bazzite (and other SELinux-enforcing Fedora Atomic spins), a **DualSense / DualShock 4**-type
virtual pad can make the stream lag then freeze — gamescope at 0 fps, `tx_mbps` collapsing. The
virtual pad binds the kernel's `hid-playstation` driver, and Valve's `ds_inhibit` (inside
`steamos-manager`) walks `/proc/*/fd/` on every open/close. SELinux denies `steamos_manager_t` that
walk — hundreds of `avc: denied` per second — and `setroubleshootd` amplifies the flood into a
box-wide fork storm that starves the stream.

Two traps while diagnosing: the AVC lines read `comm="tokio-rt-worker"` — that is steamos-manager,
not punktfunk (check `scontext=…steamos_manager_t…`); and the `setroubleshootd` storm **outlives
the denials by 15+ minutes**, so the box stays starved after the pad is gone.

- **Fix:** punktfunk ships a `dontaudit` SELinux drop-in that silences the flood (ds_inhibit then
  leaves the pad uninhibited — harmless). The sysext installs it on install/update; on an existing
  install run `sudo punktfunk-sysext reapply`. On a layered or bootc host:
  `sudo semodule -i /usr/share/punktfunk/selinux/punktfunk-ds-inhibit.cil`.
- **Hardening, recommended on any streaming host:** `sudo systemctl mask --now setroubleshootd` —
  a desktop alert daemon nothing depends on. Reversible with `unmask`.
- **Workaround with a feature loss:** set the *client's* controller type to Xbox 360 (uinput, no
  `hid-playstation`) — costs adaptive triggers, lightbar and touchpad.

## A Steam Controller 2 is captured, but Steam's controller list stays empty

The client shows **Steam Controller 2, captured** — but Steam's **Settings → Controller** has
nothing, buttons do nothing, the trackpads don't move the pointer.

Unlike every other pad, the Steam Controller 2 has exactly one consumer: **Steam**. No kernel
driver claims it, and its reports ride a vendor collection, so it produces no evdev node for
anything else. If Steam can't open its `hidraw` node, you get no controller at all.

**On a Windows host, stop here** — the pad is a UMDF device the `pf_gamepad` driver package serves,
so an empty controller list means the driver package is missing or stale:

```powershell
punktfunk-host.exe driver install --gamepad
```

On Linux the node is root-only until a udev rule says otherwise, and distro `steam-devices` rule
sets are per-product-id — a copy predating the SC2 (it shipped in 2026) never grants it. Punktfunk
ships the rule itself from 0.30.0 on; on an older host:

```sh
sudo tee /etc/udev/rules.d/61-punktfunk-sc2.rules >/dev/null <<'EOF'
KERNEL=="hidraw*", KERNELS=="*28DE:1302*", GROUP="input", MODE="0660", TAG+="uaccess"
KERNEL=="hidraw*", KERNELS=="*28DE:1304*", GROUP="input", MODE="0660", TAG+="uaccess"
KERNEL=="hidraw*", ATTRS{idVendor}=="28de", ATTRS{idProduct}=="1302", GROUP="input", MODE="0660", TAG+="uaccess"
KERNEL=="hidraw*", ATTRS{idVendor}=="28de", ATTRS{idProduct}=="1304", GROUP="input", MODE="0660", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then end the session and reconnect so the pad re-enumerates under the new rule. `1302` is the wired
controller, `1304` the Puck dongle.

To confirm this is what you're hitting, check the host log for `attached via usbip` **without** a
following `answering feature GET` — the kernel enumerated the controller and Steam never opened it.
If `attached via usbip` is missing entirely, work through
[the virtual Steam Deck section](#the-pad-works-but-arrives-as-an-xbox-360-controller-instead-of-a-steam-deck)
first.

Not a bug: with Punktfunk capturing, the trackpads stop working as a mouse whenever Steam isn't
running — capture turns the controller's built-in mouse emulation off so it can read the full
report stream, exactly as on a Steam Deck in desktop mode.

## Copy and paste between host and client does nothing

The shared clipboard needs **two** switches: the host operator allows it with `PUNKTFUNK_CLIPBOARD`
in `host.env` (then restart the host), and you turn it on for that host in your client's **Edit…**
sheet. Work through
[Why the toggle does nothing](/docs/clipboard#why-the-toggle-does-nothing-or-is-greyed-out) — it
also names the clients and host sessions where nothing crosses no matter what you set.
