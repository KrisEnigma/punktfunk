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
 * The first line's panel warning. Android's per-uid cap (the game default frame rate) leaves the
 * panel at its mode while the app renders slower; a real mode switch moves both. The HUD names
 * the cap, because from the stream alone it looks like skipped frames. `sdk = [36]`: Robolectric's
 * newest android-all jar.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class StatsOverlayPanelTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** A 120 fps stream; only the panel figures vary. */
    private val stats = doubleArrayOf(
        120.0, 80.0, 30.0, 40.0, 1.0, 1.0, 2800.0, 1260.0, 120.0, 0.0,
    )

    private fun show(panelHz: Float, panelModeHz: Float) {
        compose.setContent {
            StatsOverlay(stats, StatsVerbosity.NORMAL, panelHz = panelHz, panelModeHz = panelModeHz)
        }
    }

    @Test
    fun aPerUidCapIsNamed() {
        show(panelHz = 60f, panelModeHz = 120f)
        compose.onNodeWithText("⚠ app capped 60 Hz by the system", substring = true).assertExists()
    }

    @Test
    fun aRealModeSwitchReadsAsThePanel() {
        show(panelHz = 60f, panelModeHz = 60f)
        compose.onNodeWithText("⚠ panel 60 Hz", substring = true).assertExists()
        compose.onNodeWithText("capped", substring = true).assertDoesNotExist()
    }

    @Test
    fun aFullRatePanelWarnsOfNothing() {
        show(panelHz = 120f, panelModeHz = 120f)
        compose.onNodeWithText("⚠", substring = true).assertDoesNotExist()
    }
}
