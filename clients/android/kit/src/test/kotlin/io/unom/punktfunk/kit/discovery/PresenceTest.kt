package io.unom.punktfunk.kit.discovery

import io.unom.punktfunk.kit.security.KnownHost
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The reachability sweep behind the online pips. A device that moved from Wi-Fi to Ethernet
 * probed only the address the old browse had resolved, and read a live host as dead.
 */
class PresenceTest {
    private val fp = "ab".repeat(32)
    private val desk = KnownHost("192.168.1.9", 9777, "Desk", fp, paired = true, id = "desk")
    private fun advert(host: String) =
        DiscoveredHost(key = "id", name = "Desk", host = host, port = 9777, fingerprint = fp)

    @Test
    fun the_live_address_is_asked_first_and_the_saved_one_still_asked() {
        assertEquals(
            listOf(HostAddr("192.168.1.20", 9777), HostAddr("192.168.1.9", 9777)),
            Presence.candidates(desk, advert("192.168.1.20")),
        )
        assertEquals(listOf(HostAddr("192.168.1.9", 9777)), Presence.candidates(desk, advert("192.168.1.9")))
        assertEquals(listOf(HostAddr("192.168.1.9", 9777)), Presence.candidates(desk, null))
    }

    /** A stale live address no longer routes; the saved one answers. The host is up, there. */
    @Test
    fun a_host_that_answers_at_its_saved_address_is_up_when_the_advert_is_stale() {
        val up = Presence.sweep(listOf(desk), liveFor = { advert("10.0.0.5") }) { addr, _ -> addr == "192.168.1.9" }
        assertEquals(HostAddr("192.168.1.9", 9777), up["desk"])
    }

    /** A cold boot on a new lease: the advert wins, and the sweep says where. */
    @Test
    fun a_host_on_a_new_lease_is_reported_at_the_address_that_answered() {
        val up = Presence.sweep(listOf(desk), liveFor = { advert("192.168.1.20") }) { addr, _ -> addr == "192.168.1.20" }
        assertEquals(HostAddr("192.168.1.20", 9777), up["desk"])
        assertNull(Presence.sweep(listOf(desk), liveFor = { null }) { _, _ -> false }["desk"])
    }

    @Test
    fun one_missed_probe_keeps_a_host_online_and_two_take_it_down() {
        val t = PresenceTracker()
        assertEquals(setOf("desk"), t.apply(setOf("desk"), setOf("desk")))
        assertEquals(setOf("desk"), t.apply(setOf("desk"), emptySet()))
        assertEquals(emptySet<String>(), t.apply(setOf("desk"), emptySet()))
        // Back on the first answer; a host never seen up gets no grace.
        assertEquals(setOf("desk"), t.apply(setOf("desk", "sofa"), setOf("desk")))
        assertEquals(emptySet<String>(), t.apply(setOf("sofa"), emptySet()))
    }
}
