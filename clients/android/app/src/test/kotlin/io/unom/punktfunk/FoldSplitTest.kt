package io.unom.punktfunk

import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Pure JVM test of [foldSplit] — which postures earn the tabletop split and where it cuts.
 * Run: `./gradlew :app:testDebugUnitTest`.
 */
class FoldSplitTest {
    @Test
    fun aCreaseAcrossTheMiddleSplitsWithNoGap() {
        // A Pixel Fold in tabletop: the crease is a zero-height line across the whole width.
        val s = foldSplit(IntRect(0, 920, 2208, 920), IntSize(2208, 1840))
        assertEquals(FoldSplit(920, 0), s)
    }

    @Test
    fun aHingeWithThicknessKeepsItsStripOutOfBothHalves() {
        val s = foldSplit(IntRect(0, 900, 1800, 984), IntSize(1800, 1920))
        assertEquals(FoldSplit(900, 84), s)
    }

    @Test
    fun bookPostureDoesNotSplit() {
        // A vertical hinge folds left/right — there is no flat half to rest a pad on.
        assertNull(foldSplit(IntRect(1104, 0, 1104, 1840), IntSize(2208, 1840)))
    }

    @Test
    fun aHingeNearAnEdgeLeavesNothingWorthSplittingInto() {
        assertNull(foldSplit(IntRect(0, 100, 2208, 100), IntSize(2208, 1840)))
        assertNull(foldSplit(IntRect(0, 1740, 2208, 1740), IntSize(2208, 1840)))
    }

    @Test
    fun anUnmeasuredContainerDoesNotSplit() {
        assertNull(foldSplit(IntRect(0, 920, 2208, 920), IntSize.Zero))
    }
}
