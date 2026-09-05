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
| Anything, and you manage your own router | [Port forwarding](#port-forwarding) |

Both tunnels need the video port pinned first.

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

Forward UDP `9777` and `9779` to the host and give the friend your public address. A stranger who
finds the ports gets a refusal, and a pairing attempt is only possible during the short window you
arm. Two rules: admit only with a PIN, because the host cannot tell a stranger's knock from your
friend's, and never forward `47990` or `9778`.

## What not to use

ZeroTier, Hamachi and Radmin VPN work, but they expose every port on the host to the friend, not
just Punktfunk's. playit.gg, ngrok and Cloudflare Tunnel cannot carry a video stream.
