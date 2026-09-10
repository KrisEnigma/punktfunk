---
title: Friends over the internet
description: Let a friend stream from your host without putting them on your home network — Porthole for PC friends, Tailscale sharing for everything else, and what not to use.
---

A friend wants to join your host for the evening — a second controller, a time limit, nothing
else — and they are not on your LAN. Inviting them into your Tailscale network works, but it hands
them every machine and service on it. This page gets them to Punktfunk on one host and nothing else.

Punktfunk already does the hard part. Nothing streams without [pairing](/docs/pairing), you set
what a device may do and for how long ([Access levels](/docs/access-levels)), and video is
encrypted per session. All you need to solve is how the friend reaches the host.

## Pick a path

| Your friend streams from | Use |
|---|---|
| A PC, Mac or Steam Deck with Steam | [Porthole](#porthole) — easiest, nothing to configure on your router |
| A phone, tablet, Apple TV, or no Steam | [Tailscale machine sharing](#tailscale) |
| Anything, and you manage your own router | [Port forwarding](#port-forwarding) — not recommended, and the only path that exposes the host to the internet |

Every path needs the video port pinned first.

## Pin the video port

The video plane normally picks a random UDP port per session. A tunnel or a rule has to name one,
so pin it in `~/.config/punktfunk/host.env` (Windows: `%ProgramData%\punktfunk\host.env`) and
restart the host:

```ini
PUNKTFUNK_DATA_PORT=9779
```

Any free UDP port works. Avoid 9778, which the browser client's WebTransport plane uses.

## Porthole

[Porthole](https://porthole.sestudio.org/) is a free Steam app that shares ports you pick with
Steam friends. It works behind any router, and the friend sees only the ports you share.

1. Both of you install Porthole from Steam.
2. You create a lobby and share UDP `9777` and `9779`, without remapping. Add TCP `47990` if the
   friend should browse your game library.
3. The friend joins with your share code or from the friends list, accepts the ports, and adds
   `127.0.0.1:9777` as a host in their Punktfunk client.
4. [Admit them as a guest](#admit-them-as-a-guest).

One Porthole friend at a time: the pinned port serves a single session.

## Tailscale

Tailscale can share one machine with someone outside your network. They see that machine only.
Your access rules still decide what they may reach on it, and the default rule lets them reach
everything — that is the step people skip.

1. In the [admin console](https://login.tailscale.com/admin/machines), open the host's **⋯** menu
   → **Share** and send the invite. Any free Tailscale account can accept it; the friend installs
   Tailscale on the device they stream from (every Punktfunk client platform except LG webOS).
2. In [access controls](https://login.tailscale.com/admin/acls), replace the default `"src": ["*"]`
   rule — `*` includes shared users — with one rule for members and one for shared users, using
   the host's Tailscale IP:

   ```jsonc
   "acls": [
     { "action": "accept", "src": ["autogroup:member"], "dst": ["*:*"] },
     { "action": "accept", "src": ["autogroup:shared"], "dst": ["100.x.y.z:9777,9779"] }
   ]
   ```

   Add `47990` only if the friend should browse your game library.
3. The friend adds `100.x.y.z:9777` as a host, or opens a
   [link](/docs/profiles-and-links#punktfunk-links) you send them: `punktfunk://connect/100.x.y.z:9777`.
4. [Admit them as a guest](#admit-them-as-a-guest).

## Admit them as a guest

The friend's first connect shows up on your console as a pending device. Admit it with a
[PIN](/docs/pairing#pair-with-a-pin), not a bare Approve — read the PIN out over voice chat — and
pick **Controller only** with an expiry
([choosing access](/docs/pairing#choosing-access-when-you-admit-a-device)).

When the expiry passes the device is refused until you re-grant it. **Expire now** or **Unpair**
on the Paired devices table ends a running session at once. There is no "unpair when they
disconnect" yet, so set an expiry that fits the evening.

## Port forwarding

**Not recommended. Prefer [Porthole](#porthole) or [Tailscale](#tailscale) whenever either one
reaches your friend** — both leave the host unreachable from the internet, and this does not. Take
this path only when neither fits, and take the rules below seriously.

Forward UDP `9777` and `9779` to the host. **Never forward `47990` or `9778`.**

Send a [link](/docs/profiles-and-links), not a bare address, so your friend's first connect is
verified rather than blind. **Copy link** on a client already paired to this host writes one
carrying `fp=`; swap the address in `host=` for your public one:

```
punktfunk://connect/<id>?host=203.0.113.5:9777&fp=<64 hex>
```

Strangers knock once the port is open. Nothing streams to them — an unpaired device is refused, and
pairing answers only while a window is armed — but the host can't yet tell a knock from the internet
apart from one on your couch, so two things fall to you.

**Bind the window to their device.** The console's **Pair a device** window runs for two minutes and
any device that knocks can consume it. Binding it to one device is CLI-only today:

```sh
punktfunk-host ctl pending --json    # their knock, with the full fingerprint
punktfunk-host ctl pair arm --fingerprint <fp> --preset controller --expires-in 14400
```

Read the PIN out over voice chat. Don't use the one-click **Approve** here — the name on a pending
device is one that device chose for itself.

**Give it an expiry.** Arming from the console defaults to Full control, forever; pick Controller
only and a deadline that fits the evening ([Admit them as a guest](#admit-them-as-a-guest)).

A pairing that keeps being refused usually means a stranger is knocking into the same rate limit.
Wait a moment and retry.

## What not to use

ZeroTier, Hamachi and Radmin VPN work, but they expose every port on the host to the friend, not
just Punktfunk's. playit.gg, ngrok and Cloudflare Tunnel cannot carry a video stream.
