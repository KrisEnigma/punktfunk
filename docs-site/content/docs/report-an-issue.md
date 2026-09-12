---
title: Reporting an Issue
description: Send a client's log to its host, export everything as one file from the web console, and attach it to a bug report.
---

The most useful bug report is one file: the host's log, its health checks and the log of the client
that saw the problem, side by side. Punktfunk builds that file for you. Send the client's log to the
host, export from the web console, attach the export to an issue.

> **Found a security problem?** Don't open a public issue — email **security@punktfunk.com**. See
> [SECURITY.md](https://git.unom.io/unom/punktfunk/src/branch/main/SECURITY.md).

## 1. Reproduce it, then leave the app open

Both logs live in memory. The client keeps the newest 4096 lines of the app's **current run** and
loses them when you quit; the host keeps its own newest 4096 lines. So send and export soon after the
problem, from the same app run that saw it. Quitting the client to "try again" throws away the lines
that matter.

Note roughly when it happened. The export is long, and a time is the quickest way into it.

## 2. Send the client's log to the host

Open the menu of the host you were streaming from and pick **Send logs to host**. No stream needs to
be running.

| Client | Where |
| --- | --- |
| Linux, Windows, Android | The host card's menu |
| iPhone, iPad, Mac, Apple TV | Long-press (or right-click) the host card → **Send Logs to Host** |
| Any gamepad console — Steam Deck Gaming Mode, the Windows couch console, a controller home on Android or Apple | The host's options → **Send logs to host** |

The item appears only for a host you have **paired** with, and only while it is **online** (the Apple
apps show it for any paired host). When the upload lands the client says *Logs sent to ⟨host⟩ —
download them from its web console's Logs page*.

What goes: the newest 4096 lines (at most 768 KiB) at debug detail, with the app's version and OS on
the first line. The host keeps each device's five latest uploads. The `punktfunk` command-line client
has no Send logs.

**The upload fails?** It goes to the host's management port, TCP **47990**, the same port the game
library uses. If that client can't load the library either, the port is blocked — see
[Ports](/docs/ports). Or take the log by hand: [Logs without the console](#logs-without-the-console).

## 3. Export everything from the web console

1. Open the [web console](/docs/web-console) and pick **Troubleshooting** in the sidebar.
2. Press **Export all** at the top of the page.

The browser saves one plain-text file, `punktfunk-diagnostics-YYYYMMDD-HHMMSS.txt`, holding:

- **Health checks** — every check the host runs, worst first, with its remedy.
- **Host and plugin log** — all of it, not only the rows the page's filters show.
- **Device bundles** — every client log this host holds, from every device, exactly as sent.

Open the page *before* you reproduce if you can. An open page keeps up to 5000 lines, including ones
the host has already dropped from its own buffer, and the export takes the page's copy.

**Read it before you post it.** Nothing is redacted: expect host names, IP addresses, device names and
file paths. Issues on the tracker are public.

Want less than everything? On the **Logs** card below, the **Sources** chips pick the host, its
plugins or single devices, and **Download logs** saves only the rows showing. **Manage … uploaded
bundles** lists each upload with its own **Download bundle** and **Delete**.

## 4. File the issue

Open a new issue at [git.unom.io/unom/punktfunk/issues](https://git.unom.io/unom/punktfunk/issues).
Not sure it's a bug? Ask on [Discord](https://discord.gg/kaPNvzMuGU) first. Write down:

- **Versions** of host and client. The console's **Host** page shows the host's; the first line of each
  device bundle shows the client's.
- **Setup** — host OS and desktop (KDE, GNOME, Game Mode, Windows), GPU, the client device, wired or
  Wi-Fi.
- **What you did, what you expected, what happened**, and roughly when.
- **The export** from step 3, attached.

For stutter, lag or a low frame rate a log rarely says why. Add a **Performance** recording too:
[Recording a capture for a bug report](/docs/stats#recording-a-capture-for-a-bug-report).

## Logs without the console

When the host is down or the upload can't get through:

| Where | How |
| --- | --- |
| Linux host | `journalctl --user -u punktfunk-host` |
| Windows host | `%ProgramData%\punktfunk\logs\` (`host.log`, `service.log`, `web.log`) from an **elevated** PowerShell |
| Windows client | **Settings → About → Diagnostics → Open log folder** (`client.log`) |
| Mac, iPhone, iPad | Console.app on a Mac, filtered on subsystem `io.unom.punktfunk` (connect an iPhone or iPad by cable) |
| Android | `adb logcat` — the native side tags its lines `punktfunk` |
| Linux client | Its standard output — start it from a terminal to capture it |

The journal and the log files follow `RUST_LOG` (info by default), so they hold less than the
in-memory copies above. [Troubleshooting → Still stuck?](/docs/troubleshooting#still-stuck) has the
details.
