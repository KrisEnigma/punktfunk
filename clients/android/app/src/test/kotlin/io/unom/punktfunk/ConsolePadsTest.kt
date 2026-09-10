package io.unom.punktfunk

import io.unom.punktfunk.console.ConsoleJson
import io.unom.punktfunk.kit.Gamepad
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A captured or ungranted Steam Controller 2 has no `InputDevice`, so `Gamepad.pads()` cannot
 * see it and the console's chip fell back to "TV remote". The extras list is its only way in.
 */
class ConsolePadsTest {
    @Test
    fun aCapturedSc2NamesTheChipWithNoInputDevice() {
        val json = ConsoleJson.pads(
            emptyList(),
            null,
            listOf(
                ConsoleJson.ExtraPad(
                    name = "Steam Controller 2 Puck",
                    key = "sc2:10462:4868",
                    pref = Gamepad.PREF_STEAMCONTROLLER2_PUCK,
                    detail = "28DE:1304 · usb",
                    forwarded = true,
                ),
            ),
        )
        val j = JSONObject(json)
        assertEquals("Steam Controller 2 Puck", j.getString("label"))
        assertEquals(Gamepad.PREF_STEAMCONTROLLER2_PUCK, j.getInt("pref"))
        val pads = j.getJSONArray("pads")
        assertEquals(1, pads.length())
        assertEquals("Steam Controller 2 Puck", pads.getJSONObject(0).getString("name"))
        assertTrue(pads.getJSONObject(0).getBoolean("forwarded"))
    }

    @Test
    fun noExtrasLeavesTheChipEmpty() {
        val j = JSONObject(ConsoleJson.pads(emptyList(), null))
        assertTrue(j.isNull("label"))
        assertTrue(j.isNull("pref"))
        assertEquals(0, j.getJSONArray("pads").length())
    }
}
