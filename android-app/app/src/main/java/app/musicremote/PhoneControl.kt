package app.musicremote

import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer

/** Direct handle on the phone's own player (it lives in [PlaybackService], same process). Main thread only. */
object PhoneControl {
    val player: ExoPlayer? get() = PlaybackService.phone

    /** Run `block` on the phone player, or tell the caller there is none yet (service still starting). */
    fun use(onMissing: () -> Unit, block: (ExoPlayer) -> Unit) {
        val p = player
        if (p == null) onMissing() else block(p)
    }

    fun play(p: ExoPlayer, items: List<MediaItem>, start: Int, positionMs: Long = 0) {
        p.setMediaItems(items, start.coerceIn(0, items.lastIndex), positionMs)
        // Files stay on the phone; a stream needs the network awake as well.
        p.setWakeMode(if (items.all { it.localConfiguration?.uri?.scheme == "file" }) androidx.media3.common.C.WAKE_MODE_LOCAL else androidx.media3.common.C.WAKE_MODE_NETWORK)
        p.prepare()
        p.play()
    }

    fun resume(p: ExoPlayer) {
        if (p.playbackState == Player.STATE_IDLE) p.prepare()
        if (p.playbackState == Player.STATE_ENDED) p.seekTo(0, 0)
        p.play()
    }
}
