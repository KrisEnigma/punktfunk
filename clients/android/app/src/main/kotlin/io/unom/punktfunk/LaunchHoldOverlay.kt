package io.unom.punktfunk

import android.os.SystemClock
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.ImageLoader
import coil.compose.AsyncImage
import coil.request.ImageRequest
import io.unom.punktfunk.kit.library.GameEntry
import io.unom.punktfunk.kit.library.LibraryClient
import io.unom.punktfunk.kit.library.mtlsHttpClient
import io.unom.punktfunk.kit.security.IdentityStore
import io.unom.punktfunk.kit.security.obtainIdentity
import io.unom.punktfunk.models.LaunchHold
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext

/**
 * How long the hold waits on a title the host still calls `launching` (a cold Steam boot with
 * shader work runs to minutes), and on one the host never lists (the launch did not resolve).
 */
private const val LAUNCH_HOLD_MAX_S = 120.0
private const val LAUNCH_NO_LEASE_S = 15.0

/** The cover's flight: `response ≈ 0.75 s`, loose enough that the turn reads on the way. */
private const val FLIGHT_STIFFNESS = 70f
private const val FLIGHT_DAMPING = 0.72f

/**
 * Where each shelf tile last drew, in window coordinates, by library id.
 *
 * The launch hold's cover flies out of the tile the player picked, and by then that tile is gone —
 * the shelf is replaced by the stream. So the rect is recorded as the shelf lays out and read once,
 * at the tap. A tile that scrolled away keeps its last rect; the hold checks it is still on screen
 * and otherwise just scales the cover up in place.
 */
object TileFrames {
    private val frames = mutableMapOf<String, Rect>()

    fun record(id: String, rect: Rect) {
        frames[id] = rect
    }

    fun rect(id: String): Rect? = frames[id]?.takeIf { it.width > 1f && it.height > 1f }
}

/**
 * The launch hold: the picked title's cover leaves its shelf tile and holds the screen until the
 * game is actually up.
 *
 * Opaque, so the launcher and desktop behind it are never seen — that is the whole job. Polls the
 * host's `/status` once a second (the Resume badge's lane) and calls [onShow] once the title leaves
 * `launching`, or when the host will never say; a tap shows the stream anyway.
 */
@Composable
fun LaunchHoldOverlay(hold: LaunchHold, onShow: () -> Unit) {
    val context = LocalContext.current
    val density = LocalDensity.current
    var loader by remember(hold) { mutableStateOf<ImageLoader?>(null) }
    val flight = remember(hold) { Animatable(0f) }
    var rootInWindow by remember(hold) { mutableStateOf(Rect.Zero) }

    LaunchedEffect(hold) {
        flight.animateTo(
            1f,
            spring(dampingRatio = FLIGHT_DAMPING, stiffness = FLIGHT_STIFFNESS),
        )
    }
    LaunchedEffect(hold) {
        val id = withContext(Dispatchers.IO) {
            runCatching { obtainIdentity(IdentityStore(context)) }.getOrNull()
        }
        if (id == null) {
            onShow()
            return@LaunchedEffect
        }
        loader = ImageLoader.Builder(context)
            .okHttpClient(mtlsHttpClient(id.certPem, id.privateKeyPem, hold.address, hold.fpHex))
            .build()
        val began = SystemClock.elapsedRealtime()
        while (isActive) {
            val games = withContext(Dispatchers.IO) {
                LibraryClient.fetchRunning(
                    hold.address, hold.mgmtPort, id.certPem, id.privateKeyPem, hold.fpHex,
                )
            }
            val state = games.firstOrNull { it.appId == hold.game.id }?.state
            val elapsed = (SystemClock.elapsedRealtime() - began) / 1000.0
            val done = when (state) {
                "launching" -> elapsed >= LAUNCH_HOLD_MAX_S
                null -> elapsed >= LAUNCH_NO_LEASE_S
                // running, exited, untracked, grace: the host has said all it will.
                else -> true
            }
            if (done) {
                onShow()
                return@LaunchedEffect
            }
            delay(1_000)
        }
    }

    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .onGloballyPositioned { rootInWindow = it.boundsInWindow() }
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onShow,
            ),
    ) {
        val w = with(density) { maxWidth.toPx() }
        val h = with(density) { maxHeight.toPx() }
        // Where the cover settles: a 2:3 card on the upper-middle third.
        val ch = minOf(h * 0.40f, with(density) { 300.dp.toPx() })
        val cw = ch * 2f / 3f
        val settled = Rect(
            left = (w - cw) / 2f, top = h * 0.42f - ch / 2f,
            right = (w + cw) / 2f, bottom = h * 0.42f + ch / 2f,
        )
        // Where it flies from: the tile the player tapped, in this overlay's own space. With no
        // usable tile the cover just arrives, a little small, rather than flying in from nowhere.
        val source = hold.sourceRect
            ?.translate(-rootInWindow.left, -rootInWindow.top)
            ?.takeIf { it.overlaps(Rect(-80f, -80f, w + 80f, h + 80f)) }
        val start = source ?: Rect(
            left = settled.left + settled.width * 0.07f,
            top = settled.top + settled.height * 0.07f,
            right = settled.right - settled.width * 0.07f,
            bottom = settled.bottom - settled.height * 0.07f,
        )
        val p = flight.value
        val card = Rect(
            left = start.left + (settled.left - start.left) * p,
            top = start.top + (settled.top - start.top) * p,
            right = start.right + (settled.right - start.right) * p,
            bottom = start.bottom + (settled.bottom - start.bottom) * p,
        )
        // The backdrop closes over the shelf while the cover is still crossing it, so the cover is
        // seen LEAVING its tile rather than appearing on a screen that already replaced it.
        val veil = p.coerceIn(0f, 1f)

        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = veil))) {
            loader?.let { l ->
                // The title's own art, thrown out of focus behind it — the game colours the room
                // it is starting in.
                AsyncImage(
                    model = ImageRequest.Builder(context)
                        .data(hold.game.art.posterCandidates.firstOrNull()).build(),
                    imageLoader = l,
                    contentDescription = null,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .fillMaxSize()
                        .graphicsLayer { alpha = 0.35f * veil; scaleX = 1.25f; scaleY = 1.25f }
                        .blur(60.dp),
                )
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.45f * veil)))
            }
        }
        Box(
            Modifier
                .offset { IntOffset(card.left.toInt(), card.top.toInt()) }
                .size(
                    with(density) { card.width.toDp() },
                    with(density) { card.height.toDp() },
                )
                // One full turn on the way over — around the card's own vertical axis, so it
                // reads as a cover turning rather than a picture spinning flat.
                .graphicsLayer {
                    rotationY = 360f * p
                    // Viewer distance in pixels, taken from the card rather than a constant, so
                    // the turn has the same depth at any size — a couple of card widths, which
                    // is the Apple hold's `CoverFlip.distance`.
                    cameraDistance = 2.2f * size.width
                }
                .shadow(24.dp, RoundedCornerShape(14.dp))
                .clip(RoundedCornerShape(14.dp))
                .background(Color(0xFF1E1E26)),
            contentAlignment = Alignment.Center,
        ) {
            loader?.let { HoldPosterArt(hold.game, it) }
        }
        Column(
            Modifier
                .align(Alignment.TopCenter)
                .padding(top = with(density) { settled.bottom.toDp() } + 28.dp)
                .graphicsLayer { alpha = veil },
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                hold.game.title,
                color = Color.White,
                fontSize = 24.sp,
                fontWeight = FontWeight.SemiBold,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(horizontal = 32.dp),
            )
            Text(
                listOfNotNull(hold.game.storeLabel, hold.game.platform).joinToString(" · "),
                color = Color.White.copy(alpha = 0.6f),
                fontSize = 15.sp,
                modifier = Modifier.padding(top = 6.dp),
            )
            Spacer(Modifier.height(18.dp))
            CircularProgressIndicator(
                modifier = Modifier.size(22.dp),
                color = Color.White,
                strokeWidth = 2.5.dp,
            )
            TextButton(onClick = onShow, modifier = Modifier.padding(top = 10.dp)) {
                Text("Show stream", color = Color.White)
            }
        }
    }
}

/** The shelf's poster: each candidate in turn, the title when none loads. */
@Composable
private fun HoldPosterArt(game: GameEntry, loader: ImageLoader) {
    val candidates = game.art.posterCandidates
    var idx by remember(game.id) { mutableIntStateOf(0) }
    if (idx < candidates.size) {
        AsyncImage(
            model = ImageRequest.Builder(LocalContext.current).data(candidates[idx]).build(),
            imageLoader = loader,
            contentDescription = game.title,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize(),
            onError = { idx++ },
        )
    } else {
        Text(
            game.title,
            color = Color.White.copy(alpha = 0.7f),
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxSize().wrapContentHeight().padding(12.dp),
        )
    }
}
