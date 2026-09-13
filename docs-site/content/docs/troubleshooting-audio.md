---
title: Audio
description: Streamed audio stutters, sounds worse than the host, or lags behind the picture — and the host-side endpoint and quality knobs.
---

Sound problems on the stream. For echo — hearing yourself — see
[Why do I hear myself](/docs/echo). For everything else, back to
[Troubleshooting](/docs/troubleshooting).

## Audio stutters, and only the audio (Linux)

Video steady, sound broken up: look for this line in the host log.

```
WARN  our audio capture group is being clocked by another node — every hole in this stream is
      that node's scheduling, not ours … driver="alsa_input.usb-…" expected="punktfunk-speaker-…"
```

PipeWire schedules audio in groups, each on one node's clock. The host brings its own — the virtual
output it records — so this warning means something linked that output to another device and handed
it the clock. Whatever that device does with timing, your stream does too.

The usual cause is a loopback from the host's virtual output to a real one (a "listen to this
device" setup). The worst case is a **sound card reached over the network** — a controller
forwarded with VirtualHere or USB/IP presents one, and its clock can't be recovered across the
link. Remove the loopback, or turn that card's audio profile off (KDE → Audio → the device →
Profile → *Off*), and the group returns to the host's clock.

Without the warning, audio stutter is the same problem as any other stutter — see
[Stutter, drops, or high latency](/docs/troubleshooting-stream#stutter-drops-or-high-latency).

## Streamed audio sounds worse than the host does

The host doesn't capture "the sound card" — it captures a **render endpoint**, and by default picks
one *silent on the host* so audio plays on your client only. On a PC with Steam installed that
silent endpoint is Steam's **Streaming Microphone**, which exists to carry remote *voice* — if
Windows has it configured narrow (mono, or below 48 kHz), the whole desktop mix is squeezed through
it before it's ever encoded.

The host logs what it picked:

```
WARN  the desktop-audio loopback endpoint mixes at 24000 Hz, so the stream is band-limited …
INFO  audio loopback capturing device="…" engine_hz=48000 engine_ch=2 engine_bits=32
```

The `engine_*` line is the endpoint's **own** format — it tells you directly whether the source was
ever full quality. To choose the routing yourself, set in `host.env`:

```ini
# client_only     — default; audio plays on the client only (a silent endpoint)
# host_and_client — capture a real output device; audio plays on BOTH ends
# follow_default  — capture whatever YOUR default playback device is, and never change it
PUNKTFUNK_AUDIO_OUTPUT_MODE=host_and_client
```

`host_and_client` is also the quickest A/B: if the stream sounds right that way and wrong on the
default, the endpoint was the cause.

### Quality and the lossless plane

```ini
PUNKTFUNK_AUDIO_QUALITY=high    # low | standard | high (default high — stereo 256 kbps)
PUNKTFUNK_AUDIO_REDUNDANCY=1    # force the loss-resilient audio plane on (default: automatic)
```

Both are a **request**, not a guarantee: the host budgets audio against the session's video bitrate
and steps it down on a narrow link — audio isn't managed by adaptive bitrate, so whatever it takes
comes off the top. On a roomy link you get 256 kbps plus loss redundancy; as the link narrows the
host drops redundancy first, then the tier, and never goes below ~96 kbps. The session log line
says what it settled on (`tier=high kbps=512 redundancy=true`).

For **no lossy stage at all**, pick a **Lossless** row in the client's audio-format setting — that's
the whole opt-in; the host allows the lossless plane by default. To forbid it on a host:

```ini
PUNKTFUNK_AUDIO_HIRES=0         # refuse the lossless PCM audio plane (default: allowed)
```

The plane replaces Opus with uncompressed PCM — 44.1–176.4 kHz, 16/24-bit, stereo through 7.1 —
costing 1.4–8.5 Mbps in stereo (up to 33.9 for 176.4 kHz/24-bit 7.1) against Opus's 256 kbps, off
the top of the link where adaptive bitrate can't see it. A session only goes lossless when it can
pay for it out of a quarter of its video bitrate.

It won't fix *this* section's problem: a lossless copy of a 24 kHz mono mix is still a 24 kHz mono
mix — fix the endpoint first. What lossless buys is bit-exactness; on game content 256 kbps Opus is
already transparent. And whenever any condition fails — the client didn't ask, `HIRES=0`, the
capture path can't deliver the rate, the link can't spare it — the session quietly stays on Opus
and the log says which condition lost; a declined session and a granted one look the same from the
settings screen.

<Callout type="warn">
If the box you're editing is **also a client**: the Linux and Windows clients read a
`PUNKTFUNK_AUDIO_HIRES` of their own with a richer grammar (`96000`, `96000/24`), so one line in a
shared environment sets both halves. `0` means *off* to each; anything else reads as *allow* on the
host side. The client's spellings are in
[Configuration → Client-side](/docs/configuration#client-side-native-clients).
</Callout>

## Audio lags behind the picture

The client buffers a little audio to absorb network jitter, and the buffer **corrects itself**: if
it drifts deeper it trims back a few milliseconds at a time, inaudibly. If audio is still late:

- **Reconnect once.** It confirms whether the delay was accumulated (gone after reconnect) or
  constant (something else).
- **Check for underruns** rather than guessing — the client logs its buffer depth periodically; a
  rising `underruns` count means the buffer is starved, which is a network or CPU problem, not a
  buffering one.
- **Wired or 5 GHz Wi-Fi.** Arrival jitter is what the buffer absorbs; less jitter, shallower
  buffer.
