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
 * Each host is asked at its saved address first, then at the live advert's, and the sweep
 * reports WHICH answered, so the caller can re-point the record at it. A saved address that
 * answers is never replaced: a routed one (Tailscale, VPN) answers on the LAN too, and the advert
 * would swap it for one that stops working off the home network. The advert still finds a host
 * that came back on a new lease.
 */
object Presence {
    /** The probe's budget per address. A LAN host answers in milliseconds. */
    const val PROBE_MS = 3_000

    private val pool = Executors.newCachedThreadPool { r ->
        Thread(r, "pf-presence").apply { isDaemon = true }
    }

    /** The addresses to ask, in order: the saved one, then the live advert's if it differs. */
    fun candidates(saved: KnownHost, live: DiscoveredHost?): List<HostAddr> {
        val stored = HostAddr(saved.address, saved.port)
        val advertised = live?.let { HostAddr(it.host, it.port) }
        return if (advertised == null || advertised == stored) listOf(stored) else listOf(stored, advertised)
    }

    /**
     * Is [answered] — the fingerprint that replied to a probe, or `null` when nothing did —
     * [saved] itself?
     *
     * An address is not an identity. Whoever inherits a sleeping host's DHCP lease answers at
     * it, and counting that as the host lights the pip and, since wake is gated on `!online`,
     * silences Wake-on-LAN for exactly the machine that needs it; both OS installs of a
     * dual-boot box share one lease the same way. A record saved by address alone carries no
     * pin and has nothing to compare, so any answer is the host it names.
     */
    fun isSelf(pinHex: String, answered: String?): Boolean =
        answered != null && (pinHex.isEmpty() || pinHex.equals(answered, ignoreCase = true))

    /** [isSelf] for a saved record, which carries its own pin. */
    fun isSelf(saved: KnownHost, answered: String?): Boolean = isSelf(saved.fpHex, answered)

    /**
     * Probe every host in [saved] and return the address each one answered AT AND AS ITSELF,
     * keyed by record id. Hosts run in parallel, so a sweep costs one probe budget, not one per
     * sleeping host. Blocking — call off the main thread.
     */
    fun sweep(
        saved: List<KnownHost>,
        liveFor: (KnownHost) -> DiscoveredHost?,
        probe: (String, Int) -> String?,
    ): Map<String, HostAddr> {
        if (saved.isEmpty()) return emptyMap()
        val tasks = saved.map { kh ->
            Callable {
                candidates(kh, liveFor(kh))
                    .firstOrNull { isSelf(kh, probe(it.address, it.port)) }
                    ?.let { kh.id to it }
            }
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
