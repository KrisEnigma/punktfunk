package io.unom.punktfunk

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density

/**
 * How much larger than this screen's normal UI the streaming chrome — the stats HUD and the
 * quick-action ring — draws on a TV. The TV density bucket already carries most of the viewing
 * distance: a 1080p set is 960×540 dp, so a 55-inch panel at 3 m and a phone at 35 cm subtend
 * nearly the same angle per dp. 1.25 sits a notch above that parity, and the detailed HUD's widest
 * line still fits the 960 dp width.
 *
 * Physical screen size is deliberately not an input: `DisplayMetrics.xdpi` is invented on many TV
 * boxes, so a screen-inch rule would mis-size the very case this exists for. Android only — the
 * Apple TV client sizes its own chrome (`StreamHUDView`'s tvOS padding and inset).
 */
const val TV_OSD_SCALE = 1.25f

/** The overlay multiplier for this device. 1 anywhere held or sat in front of. */
fun osdScale(context: android.content.Context): Float =
    if (isTvDevice(context)) TV_OSD_SCALE else 1f

/**
 * Draws [content] at this device's overlay scale by scaling [LocalDensity], so every `dp` and `sp`
 * inside grows together — no metric is scaled by hand and none can be missed. `fontScale` passes
 * through untouched: the system text size the user already chose still applies on top of this.
 *
 * [CompositionLocalProvider] emits no layout node, so a `BoxScope.align` built by the caller still
 * lands on the content's own node — the overlays stay where they were placed.
 */
@Composable
fun OsdScaled(content: @Composable () -> Unit) {
    val context = LocalContext.current
    // Device-fixed: the leanback feature and the ui-mode service behind it cannot change at runtime.
    val scale = remember(context) { osdScale(context) }
    val density = LocalDensity.current
    CompositionLocalProvider(
        LocalDensity provides Density(density.density * scale, density.fontScale),
        content = content,
    )
}
