package io.unom.punktfunk

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import io.unom.punktfunk.kit.security.KnownHostStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * A host that answered the probe somewhere else has moved, and every dial reads the saved
 * address — so the record follows it, by fingerprint, keeping everything the user set on it.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36]) // Robolectric 4.16 has no SDK 37 image yet; the app targets 37
class HostAddressLearnTest {
    private val context: Context get() = ApplicationProvider.getApplicationContext()
    private val fp = "ab".repeat(32)

    @Test
    fun a_pinned_host_follows_the_address_that_answered() {
        val store = KnownHostStore(context)
        val saved = store.trust("192.168.1.9", 9777, "Desk", fp, paired = true)
        store.save(saved.copy(mac = listOf("aa:bb:cc:dd:ee:ff"), mgmtPort = 47991))
        assertTrue(store.learnAddress(fp.uppercase(), "192.168.1.20", 9777))
        val moved = store.byId(saved.id)!!
        assertEquals("192.168.1.20", moved.address)
        assertEquals(listOf("aa:bb:cc:dd:ee:ff"), moved.mac)
        assertEquals(47991, moved.mgmtPort)
        assertEquals(1, store.all().size)
        // Unchanged is a no-op; an unpinned record is named by its address and never moves.
        assertFalse(store.learnAddress(fp, "192.168.1.20", 9777))
        store.trust("192.168.1.30", 9777, "Sofa", "", paired = false)
        assertFalse(store.learnAddress("", "192.168.1.31", 9777))
    }
}
