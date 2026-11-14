package app.musicremote

import android.app.PendingIntent
import android.content.Intent
import android.os.SystemClock
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.extractor.DefaultExtractorsFactory
import androidx.media3.session.CommandButton
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import com.google.common.collect.ImmutableList
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/**
 * Publishes what is playing as a MediaSession (lock screen, notification, Bluetooth). The session drives one of two
 * players, switched with [PlayerRepository.output]:
 *  - [RemotePlayer]: nothing plays on the phone, the state and the buttons mirror the Denon through the server;
 *  - an [ExoPlayer]: the music plays on the phone (downloaded files, or streamed from the server).
 */
@UnstableApi
class PlaybackService : MediaSessionService() {
    companion object {
        /** The phone's own player while the service is alive (see [PhoneControl]). */
        @Volatile var phone: ExoPlayer? = null
    }

    private var session: MediaSession? = null
    private var remote: RemotePlayer? = null
    private var local: ExoPlayer? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var holding = false

    override fun onCreate() {
        super.onCreate()
        PlayerRepository.init(applicationContext)
        LocalLibrary.init(applicationContext)

        val r = RemotePlayer(scope)
        remote = r
        // Many MP3s of the library have no Xing/VBRI header (no duration, no seek table): ExoPlayer then refuses to
        // seek unless told to assume a constant bitrate. Slightly approximate for VBR files, but it works.
        val extractors = DefaultExtractorsFactory().setConstantBitrateSeekingEnabled(true)
        val l = ExoPlayer.Builder(this, DefaultMediaSourceFactory(this, extractors))
            .setAudioAttributes(
                AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(),
                /* handleAudioFocus = */ true,
            )
            .setHandleAudioBecomingNoisy(true) // headphones unplugged -> pause
            .setSeekBackIncrementMs(10_000) // default is 5 s: the +-10 s buttons must not lie
            .setSeekForwardIncrementMs(10_000) // default is 15 s
            .build()
        local = l
        phone = l
        l.addListener(object : Player.Listener {
            override fun onEvents(player: Player, events: Player.Events) = publishPhone()
        })

        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val back = CommandButton.Builder(CommandButton.ICON_SKIP_BACK_10)
            .setPlayerCommand(Player.COMMAND_SEEK_BACK).setDisplayName(getString(R.string.skip_back_10))
            .setSlots(CommandButton.SLOT_OVERFLOW).build()
        val forward = CommandButton.Builder(CommandButton.ICON_SKIP_FORWARD_10)
            .setPlayerCommand(Player.COMMAND_SEEK_FORWARD).setDisplayName(getString(R.string.skip_forward_10))
            .setSlots(CommandButton.SLOT_OVERFLOW).build()
        session = MediaSession.Builder(this, if (PlayerRepository.output.value == Output.Phone) l else r)
            .setSessionActivity(open)
            .setMediaButtonPreferences(ImmutableList.of(back, forward)) // 10 s skips in the extra slots, so previous/next keep theirs
            .build()
        setMediaNotificationProvider(
            DefaultMediaNotificationProvider.Builder(this).setChannelName(R.string.notification_channel_name).build()
                .also { it.setSmallIcon(R.drawable.ic_stat_music) }
        )

        // Swap the player behind the session when the output changes.
        scope.launch {
            PlayerRepository.output.collect { out ->
                val target: Player = if (out == Output.Phone) l else r
                if (session?.player !== target) {
                    if (target === r) l.pause() // back on the Denon: the phone must not keep playing
                    session?.player = target
                }
                publishPhone()
            }
        }

        scope.launch {
            combine(PlayerRepository.output, PlayerRepository.ui) { out, ui -> out to ui }.collect { (out, ui) ->
                // Keep the live stream open only while the Denon has something to control: it costs battery in the background.
                val active = out == Output.Denon && ui.snap.status != "stopped"
                if (active && !holding) { PlayerRepository.hold("service"); holding = true }
                if (!active && holding) { PlayerRepository.release("service"); holding = false }
                r.refresh()
            }
        }
    }

    /** Mirror the phone player into the same snapshot shape the Denon uses, so the UI does not care which one plays. */
    private fun publishPhone() {
        val p = local ?: return
        val n = p.mediaItemCount
        val state = p.playbackState
        val status = when {
            n == 0 || state == Player.STATE_IDLE || state == Player.STATE_ENDED -> "stopped"
            state == Player.STATE_BUFFERING && p.playWhenReady -> "loading"
            p.isPlaying -> "playing"
            else -> "paused"
        }
        val queue = (0 until n).map { p.getMediaItemAt(it).toQueueItem() }
        val index = if (n > 0) p.currentMediaItemIndex else null
        val current = index?.let { queue.getOrNull(it) }
        val duration = p.duration.takeIf { it != C.TIME_UNSET && it > 0 } ?: current?.durationMs ?: 0L
        val maxVolume = p.deviceInfo.maxVolume.coerceAtLeast(1)
        PlayerRepository.phoneUi.value = PlayerUi(
            PlayerSnapshot(
                status = status,
                heosConnected = true,
                index = index,
                current = current,
                positionMs = p.currentPosition.coerceAtLeast(0),
                durationMs = duration,
                volume = Math.round(p.deviceVolume * 100f / maxVolume),
                mute = p.isDeviceMuted,
                queue = queue,
            ),
            SystemClock.elapsedRealtime(),
            live = true,
        )
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onDestroy() {
        scope.cancel()
        if (holding) PlayerRepository.release("service")
        phone = null
        session?.release()
        remote?.release()
        local?.release()
        session = null
        remote = null
        local = null
        super.onDestroy()
    }
}
