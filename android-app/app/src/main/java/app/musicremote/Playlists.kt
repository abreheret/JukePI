package app.musicremote

import androidx.compose.ui.res.stringResource
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.DownloadDone
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** The words of an automatic playlist name, in the user's language. */
class PlaylistWords(val unnamed: String, val andOthers: (String) -> String, val locale: Locale)

@Composable
fun rememberPlaylistWords(): PlaylistWords {
    val unnamed = stringResource(R.string.playlist_default)
    val others = stringResource(R.string.and_others, "\u0000")
    val locale = androidx.compose.ui.platform.LocalConfiguration.current.locales[0]
    return remember(unnamed, others, locale) { PlaylistWords(unnamed, { others.replace("\u0000", it) }, locale) }
}

/**
 * Automatic name of a playlist: its main artist ("and others" when several are in it) and the date, e.g.
 * "Michael Jackson and others · Nov 14". A name already taken gets " (2)", " (3)"...
 */
fun autoPlaylistName(tracks: List<Track>, taken: Set<String>, words: PlaylistWords, now: Date = Date()): String {
    val artists = tracks.mapNotNull { it.artist?.trim()?.takeIf { a -> a.isNotEmpty() } }
    val top = artists.groupingBy { it }.eachCount().maxByOrNull { it.value }?.key
    val who = when {
        top == null -> words.unnamed
        artists.distinct().size > 1 -> words.andOthers(top)
        else -> top
    }
    val base = "$who · ${SimpleDateFormat(android.text.format.DateFormat.getBestDateTimePattern(words.locale, "dMMM"), words.locale).format(now)}"
    if (base !in taken) return base
    var n = 2
    while ("$base ($n)" in taken) n++
    return "$base ($n)"
}

/** Rough size of the files: the server does not tell, so about 192 kbit/s (24 kB per second) is assumed. */
fun estimatedBytes(tracks: List<Track>): Long = tracks.sumOf { (it.durationMs ?: 240_000L) / 1000 * 24_000L }

/** "Save this queue on the phone": the whole queue as it is now, with a name the user can change. */
@Composable
fun SavePlaylistDialog(vm: AppViewModel, queue: List<QueueItem>, onDismiss: () -> Unit) {
    val tracks = remember { queue.filter { it.trackId > 0 }.distinctBy { it.trackId }.map { it.toTrack() } }
    val taken = remember { LocalLibrary.playlists.value.map { it.playlist.name }.toSet() }
    val words = rememberPlaylistWords()
    var name by remember { mutableStateOf(autoPlaylistName(tracks, taken, words)) }
    val context = androidx.compose.ui.platform.LocalContext.current
    val free = remember { (context.getExternalFilesDir(null) ?: context.filesDir).usableSpace }
    val size = remember { estimatedBytes(tracks) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.save_queue_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(stringResource(R.string.save_summary, trackCount(tracks.size), fileSize(size), fileSize(free)))
                OutlinedTextField(name, { name = it }, label = { Text(stringResource(R.string.playlist_name)) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                Text(
                    stringResource(R.string.save_hint),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        confirmButton = {
            TextButton(onClick = { vm.savePlaylist(name.trim().ifEmpty { autoPlaylistName(tracks, taken, words) }, tracks); onDismiss() }, enabled = tracks.isNotEmpty()) {
                Text(stringResource(R.string.download))
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** One downloaded playlist: play it, finish or cancel its download, rename or delete it. */
@Composable
fun PlaylistScreen(vm: AppViewModel, id: Long, modifier: Modifier = Modifier) {
    val lists by LocalLibrary.playlists.collectAsState()
    val progress by LocalLibrary.playlistProgress.collectAsState()
    val pl = lists.firstOrNull { it.id == id }
    var confirmDelete by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    if (pl == null) {
        Box(modifier.fillMaxSize().padding(24.dp), Alignment.Center) { Text(stringResource(R.string.playlist_gone), color = MaterialTheme.colorScheme.onSurfaceVariant) }
        return
    }
    val p = progress[id]
    val tracks = pl.playlist.tracks
    LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                tracks.firstOrNull()?.let { Cover(vm.api, it.albumId, false, 600, Modifier.fillMaxWidth(0.6f)) }
                Spacer(Modifier.height(12.dp))
                Text(pl.playlist.name, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                Text(
                    trackCount(tracks.size) + " · " + when {
                        p != null && p.error == null -> if (p.queued) stringResource(R.string.dl_waiting) else stringResource(R.string.dl_progress, p.done, p.total)
                        pl.complete -> fileSize(pl.bytes)
                        else -> stringResource(R.string.pl_incomplete_n, pl.tracksOnPhone, tracks.size)
                    },
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (p != null && p.error == null) {
                    LinearProgressIndicator(progress = { if (p.total > 0) p.done.toFloat() / p.total else 0f }, modifier = Modifier.fillMaxWidth(0.7f).padding(vertical = 6.dp))
                }
                if (p?.error != null) Text(stringResource(R.string.interrupted_why, p.error), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.height(8.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Button(onClick = { vm.playPlaylist(pl.playlist, 0) }, enabled = pl.tracksOnPhone > 0 || vm.api != null) {
                        Icon(Icons.Default.PlayArrow, null)
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(R.string.play_all))
                    }
                    when {
                        p != null && p.error == null -> TextButton(onClick = { vm.cancelPlaylistDownload(id) }) { Text(stringResource(R.string.cancel)) }
                        !pl.complete -> OutlinedButton(onClick = { vm.downloadPlaylist(id) }) {
                            Icon(Icons.Default.Download, null)
                            Spacer(Modifier.width(6.dp))
                            Text(stringResource(R.string.resume))
                        }
                        else -> Icon(Icons.Default.DownloadDone, stringResource(R.string.on_the_phone), tint = MaterialTheme.colorScheme.primary)
                    }
                }
                Row {
                    TextButton(onClick = { renaming = true }) { Text(stringResource(R.string.rename)) }
                    TextButton(onClick = { confirmDelete = true }) { Text(stringResource(R.string.delete)) }
                }
            }
        }
        itemsIndexed(tracks, key = { i, t -> "$i-${t.id}" }) { i, t ->
            val onPhone = remember(t.id, lists) { LocalLibrary.trackFile(t.id) != null }
            ListItem(
                modifier = Modifier.clickable { vm.playPlaylist(pl.playlist, i) }.alpha(if (onPhone || vm.api != null) 1f else 0.4f),
                leadingContent = { Text("${i + 1}", color = MaterialTheme.colorScheme.onSurfaceVariant) },
                headlineContent = { Text(t.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                supportingContent = t.artist?.let { a -> { Text(a, maxLines = 1, overflow = TextOverflow.Ellipsis) } },
                trailingContent = { Text(formatMs(t.durationMs), color = MaterialTheme.colorScheme.onSurfaceVariant) },
            )
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text(stringResource(R.string.delete_title)) },
            text = { Text(stringResource(R.string.delete_playlist_text, pl.playlist.name, fileSize(pl.bytes))) },
            confirmButton = { TextButton(onClick = { vm.deletePlaylist(id); confirmDelete = false; vm.pop() }) { Text(stringResource(R.string.delete)) } },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text(stringResource(R.string.keep)) } },
        )
    }
    if (renaming) {
        var name by remember { mutableStateOf(pl.playlist.name) }
        AlertDialog(
            onDismissRequest = { renaming = false },
            title = { Text(stringResource(R.string.rename_playlist)) },
            text = { OutlinedTextField(name, { name = it }, singleLine = true, modifier = Modifier.fillMaxWidth()) },
            confirmButton = { TextButton(onClick = { vm.renamePlaylist(id, name.trim()); renaming = false }, enabled = name.isNotBlank()) { Text(stringResource(R.string.rename)) } },
            dismissButton = { TextButton(onClick = { renaming = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}
