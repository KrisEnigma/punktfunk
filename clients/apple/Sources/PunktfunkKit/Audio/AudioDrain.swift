// The audio drain loop: pull decoded PCM off the connection, place it against the picture, and
// write it into the ring.
//
// Its own type because the loop is closed. The thread captures the connection, the stop flag and
// the ring and NEVER `SessionAudio` — so it keeps draining a session being torn down and stops on
// the flag rather than on a deallocation. Taking those as parameters is what keeps that true:
// there is no `self` here to reach for.

import AVFoundation
import Foundation

private let log = ClientLog(category: "audio")

enum AudioDrain {
    /// Start the drain thread. Every argument is read on the CALLER's thread, because the values
    /// derive from the connection and the loop must not hold the owner to ask.
    static func start(
        connection: PunktfunkConnection,
        flag: StopFlag,
        done: DispatchSemaphore,
        ring: AudioRing,
        videoLatency: LatencyMeter?,
        channels: Int,
        rateHz: Int,
        frameUs: Int,
        frameMS: Int
    ) {
    let thread = Thread { [connection, flag, done] in
        defer { done.signal() }
        var drained = 0
        var av = AvSync(channels: channels, rateHz: rateHz)
        // WP-C1 — the drought half of concealment. Core heals a SEQ GAP, but only when a later
        // packet arrives to reveal it; when the wire simply goes quiet nothing arrives to
        // reveal anything, and the ring drains into an underrun and a de-prime whose re-prime
        // is a longer artifact than the audio that was missing.
        //
        // Given the SESSION's frame, like the ring: this type spends a wall-clock budget one
        // frame at a time, and each `conceal()` that says yes costs exactly one `audioPlc()`
        // frame below — so if it assumed 5 ms, a 2 ms lossless session would spend the budget
        // in two fifths of the time it promises and report `plc_ms` two and a half times too
        // high. A 5.1 session, whose frame drops to ~1 ms, would be five times out.
        var drought = DroughtConceal(maxMS: AudioRing.plcMaxMS, frameUs: frameUs)
        var lastPacketNs = DispatchTime.now().uptimeNanoseconds
        // Something has decoded, so there is both state to conceal from and continuity to
        // hold. Until then a session whose host never sends audio keeps the long timeout below
        // rather than waking two hundred times a second to do nothing.
        var decoded = false
        // Decode happens IN-CORE (libopus multistream) — AudioToolbox's Opus path is
        // stereo-only — and is handed back as interleaved f32 PCM in wire channel order.
        // Per-iteration autorelease pool: no runloop on this thread (see Stage2Pipeline).
        var alive = true
        while alive, !flag.isStopped {
            alive = autoreleasepool { () -> Bool in
            let pcm: PunktfunkConnection.AudioPCM?
            do {
                // Wait at most one frame WHILE there is a stream to protect: the drought
                // decision has to be made on the wire's schedule, not whenever the next packet
                // happens to turn up. The SESSION's frame, so a lossless plane sending every
                // 2 ms is not judged on a 5 ms clock.
                pcm = try connection.nextAudioPcm(
                    timeoutMs: decoded ? UInt32(frameMS) : 100)
            } catch {
                return false // session closed
            }
            guard let pcm, pcm.frameCount > 0 else {
                // Nothing on the wire. If the ring is draining with it, conceal from the
                // decoder's own state — the same libopus interpolation the loss path uses,
                // bounded by this ring's de-prime fuse so a genuinely dead stream is not
                // papered over. ONE frame per tick, not a burst: this arm runs every frame,
                // which is the rate the callback drains at, so concealment keeps pace with
                // playout instead of racing ahead of a depth reading it has already
                // invalidated.
                guard decoded else { return true }
                let quietMS = Int(
                    (DispatchTime.now().uptimeNanoseconds &- lastPacketNs) / 1_000_000)
                guard drought.conceal(sinceLastPacketMS: quietMS, depthMS: ring.bufferedMS)
                else {
                    return true
                }
                let plc: PunktfunkConnection.AudioPCM?
                do {
                    plc = try connection.audioPlc()
                } catch {
                    return false // session closed
                }
                if let plc {
                    plc.samples.withUnsafeBufferPointer { p in
                        if let base = p.baseAddress {
                            ring.write(base, count: plc.frameCount * plc.channels)
                        }
                    }
                }
                ring.notePlcMS(drought.totalMS)
                return true
            }
            decoded = true
            lastPacketNs = DispatchTime.now().uptimeNanoseconds
            drought.packet()
            // Place this frame against the picture it belongs with BEFORE queueing it: the
            // depth read here is everything that must still play first, which is exactly what
            // delays it. Skipped wholesale when no meter was wired, so an un-armed session
            // does not even read the ring.
            if let videoLatency {
                let depth = ring.bufferedSamples
                var ts = timespec()
                clock_gettime(CLOCK_REALTIME, &ts)
                let nowNs = Int64(ts.tv_sec) * 1_000_000_000 + Int64(ts.tv_nsec)
                // Half a second of tolerance on the reference, and steer only on an
                // observation the sync ACCEPTED: the desired depth builds on the current one,
                // so re-requesting it against a frozen offset walks the ring to its cap.
                let accepted = av.observe(AvSync.Observation(
                    ptsNs: pcm.ptsNs, nowLocalNs: nowNs,
                    clockOffsetNs: connection.clockOffsetNs, bufferedAhead: depth,
                    videoE2eNs: videoLatency.latestSample(asOfNs: nowNs, maxAgeMs: 500)))
                if accepted != nil {
                    ring.setSyncTarget(av.desiredDepth(currentDepth: depth))
                }
                ring.noteAvOffset(av.offsetMS)
            }
            pcm.samples.withUnsafeBufferPointer { p in
                if let base = p.baseAddress {
                    ring.write(base, count: pcm.frameCount * pcm.channels)
                }
            }
            // Periodic vitals (~10 s at the protocol's 5 ms frames; proportionally sooner on a
            // lossless plane, whose frames are 2–4 ms). The other three clients log buffer
            // depth and underruns; without this an Apple audio report — latency or dropout —
            // arrives with no numbers at all, which is the position every platform was in
            // before the 2026-08 audio work. `plc_ms` rides along because a healthy
            // `underruns` bought with a climbing `plc_ms` is a link in trouble, not a link
            // that is fine. `rate_hz`/`frame_us` lead it so a field log says which plane the
            // session was on, and on what frame the shed and target floor were sized, without
            // needing the connect lines above it.
            drained += 1
            if drained % 2_000 == 0 {
                let s = ring.stats
                log.info(
                    "audio: rate_hz=\(rateHz) frame_us=\(frameUs) buffer_ms=\(s.bufferedMS) target_ms=\(s.targetMS) underruns=\(s.underruns) drift_sheds=\(s.sheds) drift_inserts=\(s.inserts) av_offset_ms=\(s.avOffsetMS) plc_ms=\(s.plcMS)"
                )
            }
            return true
            }
        }
    }
        thread.name = "punktfunk-audio"
        thread.qualityOfService = .userInteractive
        thread.start()
    }
}
