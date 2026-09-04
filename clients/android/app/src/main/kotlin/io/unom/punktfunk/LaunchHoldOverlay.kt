package io.unom.punktfunk

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
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

/**
 * The launch hold: the launched title's poster over the live stream until its game is up.
 *
 * Opaque, so the launcher and desktop behind never show — every launch used to open on them. Polls
 * the host's `/status` once a second (the Resume badge's lane) and calls [onShow] once the title
 * leaves `launching`, or when the host will never say; a tap shows the stream anyway.
 */
@Composable
fun LaunchHoldOverlay(hold: LaunchHold, onShow: () -> Unit) {
    val context = LocalContext.current
    var loader by remember(hold) { mutableStateOf<ImageLoader?>(null) }
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
                LibraryClient.fetchRunning(hold.address, hold.mgmtPort, id.certPem, id.privateKeyPem, hold.fpHex)
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
    Box(
        Modifier
            .fillMaxSize()
            .background(Color(0xFF0C0C11))
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onShow),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Box(
                Modifier
                    .fillMaxHeight(0.4f)
                    .aspectRatio(2f / 3f)
                    .shadow(24.dp, RoundedCornerShape(14.dp))
                    .clip(RoundedCornerShape(14.dp))
                    .background(Color(0xFF1E1E26)),
                contentAlignment = Alignment.Center,
            ) {
                loader?.let { HoldPosterArt(hold.game, it) }
            }
            Spacer(Modifier.height(28.dp))
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
            CircularProgressIndicator(
                modifier = Modifier.padding(top = 22.dp).size(24.dp),
                color = Color.White,
                strokeWidth = 2.5.dp,
            )
            TextButton(onClick = onShow, modifier = Modifier.padding(top = 12.dp)) {
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
