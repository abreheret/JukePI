package app.musicremote

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import androidx.core.app.NotificationCompat
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import kotlinx.coroutines.CancellationException
import java.io.IOException
import java.util.concurrent.TimeUnit

/**
 * Downloads one album. Runs as a foreground "data sync" job (with a progress notification) because a big album, or the
 * 200-track audiobook in the library, can outlast the ~10 minutes a plain background job is given. If it is stopped it
 * is retried and resumes at the first missing track.
 */
class DownloadWorker(private val ctx: Context, params: WorkerParameters) : CoroutineWorker(ctx, params) {

    companion object {
        private const val CHANNEL = "downloads"
        private const val NOTIFICATION_ID = 2001
        private const val MAX_ATTEMPTS = 5

        fun tag(albumId: Long) = "dl_$albumId"

        /** Queue an album. Downloads run one after the other (a single chain), never in parallel. */
        fun enqueue(ctx: Context, albumId: Long) {
            LocalLibrary.init(ctx)
            LocalLibrary.markQueued(albumId)
            val request = OneTimeWorkRequestBuilder<DownloadWorker>()
                .setInputData(workDataOf("album_id" to albumId))
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 15, TimeUnit.SECONDS)
                .addTag(tag(albumId))
                .build()
            WorkManager.getInstance(ctx).enqueueUniqueWork("downloads", ExistingWorkPolicy.APPEND_OR_REPLACE, request)
        }

        fun tagPlaylist(id: Long) = "dlp_$id"

        /** Queue a playlist (same single chain as the albums). */
        fun enqueuePlaylist(ctx: Context, playlistId: Long) {
            LocalLibrary.init(ctx)
            LocalLibrary.markPlaylistQueued(playlistId, LocalLibrary.playlist(playlistId)?.playlist?.tracks?.size ?: 0)
            val request = OneTimeWorkRequestBuilder<DownloadWorker>()
                .setInputData(workDataOf("playlist_id" to playlistId))
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 15, TimeUnit.SECONDS)
                .addTag(tagPlaylist(playlistId))
                .build()
            WorkManager.getInstance(ctx).enqueueUniqueWork("downloads", ExistingWorkPolicy.APPEND_OR_REPLACE, request)
        }

        fun cancelPlaylist(ctx: Context, playlistId: Long) {
            WorkManager.getInstance(ctx).cancelAllWorkByTag(tagPlaylist(playlistId))
            LocalLibrary.clearPlaylistProgress(playlistId)
        }

        fun cancel(ctx: Context, albumId: Long) {
            WorkManager.getInstance(ctx).cancelAllWorkByTag(tag(albumId))
            LocalLibrary.clearProgress(albumId)
        }
    }

    private fun notification(text: String) = NotificationCompat.Builder(ctx, CHANNEL)
        .setSmallIcon(R.drawable.ic_stat_music)
        .setContentTitle(ctx.getString(R.string.dl_notification_title))
        .setContentText(text)
        .setOngoing(true)
        .setOnlyAlertOnce(true)
        .build()

    private fun ensureChannel() {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, ctx.getString(R.string.dl_channel_name), NotificationManager.IMPORTANCE_LOW))
        }
    }

    override suspend fun getForegroundInfo(): ForegroundInfo {
        ensureChannel()
        return ForegroundInfo(NOTIFICATION_ID, notification(ctx.getString(R.string.dl_preparing)), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    }

    override suspend fun doWork(): Result {
        val albumId = inputData.getLong("album_id", -1)
        val playlistId = inputData.getLong("playlist_id", -1)
        if (albumId < 0 && playlistId < 0) return Result.failure()
        fun failed(message: String) = if (playlistId >= 0) LocalLibrary.markPlaylistFailed(playlistId, message) else LocalLibrary.markFailed(albumId, message)
        LocalLibrary.init(ctx)
        val base = ctx.getSharedPreferences("server", Context.MODE_PRIVATE).getString("address", null) ?: return Result.failure()
        runCatching { setForeground(getForegroundInfo()) }
        val api = Api(base)
        return try {
            if (playlistId >= 0) LocalLibrary.downloadPlaylist(api, playlistId) else LocalLibrary.download(api, albumId)
            Result.success()
        } catch (e: CancellationException) {
            throw e
        } catch (e: IOException) {
            // Network trouble (left home, server restarting) or no room left: retry later, resuming at the first missing track.
            val noSpace = e is NotEnoughSpace
            failed(if (noSpace) ctx.getString(R.string.err_no_space) else e.message ?: ctx.getString(R.string.dl_interrupted_default))
            if (runAttemptCount < MAX_ATTEMPTS && !noSpace) Result.retry() else Result.failure()
        } catch (e: Exception) {
            failed(e.message ?: e.javaClass.simpleName)
            Result.failure()
        }
    }
}
