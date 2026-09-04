package io.unom.punktfunk

import android.app.Activity
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.window.layout.FoldingFeature
import androidx.window.layout.WindowInfoTracker
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

/*
 * Tabletop foldable (design/touch-client-overlay.md §4.4). A book foldable half-opened on a table
 * is two screens, and a touch session has two things that each want one: the picture on the
 * upright half, the virtual pad on the flat half where the thumbs already rest. The split is not
 * a mode the user turns on — the posture and the pad being up are the whole of the trigger, so
 * folding the device flat again puts the picture back over the full panel with nothing to undo.
 */

/** The tabletop halves in px: the picture keeps [videoPx], the hinge eats [hingePx], the pad takes the rest. */
internal data class FoldSplit(val videoPx: Int, val hingePx: Int)

/**
 * The split for a half-opened hinge at [hinge] across a [size] container, or null when this fold
 * cannot carry one. A hinge that does not span the full width folds the screen left/right (book
 * posture, held like a paperback — no flat half to put a pad on), and a hinge close to either
 * edge leaves a half that is too small to be either a picture or a controller.
 *
 * Both rects are window px: the stream screen is edge-to-edge with the system bars hidden, so the
 * window and this container are the same rectangle.
 */
internal fun foldSplit(hinge: IntRect, size: IntSize): FoldSplit? {
    if (size.width <= 0 || size.height <= 0) return null
    if (hinge.left > 0 || hinge.right < size.width) return null
    val video = hinge.top.coerceIn(0, size.height)
    val gap = (hinge.bottom - hinge.top).coerceIn(0, size.height - video)
    val min = size.height / 5
    if (video < min || size.height - video - gap < min) return null
    return FoldSplit(video, gap)
}

/**
 * The half-opened hinge across this window, or null while the device is flat, shut, or not a
 * foldable at all. Recomposes as the hinge moves, so opening the device mid-stream splits the
 * screen and closing it joins it again.
 */
@Composable
internal fun rememberFoldHinge(): IntRect? {
    val activity = LocalContext.current as? Activity
    val hinges = remember(activity) {
        if (activity == null) {
            flowOf(null)
        } else {
            WindowInfoTracker.getOrCreate(activity).windowLayoutInfo(activity).map { info ->
                info.displayFeatures.filterIsInstance<FoldingFeature>()
                    .firstOrNull { it.state == FoldingFeature.State.HALF_OPENED }
                    ?.bounds
                    ?.let { IntRect(it.left, it.top, it.right, it.bottom) }
            }
        }
    }
    return hinges.collectAsState(null).value
}
