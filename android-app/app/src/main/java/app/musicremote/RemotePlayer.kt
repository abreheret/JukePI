package app.musicremote

import android.net.Uri
import android.os.Looper
import android.os.SystemClock
import androidx.media3.common.C
import androidx.media3.common.DeviceInfo
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import androidx.media3.common.util.UnstableApi
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.guava.future
import kotlinx.coroutines.withTimeoutOrNull

/**
 * A Media3 [Player] that plays nothing itself: its state mirrors the music server (what the Denon is playing) and
 * its commands (play/pause/next/previous/stop/jump) are sent to the server. Wrapped in a MediaSession it gives the
 * lock screen controls, the media notification and the Bluetooth/headset buttons for free.
 *
 * The Denon cannot seek inside a streamed URL, so seeking within a track is not offered.
 */
private const val MAX_VOLUME = 100

/** Volume change per press of the phone's volume keys (the Denon's scale is 0-100). */
private const val VOLUME_STEP = 2

/** Skip back / forward step of the notification and lock screen buttons. */
private const val SEEK_STEP_MS = 10_000L

@UnstableApi
class RemotePlayer(private val scope: CoroutineScope) : SimpleBasePlayer(Looper.getMainLooper()) {

    /** Call when the repository has a new snapshot. */
    fun refresh() = invalidateState()

    /**
     * Volume the user just asked for, shown until the Denon confirms it. Without it, several quick presses of the
     * phone's volume keys would all start from the same stale value and the volume would barely move.
     */
    private var volumeTarget: Int? = null
    private var volumeToken = 0

    override fun getState(): State {
        val ui = PlayerRepository.ui.value
        val s = ui.snap
        val api = PlayerRepository.api
        val queue = s.queue
        val index = s.index?.takeIf { it in queue.indices }

        val playlist = queue.mapIndexed { i, q ->
            val artwork = if (api != null && PlayerRepository.hasCover(q.albumId)) Uri.parse(api.coverUrl(q.albumId, 300)) else null
            val meta = MediaMetadata.Builder().setTitle(q.title).setArtist(q.artist).setArtworkUri(artwork).setIsPlayable(true).build()
            val durationMs = if (i == index && s.durationMs > 0) s.durationMs else q.durationMs
            // The same track can be in the queue twice (an album queued again, a suggestion that was already there):
            // the uid must be unique or Media3 throws "Duplicate MediaItemData UID" and the app crashes.
            MediaItemData.Builder("$i-${q.trackId}")
                .setMediaItem(MediaItem.Builder().setMediaId(q.trackId.toString()).setMediaMetadata(meta).build())
                .setDurationUs(if (durationMs != null && durationMs > 0) durationMs * 1000 else C.TIME_UNSET)
                .setIsSeekable(true)
                .build()
        }

        val playbackState = when {
            index == null || s.status == "stopped" -> STATE_IDLE
            s.status == "loading" -> STATE_BUFFERING
            else -> STATE_READY
        }
        val playing = s.status == "playing"
        val position = s.positionMs + if (playing) (SystemClock.elapsedRealtime() - ui.receivedAt).coerceAtLeast(0) else 0
        val clamped = if (s.durationMs > 0) position.coerceIn(0, s.durationMs) else position

        val commands = Player.Commands.Builder().addAll(
            Player.COMMAND_PLAY_PAUSE, Player.COMMAND_STOP,
            Player.COMMAND_SEEK_TO_NEXT, Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
            Player.COMMAND_SEEK_TO_PREVIOUS, Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM,
            Player.COMMAND_SEEK_TO_MEDIA_ITEM,
            Player.COMMAND_GET_CURRENT_MEDIA_ITEM, Player.COMMAND_GET_METADATA, Player.COMMAND_GET_TIMELINE,
            Player.COMMAND_GET_DEVICE_VOLUME, Player.COMMAND_SET_DEVICE_VOLUME_WITH_FLAGS, Player.COMMAND_ADJUST_DEVICE_VOLUME_WITH_FLAGS,
            // Seeking inside a track: the server restarts the stream at the wanted frame (see Api.seekTo).
            Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM, Player.COMMAND_SEEK_BACK, Player.COMMAND_SEEK_FORWARD,
        ).build()

        return State.Builder()
            .setAvailableCommands(commands)
            .setPlaylist(playlist)
            .setCurrentMediaItemIndex(index ?: 0)
            .setPlaybackState(playbackState)
            .setPlayWhenReady(s.status == "playing" || s.status == "loading", PLAY_WHEN_READY_CHANGE_REASON_REMOTE)
            // A REMOTE device: the system sends the phone's volume keys to this session instead of changing the phone's volume.
            .setDeviceInfo(DeviceInfo.Builder(DeviceInfo.PLAYBACK_TYPE_REMOTE).setMinVolume(0).setMaxVolume(MAX_VOLUME).build())
            .setDeviceVolume((volumeTarget ?: s.volume ?: 0).coerceIn(0, MAX_VOLUME))
            .setIsDeviceMuted(s.mute)
            .setSeekBackIncrementMs(SEEK_STEP_MS)
            .setSeekForwardIncrementMs(SEEK_STEP_MS)
            .setContentPositionMs(
                if (playing) PositionSupplier.getExtrapolating(clamped, 1f) else PositionSupplier.getConstant(clamped)
            )
            .build()
    }

    override fun handleSetPlayWhenReady(playWhenReady: Boolean): ListenableFuture<*> =
        command { if (playWhenReady) resume() else pause() }

    override fun handleStop(): ListenableFuture<*> = command { stop() }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> =
        command {
            when (seekCommand) {
                Player.COMMAND_SEEK_TO_NEXT, Player.COMMAND_SEEK_TO_NEXT_MEDIA_ITEM -> next()
                Player.COMMAND_SEEK_TO_PREVIOUS, Player.COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM -> previous()
                Player.COMMAND_SEEK_TO_MEDIA_ITEM -> jump(mediaItemIndex)
                // `positionMs` is already the target (current position -/+ the increment for back/forward).
                Player.COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM, Player.COMMAND_SEEK_BACK, Player.COMMAND_SEEK_FORWARD ->
                    if (positionMs != C.TIME_UNSET) seekTo(positionMs)
                else -> {}
            }
        }

    override fun handleSetDeviceVolume(deviceVolume: Int, flags: Int): ListenableFuture<*> = setVolume(deviceVolume)

    override fun handleIncreaseDeviceVolume(flags: Int): ListenableFuture<*> = setVolume(currentVolume() + VOLUME_STEP)

    override fun handleDecreaseDeviceVolume(flags: Int): ListenableFuture<*> = setVolume(currentVolume() - VOLUME_STEP)

    override fun handleSetDeviceMuted(muted: Boolean, flags: Int): ListenableFuture<*> = command { mute(muted) }

    override fun handleRelease(): ListenableFuture<*> = Futures.immediateVoidFuture()

    private fun currentVolume(): Int = volumeTarget ?: PlayerRepository.ui.value.snap.volume ?: 0

    private fun setVolume(level: Int): ListenableFuture<*> {
        val v = level.coerceIn(0, MAX_VOLUME)
        val token = ++volumeToken
        volumeTarget = v
        return command { volume(v) }.also { f ->
            f.addListener({ if (token == volumeToken) volumeTarget = null }, MoreExecutors.directExecutor())
        }
    }

    /**
     * Send a command, then wait (at most 1.5 s) for the server's next snapshot: the player keeps showing the
     * optimistic state until then, instead of flickering back to the old one when the future completes.
     */
    private fun command(block: suspend Api.() -> Unit): ListenableFuture<*> = scope.future {
        val api = PlayerRepository.api ?: return@future null
        val before = PlayerRepository.ui.value.receivedAt
        try {
            api.block()
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (_: Exception) {
            return@future null
        }
        withTimeoutOrNull(1500) { PlayerRepository.ui.first { it.receivedAt != before } }
        null
    }
}
