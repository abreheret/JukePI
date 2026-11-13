package app.musicremote

import android.content.Context
import android.os.SystemClock
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/** Where the music comes out: the Denon (driven through the server) or the phone itself (ExoPlayer). */
enum class Output { Denon, Phone }

/** Is the music server reachable from here? Unknown only until the first probe has answered. */
enum class Server { Unknown, Online, Offline }

/**
 * Process-wide owner of the live player state (the SSE stream from the server).
 *
 * Shared by the UI and by [PlaybackService] (lock screen controls), which must keep working while the app is in the
 * background. The stream is open as long as at least one "holder" needs it: the UI while visible, the service while
 * something is playing or paused.
 */
object PlayerRepository {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _ui = MutableStateFlow(PlayerUi())
    val ui: StateFlow<PlayerUi> = _ui

    /** Snapshot of the phone's own player, published by [PlaybackService] (same shape as the Denon one, so the UI is shared). */
    val phoneUi = MutableStateFlow(PlayerUi())

    val server = MutableStateFlow(Server.Unknown)

    /** Which player the UI and the media session currently drive. */
    val output = MutableStateFlow(Output.Denon)

    var api: Api? = null
        private set

    private val holders = mutableSetOf<String>()
    private var job: Job? = null

    /** album id -> has_cover, learnt from the lists we load, so nobody asks for a cover that does not exist. */
    private val coverFlags = HashMap<Long, Boolean>()
    fun remember(albums: List<Album>) = albums.forEach { coverFlags[it.id] = it.hasCover }
    fun hasCover(albumId: Long): Boolean = coverFlags[albumId] ?: true

    /** Load the saved server address when the process was started by the service rather than by the activity. */
    fun init(ctx: Context) {
        if (api != null) return
        ctx.getSharedPreferences("server", Context.MODE_PRIVATE).getString("address", null)?.let { setApi(Api(it)) }
    }

    fun setApi(a: Api?) {
        if (a?.base == api?.base) return
        api = a
        _ui.value = PlayerUi()
        restart()
    }

    fun hold(owner: String) {
        if (holders.add(owner)) restart()
    }

    fun release(owner: String) {
        if (holders.remove(owner) && holders.isEmpty()) {
            job?.cancel()
            job = null
            _ui.update { it.copy(live = false) }
        }
    }

    private fun restart() {
        job?.cancel()
        job = null
        val a = api ?: return
        if (holders.isEmpty()) return
        job = scope.launch {
            var backoff = 1000L
            while (isActive) {
                try {
                    a.events().collect { s ->
                        _ui.value = PlayerUi(s, SystemClock.elapsedRealtime(), true)
                        backoff = 1000L
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                }
                _ui.update { it.copy(live = false) }
                delay(backoff)
                backoff = (backoff * 2).coerceAtMost(10_000L)
            }
        }
    }
}
