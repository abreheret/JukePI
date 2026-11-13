package app.musicremote

import android.app.Application
import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import android.net.ConnectivityManager
import android.net.Network
import android.os.SystemClock
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

enum class Tab { Albums, Downloaded, Artists, Search }

/** Screens pushed on top of the tabs. Back pops them. */
sealed interface Detail {
    data class AlbumPage(val id: Long, val title: String) : Detail
    data class ArtistPage(val name: String) : Detail
    data object NowPlaying : Detail
    data object Settings : Detail
    data object Downloads : Detail
}

class AppViewModel(app: Application) : AndroidViewModel(app) {
    private val prefs = app.getSharedPreferences("server", Context.MODE_PRIVATE)

    var address by mutableStateOf(prefs.getString("address", null))
        private set
    var api by mutableStateOf(address?.let { Api(it) })
        private set

    /** The output the user chose. The one actually driven may differ: away from home it is the phone. */
    private var preferred: Output = if (prefs.getString("output", "denon") == "phone") Output.Phone else Output.Denon

    init {
        LocalLibrary.init(app)
        PlayerRepository.setApi(api)
        PlayerRepository.output.value = preferred
        startProbing()
        // Re-evaluate the output whenever the server state or what is playing changes.
        viewModelScope.launch {
            combine(PlayerRepository.server, PlayerRepository.ui, PlayerRepository.phoneUi) { _, _, _ -> }.collect { reconcileOutput() }
        }
        val cm = app.getSystemService(ConnectivityManager::class.java)
        cm?.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) { probeNow() }
            override fun onLost(network: Network) { probeNow() }
        })
    }
    val server: StateFlow<Server> = PlayerRepository.server

    var searching by mutableStateOf(false)
        private set
    var discoveryNote by mutableStateOf<String?>(null)
        private set
    var tab by mutableStateOf(Tab.Albums)
    val details = mutableStateListOf<Detail>()
    var message by mutableStateOf<String?>(null)

    val output: StateFlow<Output> = PlayerRepository.output

    /** The player the UI shows: the Denon's (from the server) or the phone's own, depending on the output. */
    val player: StateFlow<PlayerUi> =
        combine(PlayerRepository.output, PlayerRepository.ui, PlayerRepository.phoneUi) { out, denon, phone ->
            if (out == Output.Phone) phone else denon
        }.stateIn(viewModelScope, SharingStarted.Eagerly, PlayerUi())

    // ---- "more like this": suggestions from the server (Last.fm when configured there, local tags otherwise) ----
    var suggesting by mutableStateOf(false)
        private set

    /** Keep the music going: when the last track of the queue starts, suggestions are appended by themselves. */
    var autoSimilar by mutableStateOf(prefs.getBoolean("auto_similar", true))
        private set
    fun updateAutoSimilar(on: Boolean) { autoSimilar = on; prefs.edit().putBoolean("auto_similar", on).apply() }
    private var autoDoneFor: Long? = null

    init {
        viewModelScope.launch { player.collect { maybeAutoSuggest(it.snap) } }
    }

    private fun maybeAutoSuggest(s: PlayerSnapshot) {
        val cur = s.current ?: return
        if (!autoSimilar || s.queue.isEmpty() || s.index != s.queue.lastIndex) return
        if (s.status != "playing" && s.status != "loading") return
        if (autoDoneFor == cur.trackId) return
        autoDoneFor = cur.trackId
        suggestSimilar(auto = true)
    }

    /** Append tracks similar to the current one to the queue (Denon or phone). Needs the server. */
    fun suggestSimilar(auto: Boolean = false) {
        val a = api ?: return
        val snap = player.value.snap
        val cur = snap.current ?: return
        if (PlayerRepository.server.value != Server.Online) {
            if (!auto) message = "Hors connexion : les suggestions demandent le serveur."
            return
        }
        if (suggesting) return
        suggesting = true
        val oldSize = snap.queue.size
        val stopped = snap.status == "stopped"
        val toPhone = phoneMode
        viewModelScope.launch {
            try {
                val rec = a.recommendations(cur.trackId, 20, snap.queue.map { it.trackId })
                if (rec.items.isEmpty()) {
                    if (!auto) message = "Aucune suggestion trouvée pour ce titre."
                    return@launch
                }
                if (toPhone) {
                    val items = rec.items.mapNotNull { phoneItem(it, a.base) }
                    phone { p ->
                        val old = p.mediaItemCount
                        p.addMediaItems(items)
                        if (p.playbackState == Player.STATE_ENDED) { p.seekTo(old, 0); p.play() }
                    }
                } else {
                    a.enqueue(rec.items.map { it.id })
                    if (stopped) a.jump(oldSize)
                }
                val how = if (rec.source == "lastfm") "d'après Last.fm" else "d'après les genres de ta bibliothèque"
                message = "${rec.items.size} titres ajoutés à la file ($how)."
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (!auto) message = "Suggestions indisponibles : ${e.message ?: e.javaClass.simpleName}"
            } finally {
                suggesting = false
            }
        }
    }

    fun remember(albums: List<Album>) = PlayerRepository.remember(albums)
    fun hasCover(albumId: Long): Boolean = PlayerRepository.hasCover(albumId)

    fun push(d: Detail) { if (details.lastOrNull() != d) details.add(d) }
    fun pop() { if (details.isNotEmpty()) details.removeAt(details.lastIndex) }

    fun saveAddress(raw: String) {
        var a = raw.trim().trimEnd('/')
        if (a.isEmpty()) return
        if (!a.startsWith("http://") && !a.startsWith("https://")) a = "http://$a"
        prefs.edit().putString("address", a).apply()
        address = a
        api = Api(a).also { PlayerRepository.setApi(it) }
        PlayerRepository.server.value = Server.Unknown
        probeNow()
    }

    /** mDNS discovery. Needs ACCESS_LOCAL_NETWORK. Saves the address when something is found. */
    fun discover() {
        if (searching) return
        searching = true
        discoveryNote = null
        viewModelScope.launch {
            val found = runCatching { discoverServer(getApplication()) }.getOrNull()
            searching = false
            if (found != null) {
                saveAddress(found)
                discoveryNote = "Serveur trouvé : $found"
            } else {
                discoveryNote = "Aucun serveur trouvé. Vérifie que le téléphone est sur le même Wi-Fi, ou saisis l'adresse à la main."
                if (api == null) push(Detail.Settings)
            }
        }
    }

    /** Run a server command; failures are shown in the snackbar. The new state comes back on the SSE stream. */
    fun act(block: suspend Api.() -> Unit) {
        val a = api ?: return
        viewModelScope.launch {
            try { a.block() } catch (e: CancellationException) { throw e } catch (e: Exception) {
                message = "Commande refusée : ${e.message ?: e.javaClass.simpleName}"
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Playback commands. ONE entry point for the UI: they go to the Denon (server API) or to the phone's player.

    private val phoneMode get() = PlayerRepository.output.value == Output.Phone

    private fun phone(block: (ExoPlayer) -> Unit) =
        PhoneControl.use({ message = "Le lecteur du téléphone démarre, réessaie dans un instant." }, block)

    /**
     * Choose where the music comes out (player or Settings). What is playing moves with it: same queue, same track,
     * same position (the Denon side restarts the stream, so expect about a second of silence). Offline, only the
     * phone is possible; the choice is remembered and applies again once the server is back and the phone is idle.
     */
    fun chooseOutput(o: Output) {
        preferred = o
        prefs.edit().putString("output", if (o == Output.Phone) "phone" else "denon").apply()
        val from = PlayerRepository.output.value
        if (from == o) return
        val online = PlayerRepository.server.value == Server.Online
        if (o == Output.Denon && !online) {
            message = "Hors connexion : le Denon n'est pas joignable, la lecture reste sur le téléphone."
            return
        }
        val src = if (from == Output.Phone) PlayerRepository.phoneUi.value else PlayerRepository.ui.value
        val snap = src.snap
        val active = snap.status == "playing" || snap.status == "paused" || snap.status == "loading"
        val elapsed = if (snap.status == "playing") SystemClock.elapsedRealtime() - src.receivedAt else 0L
        val position = (snap.positionMs + elapsed).coerceIn(0, if (snap.durationMs > 0) snap.durationMs else Long.MAX_VALUE)

        // 1) the side we leave stops, so nothing plays twice
        if (from == Output.Phone) PhoneControl.player?.stop() else if (online) act { stop() }
        PlayerRepository.output.value = o

        // 2) the other side carries on from the same place
        if (!active || snap.queue.isEmpty()) return
        val index = (snap.index ?: 0).coerceIn(0, snap.queue.lastIndex)
        if (o == Output.Phone) {
            val base = if (online) api?.base else null
            val items = snap.queue.mapNotNull { q -> phoneItem(Track(q.trackId, q.albumId, q.title, q.artist, null, q.durationMs), base) }
            if (items.isEmpty()) { message = "Ces titres ne sont pas disponibles sur le téléphone."; return }
            val start = items.indexOfFirst { it.mediaId == snap.queue[index].trackId.toString() }.coerceAtLeast(0)
            phone { PhoneControl.play(it, items, start, position) }
        } else {
            act { playTracks(snap.queue.map { it.trackId }, index) }
            if (position > 2000) viewModelScope.launch { delay(1800); act { seekTo(position) } }
        }
    }

    /**
     * Phone when the server is unreachable, otherwise the user's choice. Never takes the music away from what is
     * playing right now: coming home with the phone playing, it stays on the phone until it stops. The one exception
     * is the Denon when the server is gone: it cannot be controlled anyway.
     */
    private fun reconcileOutput() {
        val offline = PlayerRepository.server.value == Server.Offline
        val desired = if (offline) Output.Phone else preferred
        val current = PlayerRepository.output.value
        if (desired == current) return
        val ui = if (current == Output.Phone) PlayerRepository.phoneUi.value else PlayerRepository.ui.value
        val busy = ui.snap.status != "stopped"
        if (busy && !(offline && current == Output.Denon)) return
        PlayerRepository.output.value = desired
    }

    // ---- is the server reachable? (3 states: no "offline" flash at start-up) ----
    private val wake = MutableSharedFlow<Unit>(extraBufferCapacity = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** Probe again now (network change, "Réessayer", new address). */
    fun probeNow() { wake.tryEmit(Unit) }

    private fun startProbing() {
        viewModelScope.launch {
            var failures = 0
            while (isActive) {
                val a = api
                val ok = a != null && a.probe()
                if (ok) {
                    failures = 0
                    PlayerRepository.server.value = Server.Online
                } else {
                    failures++
                    // First answer: believe it at once (opening the app away from home). Later: two failures in a row,
                    // so a Wi-Fi blink at home does not flip the app to offline.
                    if (a == null || PlayerRepository.server.value == Server.Unknown || failures >= 2) PlayerRepository.server.value = Server.Offline
                }
                withTimeoutOrNull(if (ok) 30_000L else 8_000L) { wake.first() }
            }
        }
        // A live stream from the server proves it is there.
        viewModelScope.launch { PlayerRepository.ui.collect { if (it.live) PlayerRepository.server.value = Server.Online } }
    }

    fun playAlbum(detail: AlbumDetail, start: Int) {
        if (phoneMode) { playOnPhone(detail.tracks, start); markPlayed(detail.album.id) } else act { playAlbum(detail.album.id, start) }
    }

    fun playTrack(t: Track) {
        if (phoneMode) { playOnPhone(listOf(t), 0); markPlayed(t.albumId) } else act { playTracks(listOf(t.id)) }
    }

    /** "Récents" is what was listened to: the server records the amplifier's plays, the phone reports its own. Best effort. */
    private fun markPlayed(albumId: Long) {
        val a = api ?: return
        if (PlayerRepository.server.value != Server.Online) return
        viewModelScope.launch { try { a.markPlayed(albumId) } catch (e: CancellationException) { throw e } catch (_: Exception) { } }
    }

    private fun playOnPhone(tracks: List<Track>, start: Int) {
        val startId = tracks.getOrNull(start)?.id
        val base = if (PlayerRepository.server.value == Server.Online) api?.base else null
        val items = tracks.mapNotNull { phoneItem(it, base) }
        if (items.isEmpty()) { message = "Ces titres ne sont pas disponibles sur le téléphone."; return }
        val first = items.indexOfFirst { it.mediaId == startId?.toString() }.coerceAtLeast(0)
        phone { PhoneControl.play(it, items, first) }
    }

    fun pause() { if (phoneMode) phone { it.pause() } else act { pause() } }
    fun resume() { if (phoneMode) phone { PhoneControl.resume(it) } else act { resume() } }
    fun next() { if (phoneMode) phone { it.seekToNextMediaItem() } else act { next() } }
    fun previous() { if (phoneMode) phone { it.seekToPrevious() } else act { previous() } }
    fun stop() { if (phoneMode) phone { it.stop() } else act { stop() } }
    fun jump(index: Int) { if (phoneMode) phone { it.seekTo(index, 0); PhoneControl.resume(it) } else act { jump(index) } }
    fun seekTo(ms: Long) { if (phoneMode) phone { it.seekTo(ms.coerceAtLeast(0)) } else act { seekTo(ms) } }

    fun seekBy(deltaMs: Long) {
        if (phoneMode) phone {
            val end = it.duration.takeIf { d -> d > 0 } ?: Long.MAX_VALUE
            it.seekTo((it.currentPosition + deltaMs).coerceIn(0, end))
        } else act { seekBy(deltaMs) }
    }

    /** `level` is 0-100: the Denon's scale, or the phone's media volume scaled to it. */
    fun setVolume(level: Int) {
        if (phoneMode) phone { it.setDeviceVolume(Math.round(level.coerceIn(0, 100) * it.deviceInfo.maxVolume / 100f), 0) }
        else act { volume(level) }
    }

    fun setMute(on: Boolean) { if (phoneMode) phone { it.setDeviceMuted(on, 0) } else act { mute(on) } }

    // ---- downloads (albums kept on the phone) ----
    fun downloadAlbum(albumId: Long) = DownloadWorker.enqueue(getApplication(), albumId)
    fun cancelDownload(albumId: Long) = DownloadWorker.cancel(getApplication(), albumId)
    fun deleteDownload(albumId: Long) {
        DownloadWorker.cancel(getApplication(), albumId)
        LocalLibrary.delete(albumId)
    }

    // ---- live player state: owned by PlayerRepository, the UI only needs it while visible ----
    fun onForeground(on: Boolean) {
        if (on) PlayerRepository.hold("ui") else PlayerRepository.release("ui")
    }
}
