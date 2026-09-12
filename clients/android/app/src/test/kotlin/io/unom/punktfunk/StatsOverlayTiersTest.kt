package io.unom.punktfunk

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Which counters each HUD tier draws. `lost` is the network's share and renders from NORMAL up;
 * `skipped`, `FEC` and the cadence line are pipeline holds the client cannot act on and read as
 * faults there, so they are DETAILED-only. `sdk = [36]`: Robolectric's newest android-all jar.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class StatsOverlayTiersTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** The ShotScenes window (lost 2 · skipped 1 · FEC 5 of 238) plus judder 78‰ · coalesced 3. */
    private val stats = doubleArrayOf(
        238.0, 921.4, 1.3, 2.1, 1.0, 1.0, 5120.0, 1440.0, 240.0, 2.0,
        10.0, 9.0, 16.0, 1.0, 0.9, 0.4, 0.6, 0.3,
        2.0, 1.0, 5.0, 238.0,
        1.0, 0.5, 1.8, 2.6,
        0.2, 0.3, 236.0, 1.0,
        0.1, 0.3, 0.0,
        28.0, 4.0,
        0.0, 48_000.0, 16.0,
        78.0, 3.0,
    )

    private fun show(verbosity: StatsVerbosity) {
        compose.setContent {
            StatsOverlay(stats, verbosity, decoderLabel = "c2.qti.hevc.decoder", codecLabel = "HEVC")
        }
    }

    @Test
    fun normalShowsLostAlone() {
        show(StatsVerbosity.NORMAL)
        compose.onNodeWithText("lost 2 (0.8%)").assertExists()
        compose.onNodeWithText("skipped", substring = true).assertDoesNotExist()
        compose.onNodeWithText("judder", substring = true).assertDoesNotExist()
    }

    @Test
    fun detailedShowsThePipelineCounters() {
        show(StatsVerbosity.DETAILED)
        compose.onNodeWithText("lost 2 (0.8%) · skipped 1 · FEC 5").assertExists()
        compose.onNodeWithText("judder 78‰ · coalesced 3").assertExists()
    }

    /** One line for the decoder and its feed; the equation carries its four terms whole. */
    @Test
    fun detailedDrawsOneLinePerIdea() {
        show(StatsVerbosity.DETAILED)
        compose.onNodeWithText("c2.qti.hevc.decoder · HEVC · 10-bit · HDR (BT.2020 PQ) · 4:2:0")
            .assertExists()
        compose.onNodeWithText("= host 0.6 + network 0.3 + decode 0.4 + display 0.2   · presents 236")
            .assertExists()
    }
}
