package io.unom.punktfunk

import android.accessibilityservice.AccessibilityService
import android.content.Intent
import android.view.KeyEvent
import android.view.accessibility.AccessibilityEvent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/**
 * Hardware keys ahead of Android's own shortcuts. The window policy takes Alt+Tab, every Meta
 * chord and the Language key before an app sees them; an accessibility service that filters keys
 * runs in the input dispatcher before that policy. The user turns it on under Accessibility.
 *
 * It hands every key to the focused stream ([stream], set by [MainActivity]) and consumes what the
 * stream owns. With no focused stream it touches nothing: other apps' keys pass straight through,
 * and it never reads window content — the config requests key filtering only.
 */
class KeyCaptureService : AccessibilityService() {
    override fun onServiceConnected() {
        running = true
    }

    override fun onUnbind(intent: Intent?): Boolean {
        running = false
        return super.onUnbind(intent)
    }

    override fun onKeyEvent(event: KeyEvent): Boolean = stream?.invoke(event) == true

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    companion object {
        /** The focused stream's key handler; null whenever no stream holds the window. */
        @Volatile
        var stream: ((KeyEvent) -> Boolean)? = null

        /** The user has the service on, so Alt+Tab needs no stand-in and the banner says nothing. */
        var running by mutableStateOf(false)
            private set
    }
}
