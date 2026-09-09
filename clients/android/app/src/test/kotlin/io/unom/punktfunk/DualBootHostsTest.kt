package io.unom.punktfunk

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import io.unom.punktfunk.kit.discovery.DiscoveredHost
import io.unom.punktfunk.kit.security.KnownHostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * A dual-boot box answers at one lease with one MAC and a certificate per OS. Trusting the second
 * OS used to overwrite the first one's record, and the advert of whichever OS was up read as the
 * host already saved — so it never reached "Discovered" and could not be added at all.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36]) // Robolectric 4.16 has no SDK 37 image yet; the app targets 37
class DualBootHostsTest {
    private val context: Context get() = ApplicationProvider.getApplicationContext()
    private val windows = "ab".repeat(32)
    private val linux = "cd".repeat(32)

    private fun advert(fp: String?, host: String = "192.168.1.9") =
        DiscoveredHost(key = "id-$fp", name = "Desk", host = host, port = 9777, fingerprint = fp)

    @Test
    fun a_second_os_at_one_address_is_a_different_host() {
        val store = KnownHostStore(context)
        val saved = store.trust("192.168.1.9", 9777, "Desk (Windows)", windows, paired = true)
        assertFalse(saved.matches(advert(linux)))
        // The pin decides on its own, so a new DHCP lease is still the same host.
        assertTrue(saved.matches(advert(windows.uppercase(), host = "192.168.1.20")))
        // One side unpinned: the address is all there is to go on.
        assertTrue(saved.matches(advert(null)))
    }

    @Test
    fun trusting_the_second_os_leaves_the_first_alone() {
        val store = KnownHostStore(context)
        val first = store.trust("192.168.1.9", 9777, "Desk (Windows)", windows, paired = true)
        val second = store.trust("192.168.1.9", 9777, "Desk (Linux)", linux, paired = true)
        assertEquals(2, store.all().size)
        assertNotNull("the first OS keeps its record", store.byId(first.id))
        assertEquals(windows, store.byId(first.id)?.fpHex)
        assertEquals("Desk (Windows)", store.byId(first.id)?.name)
        assertEquals(linux, store.getByFp(linux)?.fpHex)
        assertEquals(second.id, store.getByFp(linux)?.id)

        // Re-trusting one of them is still an update in place, not a third record.
        store.trust("192.168.1.9", 9777, "Desk (Linux)", linux, paired = true)
        assertEquals(2, store.all().size)
    }

    /** An empty fingerprint is not a key — it would match the first unpinned placeholder. */
    @Test
    fun an_unpinned_placeholder_takes_the_first_pin_offered_at_its_address() {
        val store = KnownHostStore(context)
        val placeholder = store.trust("192.168.1.9", 9777, "192.168.1.9", "", paired = false)
        assertNull(store.getByFp(""))
        val pinned = store.trust("192.168.1.9", 9777, "Desk", windows, paired = true)
        assertEquals("the placeholder is the record this pin was waiting for", placeholder.id, pinned.id)
        assertEquals(1, store.all().size)
    }
}
