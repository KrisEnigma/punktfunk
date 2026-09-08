package io.unom.punktfunk

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.unom.punktfunk.kit.NativeBridge
import io.unom.punktfunk.kit.SessionAccess

/**
 * The stream screen's Compose state that the session's peripherals write from their callbacks —
 * held in one object so [StreamPeripherals] can be a plain class over it instead of a closure over
 * the composable's scope. Per session: created with the handle.
 */
internal class StreamUi(
    private val handle: Long,
    initialAccess: IntArray?,
    initialVerbosity: StatsVerbosity,
) {
    /** The session's grants (per-client access), the courtesy mirror of what the host enforces. */
    var accessGrants by mutableIntStateOf(initialAccess?.getOrNull(0) ?: SessionAccess.ALL)

    /** Seconds until this session's access expires (0 = permanent), as last reported natively. */
    var accessRemaining by mutableIntStateOf(initialAccess?.getOrNull(1) ?: 0)

    /**
     * In-stream mic mute. Per SESSION and never persisted: a new stream always starts unmuted. The
     * authoritative flag lives on the native handle, which is why a mute survives the mic
     * stop/start a surface recreate performs — this is the UI's mirror of it.
     */
    var micMuted by mutableStateOf(false)
        private set

    /** Whether a capture is actually RUNNING, not merely wanted — from what nativeMicActive reports. */
    var micRunning by mutableStateOf(false)

    /** Transient confirmation of a mic-chord toggle (null = nothing showing). */
    var micHint by mutableStateOf<String?>(null)

    /** A captured pad has a gyro this session's virtual controller cannot carry. Shown briefly. */
    var motionHint by mutableStateOf(false)

    /**
     * Whether this session has a controller — seeded from the router the moment it is built and
     * latched true by a pad that arrives later; it never goes back to false.
     */
    var padPresent by mutableStateOf(false)

    /** The gamepad exit chord is held and counting down — drives the "hold to quit" hint. */
    var exitArming by mutableStateOf(false)

    /** The TV remote is acting as a pointer (hold SELECT toggles) — drives the mode hint. */
    var remotePointerOn by mutableStateOf(false)

    /** The stats HUD tier, cycled live by the three-finger tap or the Select + X chord. */
    var statsVerbosity by mutableStateOf(initialVerbosity)

    /** The one place mute is toggled — Compose state + the native flag, always together. */
    fun mute(muted: Boolean) {
        micMuted = muted
        NativeBridge.nativeSetMicMuted(handle, muted)
    }
}
