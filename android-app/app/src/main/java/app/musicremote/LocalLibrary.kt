package app.musicremote

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.File

/** An album on the phone. `complete` = every track is there; an interrupted download resumes where it stopped. */
data class DownloadedAlbum(val detail: AlbumDetail, val complete: Boolean, val bytes: Long, val tracksOnPhone: Int) {
    val id: Long get() = detail.album.id
}

data class DownloadProgress(val done: Int, val total: Int, val queued: Boolean = false, val error: String? = null)

/**
 * Albums downloaded to the phone, in the app's own storage (`getExternalFilesDir`): never evicted like a cache would
 * be, no storage permission needed, removed with the app.
 *
 * Layout: `music/album_<id>/` with `album.json` (the album and its tracks, as the server sent them), `cover.jpg`
 * (600 px), `t<trackId>.mp3`, and a `.complete` marker written only once every track is there. Files are written as
 * `*.part` and renamed, so a file with its final name is always whole.
 */
object LocalLibrary {
    private val json = Json { ignoreUnknownKeys = true }
    private var root: File? = null

    val albums = MutableStateFlow<List<DownloadedAlbum>>(emptyList())
    val progress = MutableStateFlow<Map<Long, DownloadProgress>>(emptyMap())

    @Volatile private var trackFiles: Map<Long, File> = emptyMap()

    fun init(ctx: Context) {
        if (root != null) return
        root = File(ctx.getExternalFilesDir(null) ?: ctx.filesDir, "music").apply { mkdirs() }
        rescan()
    }

    private fun albumDir(id: Long) = File(root!!, "album_$id")

    fun trackFile(trackId: Long): File? = trackFiles[trackId]
    fun coverFile(albumId: Long): File? = root?.let { File(albumDir(albumId), "cover.jpg").takeIf { f -> f.exists() } }
    fun isComplete(albumId: Long): Boolean = albums.value.any { it.id == albumId && it.complete }
    fun detail(albumId: Long): AlbumDetail? = albums.value.firstOrNull { it.id == albumId }?.detail

    /** Rebuild the in-memory view from the disk (cheap: a few small JSON files and a directory listing). */
    fun rescan() {
        val dirs = root?.listFiles { f -> f.isDirectory && f.name.startsWith("album_") } ?: return
        val files = HashMap<Long, File>()
        val list = dirs.mapNotNull { dir ->
            val detail = runCatching { json.decodeFromString<AlbumDetail>(File(dir, "album.json").readText()) }.getOrNull()
                ?: return@mapNotNull null
            var bytes = 0L
            var present = 0
            for (t in detail.tracks) {
                val f = File(dir, "t${t.id}.mp3")
                if (f.exists()) { files[t.id] = f; bytes += f.length(); present++ }
            }
            DownloadedAlbum(detail, File(dir, ".complete").exists() && present == detail.tracks.size, bytes, present)
        }.sortedBy { it.detail.album.name.lowercase() }
        trackFiles = files
        albums.value = list
    }

    fun delete(albumId: Long) {
        root ?: return
        albumDir(albumId).deleteRecursively()
        progress.update { it - albumId }
        rescan()
    }

    fun markQueued(albumId: Long, total: Int = 0) = progress.update { it + (albumId to DownloadProgress(0, total, queued = true)) }
    fun clearProgress(albumId: Long) = progress.update { it - albumId }
    fun markFailed(albumId: Long, message: String) =
        progress.update { it + (albumId to (it[albumId] ?: DownloadProgress(0, 0)).copy(queued = false, error = message)) }

    fun totalBytes(): Long = albums.value.sumOf { it.bytes }

    /** Download (or finish downloading) one album. Cancellable; safe to call again after an interruption. */
    suspend fun download(api: Api, albumId: Long) = withContext(Dispatchers.IO) {
        val detail = api.album(albumId)
        val dir = albumDir(albumId).apply { mkdirs() }
        File(dir, "album.json").writeText(json.encodeToString(detail))
        File(dir, ".complete").delete()
        val total = detail.tracks.size
        progress.update { it + (albumId to DownloadProgress(0, total)) }
        rescan()

        val cover = File(dir, "cover.jpg")
        if (detail.album.hasCover && !cover.exists()) {
            runCatching {
                val part = File(dir, "cover.jpg.part")
                api.downloadTo("/cover/$albumId?size=600", part)
                part.renameTo(cover)
            } // a missing cover must not fail the album
        }

        var done = detail.tracks.count { File(dir, "t${it.id}.mp3").exists() }
        progress.update { it + (albumId to DownloadProgress(done, total)) }
        for (t in detail.tracks) {
            ensureActive()
            val f = File(dir, "t${t.id}.mp3")
            if (f.exists()) continue
            val part = File(dir, "t${t.id}.mp3.part")
            api.downloadTo("/stream/${t.id}.mp3", part)
            part.renameTo(f)
            done++
            progress.update { it + (albumId to DownloadProgress(done, total)) }
            rescan() // the track is playable from now on
        }
        File(dir, ".complete").writeText("1")
        progress.update { it - albumId }
        rescan()
    }
}
