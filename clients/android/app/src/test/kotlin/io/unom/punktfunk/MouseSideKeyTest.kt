package io.unom.punktfunk

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** [isMouseSideKey]: which Back keys reach the host as X1 instead of opening the ring. */
class MouseSideKeyTest {
    private fun claim(
        tv: Boolean = false,
        external: Boolean = true,
        pad: Boolean = false,
        fallback: Boolean = false,
        mouse: Boolean = true,
        dpad: Boolean = false,
    ) = isMouseSideKey(tv, external, pad, fallback, mouse, dpad)

    @Test
    fun offTvEveryExternalBackIsTheHosts() {
        assertTrue("plain mouse", claim())
        assertTrue("keyboard-and-mouse combo", claim(dpad = true))
        assertTrue("consumer-page node without a mouse source", claim(mouse = false))
    }

    @Test
    fun theRingKeepsTheGestureAndThePad() {
        assertFalse("nav bar or gesture Back", claim(external = false))
        assertFalse("a pad's Select-as-Back", claim(pad = true))
        assertFalse("the framework's fallback duplicate", claim(fallback = true))
    }

    @Test
    fun onTvARemoteKeepsItsBack() {
        assertTrue("mouse", claim(tv = true))
        assertFalse("air-mouse remote", claim(tv = true, dpad = true))
        assertFalse("D-pad remote", claim(tv = true, mouse = false, dpad = true))
    }
}
