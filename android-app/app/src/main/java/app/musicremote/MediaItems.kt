package app.musicremote

import android.net.Uri
import android.os.Bundle
import androidx.core.net.toUri
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata

private const val KEY_ALBUM_ID = "album_id"
private const val KEY_DURATION_MS = "duration_ms"

/**
 * A track as a Media3 item for the phone's own player. Plays the downloaded file when there is one, otherwise
 * streams it from the server (`base` = server address, null when the server is not reachable).
 * Returns null when the track can be played neither way.
 */
fun phoneItem(t: Track, base: String?): MediaItem? {
    val uri: Uri = LocalLibrary.trackFile(t.id)?.toUri() ?: base?.let { "$it/stream/${t.id}.mp3".toUri() } ?: return null
    val artwork: Uri? = LocalLibrary.coverFile(t.albumId)?.toUri()
        ?: if (base != null && PlayerRepository.hasCover(t.albumId)) "$base/cover/${t.albumId}?size=600".toUri() else null
    val extras = Bundle().apply {
        putLong(KEY_ALBUM_ID, t.albumId)
        putLong(KEY_DURATION_MS, t.durationMs ?: 0L)
    }
    return MediaItem.Builder()
        .setMediaId(t.id.toString())
        .setUri(uri)
        .setMediaMetadata(
            MediaMetadata.Builder().setTitle(t.title).setArtist(t.artist).setAlbumTitle(t.albumName)
                .setArtworkUri(artwork).setIsPlayable(true).setExtras(extras).build()
        )
        .build()
}

/** Back from a Media3 item to the shape the UI already knows (the same as the server's queue items). */
fun MediaItem.toQueueItem(): QueueItem {
    val m = mediaMetadata
    val ex = m.extras
    return QueueItem(
        trackId = mediaId.toLongOrNull() ?: 0L,
        albumId = ex?.getLong(KEY_ALBUM_ID) ?: 0L,
        title = m.title?.toString().orEmpty(),
        artist = m.artist?.toString(),
        durationMs = ex?.getLong(KEY_DURATION_MS)?.takeIf { it > 0 },
    )
}
