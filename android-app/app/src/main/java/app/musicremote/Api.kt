package app.musicremote

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import java.io.File
import java.io.IOException
import java.util.concurrent.TimeUnit

private val json = Json { ignoreUnknownKeys = true; coerceInputValues = true }
private val JSON_TYPE = "application/json".toMediaType()

/** Short timeouts: away from home this is the question "is my server here?" and it must not hang the app. */
private val probeClient = OkHttpClient.Builder().connectTimeout(1500, TimeUnit.MILLISECONDS).readTimeout(1500, TimeUnit.MILLISECONDS)
    .callTimeout(3, TimeUnit.SECONDS).build()

private val client = OkHttpClient.Builder().connectTimeout(5, TimeUnit.SECONDS).readTimeout(15, TimeUnit.SECONDS).build()

/** The server sends an SSE keep-alive every ~15 s: a normal read timeout would kill the stream, so no timeout here. */
private val sseClient = client.newBuilder().readTimeout(0, TimeUnit.SECONDS).build()

/** Downloads can be long: a generous read timeout (the normal client gives up after 15 s of silence). */
private val downloadClient = client.newBuilder().readTimeout(60, TimeUnit.SECONDS).build()

class ApiException(message: String) : Exception(message)

/** Thin client for the music server (see music-server/README.md). `base` looks like http://192.168.1.50:8765 */
class Api(val base: String) {

    private suspend fun <T> get(path: String, parse: (String) -> T): T = withContext(Dispatchers.IO) {
        client.newCall(Request.Builder().url("$base$path").build()).execute().use { r -> parse(body(r)) }
    }

    private fun body(r: Response): String {
        val text = r.body.string()
        if (!r.isSuccessful) throw ApiException("HTTP ${r.code}")
        return text
    }

    private suspend fun post(path: String, payload: JsonObject? = null) = withContext(Dispatchers.IO) {
        val req = Request.Builder().url("$base$path")
            .post((payload?.toString() ?: "{}").toRequestBody(JSON_TYPE)).build()
        client.newCall(req).execute().use { r -> if (!r.isSuccessful) throw ApiException(errorText(r)) }
    }

    private fun errorText(r: Response): String =
        runCatching { (json.parseToJsonElement(r.body.string()) as JsonObject)["error"]?.let { (it as JsonPrimitive).content } }
            .getOrNull() ?: "HTTP ${r.code}"

    private fun url(path: String, vararg q: Pair<String, Any?>): String {
        val b = "$base$path".toHttpUrl().newBuilder()
        q.forEach { (k, v) -> if (v != null) b.addQueryParameter(k, v.toString()) }
        return b.build().toString().removePrefix(base)
    }

    /**
     * Is the music server really there? Checks the body of /health, not just a connection: on another network the same
     * address (192.168.1.50) can belong to a different device.
     */
    suspend fun probe(): Boolean = withContext(Dispatchers.IO) {
        runCatching {
            probeClient.newCall(Request.Builder().url("$base/health").build()).execute().use { r ->
                r.isSuccessful && (r.body.string().replace(" ", "") == """{"status":"ok"}""")
            }
        }.getOrDefault(false)
    }

    suspend fun health(): Boolean = runCatching { get("/health") { true } }.getOrDefault(false)

    suspend fun albums(limit: Int, offset: Int, sort: String, artist: String? = null): AlbumsPage =
        get(url("/albums", "limit" to limit, "offset" to offset, "sort" to sort, "artist" to artist)) {
            json.decodeFromString<AlbumsPage>(it)
        }

    suspend fun album(id: Long): AlbumDetail = get("/albums/$id") { json.decodeFromString<AlbumDetail>(it) }

    suspend fun artists(): ArtistsPage = get("/artists") { json.decodeFromString<ArtistsPage>(it) }

    suspend fun search(q: String, limit: Int = 12): SearchResult =
        get(url("/search", "q" to q, "limit" to limit)) { json.decodeFromString<SearchResult>(it) }

    /** Tracks of the library that sound like `trackId`, never the ones in `exclude` (what is already queued). */
    suspend fun recommendations(trackId: Long, limit: Int = 20, exclude: List<Long> = emptyList()): Recommendations =
        get(url("/recommendations", "track_id" to trackId, "limit" to limit, "exclude" to exclude.joinToString(",").ifEmpty { null })) {
            json.decodeFromString<Recommendations>(it)
        }

    suspend fun player(): PlayerSnapshot = get("/player") { json.decodeFromString<PlayerSnapshot>(it) }

    /**
     * Save `path` (e.g. /stream/12.mp3) into `dest`. Fails early when the phone lacks room, deletes the partial file on
     * any failure or cancellation. `onBytes` is called with the size of each chunk written.
     */
    suspend fun downloadTo(path: String, dest: File, minFreeBytes: Long = 300L * 1024 * 1024, onBytes: (Long) -> Unit = {}) =
        withContext(Dispatchers.IO) {
            downloadClient.newCall(Request.Builder().url("$base$path").build()).execute().use { r ->
                if (!r.isSuccessful) throw ApiException("HTTP ${r.code}")
                val length = r.body.contentLength()
                val room = (dest.parentFile ?: dest).usableSpace
                if (length > 0 && room < length + minFreeBytes) throw IOException("Espace insuffisant sur le téléphone")
                try {
                    r.body.byteStream().use { input ->
                        dest.outputStream().use { out ->
                            val buf = ByteArray(64 * 1024)
                            while (true) {
                                ensureActive()
                                val n = input.read(buf)
                                if (n < 0) break
                                out.write(buf, 0, n)
                                onBytes(n.toLong())
                            }
                        }
                    }
                } catch (t: Throwable) {
                    dest.delete()
                    throw t
                }
            }
        }

    /** Only request a cover when `has_cover` is true: the server answers 404 otherwise. size in 64, 150, 300, 600. */
    fun coverUrl(albumId: Long, size: Int) = "$base/cover/$albumId?size=$size"

    // ---- player commands (the server answers 202 immediately; the real state arrives on the SSE stream) ----
    suspend fun playAlbum(albumId: Long, start: Int = 0) =
        post("/player/play", buildJsonObject { put("album_id", albumId); put("start", start) })

    suspend fun playTracks(ids: List<Long>, start: Int = 0) =
        post("/player/play", buildJsonObject {
            put("track_ids", buildJsonArray { ids.forEach { add(JsonPrimitive(it)) } })
            put("start", start)
        })

    suspend fun enqueue(ids: List<Long>) =
        post("/player/enqueue", buildJsonObject { put("track_ids", buildJsonArray { ids.forEach { add(JsonPrimitive(it)) } }) })

    suspend fun jump(index: Int) = post("/player/jump", buildJsonObject { put("index", index) })
    suspend fun pause() = post("/player/pause")
    suspend fun resume() = post("/player/resume")
    suspend fun next() = post("/player/next")
    suspend fun previous() = post("/player/previous")
    suspend fun stop() = post("/player/stop")
    suspend fun volume(level: Int) = post("/player/volume", buildJsonObject { put("level", level) })
    suspend fun mute(on: Boolean) = post("/player/mute", buildJsonObject { put("mute", on) })

    /**
     * Move inside the current track. The Denon cannot seek in a stream, so the server restarts the stream at the
     * wanted frame: expect about one second of silence.
     */
    suspend fun seekTo(positionMs: Long) = post("/player/seek", buildJsonObject { put("position_ms", positionMs) })
    suspend fun seekBy(deltaMs: Long) = post("/player/seek", buildJsonObject { put("delta_ms", deltaMs) })

    /** Player snapshots as they change. The flow ends when the connection drops: the caller reconnects. */
    fun events(): Flow<PlayerSnapshot> = callbackFlow {
        val req = Request.Builder().url("$base/player/events").header("Accept", "text/event-stream").build()
        val source = EventSources.createFactory(sseClient).newEventSource(req, object : EventSourceListener() {
            override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                runCatching { json.decodeFromString<PlayerSnapshot>(data) }.onSuccess { trySend(it) }
            }
            override fun onClosed(eventSource: EventSource) { close() }
            override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) { close() }
        })
        awaitClose { source.cancel() }
    }
}
