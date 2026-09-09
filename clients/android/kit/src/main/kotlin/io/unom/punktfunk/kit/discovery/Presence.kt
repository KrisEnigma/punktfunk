package io.unom.punktfunk.kit.discovery

import io.unom.punktfunk.kit.security.KnownHost
import java.util.concurrent.Callable
import java.util.concurrent.Executors

/** Where a saved host answered its last probe. */
data class HostAddr(val address: String, val port: Int)

/**
 * The reachability sweep behind every "online" pip on Android — the touch home and the console
 * share it, so both answer the same way. Presence is the QUIC probe alone: an mDNS advert is a
 * cache entry a suspending host sends no goodbye for.
 *
 * Each host is asked at every address it could be at — the live advert's first (a cold boot can
 * land on a new lease), then the saved one — and the sweep reports WHICH answered, so the caller
 * can re-point the record at it. A live address that stops routing (the device moved from Wi-Fi
 * to Ethernet and the browse still holds what it resolved there) used to be the only one tried,
 * and read as a dead host.
 */
object Presence {
    /** The probe's budget per address. A LAN host answers in milliseconds. */
    const val PROBE_MS = 3_000

    private val pool = Executors.newCachedThreadPool { r ->
        Thread(r, "pf-presence").apply { isDaemon = true }
    }

    /** The addresses to ask, in order: the live advert's, then the saved one if it differs. */
    fun candidates(saved: KnownHost, live: DiscoveredHost?): List<HostAddr> {
        val stored = HostAddr(saved.address, saved.port)
        val advertised = live?.let { HostAddr(it.host, it.port) }
        return if (advertised == null || advertised == stored) listOf(stored) else listOf(advertised, stored)
    }

    /**
     * Probe every host in [saved] and return the address each one answered at, keyed by record
     * id. Hosts run in parallel, so a sweep costs one probe budget, not one per sleeping host.
     * Blocking — call off the main thread.
     */
    fun sweep(
        saved: List<KnownHost>,
        liveFor: (KnownHost) -> DiscoveredHost?,
        probe: (String, Int) -> Boolean,
    ): Map<String, HostAddr> {
        if (saved.isEmpty()) return emptyMap()
        val tasks = saved.map { kh ->
            Callable { candidates(kh, liveFor(kh)).firstOrNull { probe(it.address, it.port) }?.let { kh.id to it } }
        }
        return pool.invokeAll(tasks).mapNotNull { runCatching { it.get() }.getOrNull() }.toMap()
    }
}

/**
 * The online set with one sweep of grace: a host joins on its first answer and leaves after two
 * consecutive misses. One missed probe is not proof of anything — the host may still be tearing
 * down the session this device just left — and a pip that flickers grey between two green sweeps
 * hides the Wake row and reads as a broken network.
 */
class PresenceTracker {
    private val misses = mutableMapOf<String, Int>()

    /** Ids in the online set after this sweep. [probed] is every id asked; [answered] who replied. */
    var online: Set<String> = emptySet()
        private set

    fun apply(probed: Set<String>, answered: Set<String>): Set<String> {
        val next = mutableSetOf<String>()
        for (id in probed) {
            if (id in answered) {
                misses.remove(id)
                next += id
                continue
            }
            val n = (misses[id] ?: 0) + 1
            misses[id] = n
            if (id in online && n < 2) next += id
        }
        misses.keys.retainAll(probed)
        online = next
        return next
    }
}
