---
title: Connection & discovery
description: The client can't find or reach the host — Sunshine conflicts, discovery, firewall ports, pairing, Wake-on-LAN, and the plugin UI port.
---

Problems getting a client to see and reach the host. For everything else, back to
[Troubleshooting](/docs/troubleshooting).

## Another streaming host (Sunshine, Apollo, …) is installed

Punktfunk's Moonlight-compatible mode and **Sunshine** / its forks (**Apollo**, **Vibeshine**,
Vibepollo, LuminalShine, …) bind the same GameStream ports and advertise the same `_nvstream` mDNS
name; on Windows they often install a conflicting virtual-display driver, and even native-only both
sides want **TCP 47990**. Symptoms: `address already in use` in the host log, pairing that silently
fails, the wrong host answering, or a host that "worked until one day it didn't" (a boot race for
47990).

- **Check:** `punktfunk-host detect-conflicts` lists every Sunshine-family host it finds and exits
  **1 only if one is running or set to start on its own**. The host logs the same finding at
  startup; `ss -lptn 'sport = :47990'` (Linux) / `netstat -ano | findstr :47990` (Windows) shows
  who holds the port.
- **Fix:** stop and uninstall the other host (`sudo systemctl disable --now sunshine`; on Windows
  `sc stop SunshineService`, then its uninstaller and its display driver), then start Punktfunk.
- **Keeping both for a while?** Leave GameStream compat off and move Punktfunk's management port
  (`PUNKTFUNK_MGMT_BIND`); on Windows pick a non-exclusive display topology. The whole recipe is on
  [Switching from Sunshine](/docs/switching-from-sunshine).

## The host isn't found on the network

- Make sure the host is running — `systemctl --user status punktfunk-host` (Linux),
  `punktfunk-host service status` (Windows).
- **On an Android phone or TV, check the app's local-network permission.** Android 17+ blocks
  Punktfunk from touching your LAN — discovery, connect, Wake-on-LAN, the library — until you allow
  it. Tap **Allow…** under *Local network access is off* at the top of the host list, or enable
  **Nearby devices** for Punktfunk in Android's app settings.
- Host and client must be on the **same subnet** — discovery uses mDNS, which doesn't cross routed
  subnets or most VPNs. Fallback: add the host by **IP address** in the client.
- A host firewall can block it. The native protocol needs **UDP 9777** (control) and **UDP 5353**
  (mDNS); the packages ship a `punktfunk-native` rule for both, plus TCP 47990 for the library API:

  ```sh
  sudo ufw allow punktfunk-native            # ufw (CachyOS, Ubuntu)
  sudo firewall-cmd --permanent --add-service=punktfunk-native \
    && sudo firewall-cmd --reload            # firewalld (Fedora, some Arch spins)
  ```

  GameStream/Moonlight needs TCP **47984/47989/48010** + UDP **47998/47999/48000/5353** — the
  `punktfunk-gamestream` rule. The full table: [Ports & firewall](/docs/ports).

### Windows firewall

On a Windows host, check the network profile. The installer opens the streaming and console ports
on **Private** and **Domain** networks only — if Windows classified your LAN as **Public**, no
client can reach the host (the host logs a warning at startup). Set it to Private in **Settings →
Network & internet → your network → Network profile type**.

For a trusted network Windows insists on marking Public, re-scope the streaming ports from an
elevated prompt — the installer's *Allow connections on Public networks* option only takes effect
on a **first** install:

```powershell
punktfunk-host service install --allow-public-network=on
```

That leaves your GameStream choice and the rest of `host.env` alone. It covers the streaming ports;
the console's own rule for TCP 47992 keeps the scope it was installed with. See
[Running as a Service → Windows](/docs/running-as-a-service#windows).

## Video is slow to start, or fails across subnets

The native **data plane** (the raw UDP that carries video, separate from the 9777 control plane)
uses a **random, per-session UDP port** — the host binds `0.0.0.0:0` and tells the client the port
during the connect handshake.

Video flows host → client, but the **client sends the first packet**: a hole-punch datagram to that
port, so the host learns the client's real (possibly NAT-translated) source address. What that
means for a host firewall:

- **No host firewall (or the port allowed):** video starts at once. Nothing to configure.
- **Host firewall that denies inbound** (ufw/nftables/firewalld default): the punch is dropped, the
  host waits **~2.5 s**, then falls back to the address the client reported — a stateful firewall
  admits the return traffic. **It works, but each session starts ~2.5 s late.** That slow start is
  the symptom of a missing data-plane rule.
- **Across subnets / NAT:** the same applies as long as the host's outbound video can reach the
  client. A host behind NAT reached only through a forwarded control port is the case a fixed data
  port solves.

To remove the delay, pin the data port in [`host.env`](/docs/configuration) and open exactly that
one:

```ini
# ~/.config/punktfunk/host.env (Linux) · %ProgramData%\punktfunk\host.env (Windows)
PUNKTFUNK_DATA_PORT=9779
```

```sh
systemctl --user restart punktfunk-host    # Windows: punktfunk-host service restart
sudo ufw allow 9779/udp
```

Running `serve` by hand? Pass `--data-port 9779` — but not alongside the service, which already
holds the ports.

A fixed data port serves **one session at a time**; a second concurrent session falls back to a
random port (logged). On a normal single-LAN setup you can also just accept the one-time ~2.5 s
punch-timeout.

## The host is asleep and won't wake

Clients wake a saved host themselves — auto-wake is on by default — but only once they've seen it
awake (that's how they learn its MAC) and only if the machine is armed to answer a magic packet.

The arming is what's usually missing. A **Linux** host tells you outright: search the console's
**Troubleshooting** page for `Wake-on-` — the line either confirms the card is armed or names the interface
and the exact command to arm it (wired and Wi-Fi cards use different commands). Windows and macOS
hosts don't run that check — go to [Arming the machine](/docs/wake-on-lan#arming-the-machine).

## Pairing is rejected / the client can't connect

- The host **requires pairing** by default — arm pairing from the web console, then enter the PIN
  on the client. See [Pairing & Trust](/docs/pairing).
- If you re-installed the host, its identity changed — re-pair the client.

## A plugin's interface doesn't load

The plugin's page in the console opens — title, version, **Open in new tab** — but the panel below
stays empty. Plugin interfaces are served on **TCP 47993**, a separate port from the console's
47992, so a plugin can't act as you with your session
([why](/docs/web-console#two-ports-not-one)). An empty panel means the browser can't load that
port:

- **The port isn't open.** On an upgraded host this is the usual answer — a saved firewall rule
  lists only the ports it knew at the time. Re-expand it:

  ```sh
  sudo ufw app update punktfunk-web && sudo ufw reload        # ufw
  sudo firewall-cmd --reload                                   # firewalld
  ```

  ```powershell
  punktfunk-host service install      # Windows: re-adds both console rules
  ```

  Verify with `sudo ufw status verbose` or `sudo firewall-cmd --info-service=punktfunk-web` —
  **47993** should be listed next to 47992.
- **The certificate isn't trusted for that port yet.** Browsers keep self-signed exceptions *per
  port*, and a warning page can't render inside a panel. The console offers a link to open the
  plugin in its own tab — accept the warning there once and come back.
