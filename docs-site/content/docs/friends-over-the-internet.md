---
title: Friends over the internet
description: Let a friend stream from your host without putting them on your home network — Tailscale machine sharing scoped to Punktfunk's two ports, the guest access level that goes with it, and which other tunnels do and don't work.
---

You want a friend to join your host for an evening — a second controller, a time limit, nothing
else — and they are not on your LAN. The obvious move, inviting them into your Tailscale network,
works, but it hands them **every machine and every service** your tailnet can reach: your NAS, your
printer, the management console of anything with a web page. This guide sets it up so the friend
reaches exactly one thing: Punktfunk, on one host.

## What the friend actually gets

Punktfunk is safe to point a friend at because the host already defends itself:

- **Nothing streams without pairing.** A device you have not admitted gets a refusal, whatever
  network it comes from ([Pairing](/docs/pairing)).
- **You choose what the device may do**, and for how long — *Controller only* for the evening is
  the guest preset ([Access levels](/docs/access-levels)). The host enforces it; the client cannot
  widen its own access.
- **Video is encrypted per session** and addressed only to the device the host admitted.

So the whole job is to make the host *reachable* by the friend and *only* the host. Everything else
on this page is that.

## Share the host over Tailscale

Tailscale can share a **single machine** with someone outside your tailnet. They see that one
machine and nothing else — not your other devices, not your subnets — and *your* access rules
still apply to what they may reach on it. That last part is the step people skip.

### 1. Pin the video port

The video plane normally uses a fresh random UDP port per session (the client opens it with a
hole-punch). An access rule has to name a port, so pin one. In `~/.config/punktfunk/host.env`
(Windows: `%ProgramData%\punktfunk\host.env`):

```ini
PUNKTFUNK_DATA_PORT=9779
```

Restart the host. Any free UDP port works; avoid 9778, which the browser client's WebTransport
plane takes when you enable it.

### 2. Share the machine

In the [Tailscale admin console](https://login.tailscale.com/admin/machines), open the host's
**⋯** menu → **Share** → send the invite link. Your friend accepts it with any Tailscale account —
a free personal one is fine — and installs Tailscale on the device they will stream from. Tailscale
has apps for every platform Punktfunk has a client on except LG webOS ([Clients](/docs/clients)).

### 3. Restrict what a shared user may reach

Tailscale's default access rule is `"src": ["*"]`, and **`*` includes people you shared a machine
with** — so out of the box your friend could reach every port on the host. Edit the
[access controls](https://login.tailscale.com/admin/acls) so members keep everything and shared
users get Punktfunk's two ports, using the host's Tailscale IP (`100.x.y.z` in the machine list):

```jsonc
"acls": [
  { "action": "accept", "src": ["autogroup:member"], "dst": ["*:*"] },
  { "action": "accept", "src": ["autogroup:shared"], "dst": ["100.x.y.z:9777,9779"] }
]
```

Add `47990` to that list only if the friend should browse your game library — it is the
management API's read-only surface, which paired clients reach over mutual TLS. A *Controller only*
guest cannot launch anything, so they do not need it.

### 4. Connect and admit them

Discovery does not cross Tailscale, so the friend adds the host by address: `100.x.y.z:9777`.
Easier: send them a [link](/docs/profiles-and-links#punktfunk-links) that carries it —

```text
punktfunk://connect/100.x.y.z:9777
```

They open it, the client asks them to confirm the new host, and the connect shows up on your
console as a pending device. Admit it with a [PIN](/docs/pairing#pair-with-a-pin) rather than a bare Approve
— you read the PIN out over voice chat — and pick **Controller only** with an expiry
([choosing access](/docs/pairing#choosing-access-when-you-admit-a-device)).

### 5. Afterwards

When the expiry passes the device is refused until you re-grant it; **Expire now** or **Unpair**
on the Paired devices table ends a running session on the spot. There is no "unpair when they
disconnect" yet — set an expiry that fits the evening.

## Other tunnels

The Tailscale steps are more than most people want to do. These are the alternatives people ask
about, measured against what matters here: does it carry UDP at streaming bitrates, does the
friend's *client* platform run it, and what does the friend get to see.

| Tool | Friend needs | What they can reach | Works with Punktfunk today |
|---|---|---|---|
| **Tailscale machine sharing** (above) | a free Tailscale account + app | one host, two ports | yes — direct peer-to-peer, relayed only when NAT defeats it |
| **[Porthole](https://porthole.sestudio.org/)** (free Steam app) | Steam + Porthole on a PC / Mac / Steam Deck | only the ports you share | **not yet** — see below |
| **ZeroTier** | a free ZeroTier account + app, your network id | the whole host, no other machines | yes — same shape as Tailscale without the ACL step; no Apple TV app; free tier is one network, ten devices |
| Hamachi, Radmin VPN | the app | the whole host | yes, PC-only friends (Radmin is Windows-only; Hamachi's free tier is five machines) |
| playit.gg, ngrok, Cloudflare Tunnel | nothing | a public address anyone can knock on | no — TCP-only, or a throttled relay that cannot carry a video stream |

**Porthole** is the friendliest of the lot for PC-to-PC: both of you run it, the friend joins with a
share code or from your Steam friends list, and the ports you share appear on *their* machine at
`127.0.0.1`. Punktfunk's control plane would work through it. Its video plane does not yet: a port
proxy hands the host a translated source address, and with a pinned data port the host currently
sends video to the port the client *reported* instead of the one it *heard from*. That is a host
change on the roadmap, not something you can configure around. We have not tested Porthole; from
the code, it and any other port proxy connect and then show a black screen.

**ZeroTier** is the closest to "friendly and safe" without editing rules: create a network, the
friend joins it by id, you tick them as authorized. The scope is inherently one host — but *all* of
that host's ports, so anything else the PC serves (file shares, remote desktop) is reachable by
the friend too. Tailscale sharing without step 3 has exactly the same scope; step 3 is what makes
it two ports.

## Plain port forwarding

Forwarding UDP `9777` and your pinned data port on the router works — no tunnel, no account, the
friend connects to your public address — with one catch: video reaches the friend only if *their*
router keeps the port number their client used (most home routers do; none promise it), for the
same reason Porthole fails. What a stranger who finds the port gets is a TLS handshake
and a refusal, and a pairing attempt is only possible during the short window you arm and can only
guess the PIN once. Two honest caveats, which are why this page leads with Tailscale: **approve
without a PIN is not safe on a forwarded port** — a stranger's knock looks exactly like your
friend's — so admit only via a PIN bound to the pending device; and the host has no idea a knock
came from the internet, so nothing stops you from clicking Approve anyway. Never forward `47990`
or `9778`.
