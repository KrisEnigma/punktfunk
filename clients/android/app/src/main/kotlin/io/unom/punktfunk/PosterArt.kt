package io.unom.punktfunk

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import coil.ImageLoader
import coil.compose.AsyncImage
import coil.request.ImageRequest
import io.unom.punktfunk.kit.library.GameEntry

/**
 * The poster-candidate walk every poster surface shares: try each URL in order, one at a time,
 * and hand over to [fallback] once the last one has failed. The index resets with the game.
 */
@Composable
fun PosterArt(game: GameEntry, loader: ImageLoader, fallback: @Composable () -> Unit) {
    val candidates = game.art.posterCandidates
    var idx by remember(game.id) { mutableIntStateOf(0) }
    if (idx < candidates.size) {
        AsyncImage(
            model = ImageRequest.Builder(LocalContext.current).data(candidates[idx]).build(),
            imageLoader = loader,
            contentDescription = game.title,
            contentScale = ContentScale.Crop,
            modifier = Modifier.fillMaxSize(),
            onError = { idx++ }, // this candidate failed — try the next, or fall to the placeholder
        )
    } else {
        fallback()
    }
}
