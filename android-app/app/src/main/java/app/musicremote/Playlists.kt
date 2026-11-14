package app.musicremote

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

/**
 * Automatic name of a playlist: its main artist ("et autres" when several are in it) and the date, e.g.
 * "Michael Jackson et autres · 14 nov.". A name already taken gets " (2)", " (3)"...
 */
fun autoPlaylistName(tracks: List<Track>, taken: Set<String>, now: Date = Date()): String {
    val artists = tracks.mapNotNull { it.artist?.trim()?.takeIf { a -> a.isNotEmpty() } }
    val top = artists.groupingBy { it }.eachCount().maxByOrNull { it.value }?.key
    val who = when {
        top == null -> "Playlist"
        artists.distinct().size > 1 -> "$top et autres"
        else -> top
    }
    val base = "$who · ${SimpleDateFormat("d MMM", Locale.FRENCH).format(now)}"
    if (base !in taken) return base
    var n = 2
    while ("$base ($n)" in taken) n++
    return "$base ($n)"
}

/** Rough size of the files: the server does not tell, so about 192 kbit/s (24 kB per second) is assumed. */
fun estimatedBytes(tracks: List<Track>): Long = tracks.sumOf { (it.durationMs ?: 240_000L) / 1000 * 24_000L }

private fun megabytes(bytes: Long) = "%.0f Mo".format(bytes / 1_048_576.0)

/** "Save this queue on the phone": the whole queue as it is now, with a name the user can change. */
@Composable
fun SavePlaylistDialog(vm: AppViewModel, queue: List<QueueItem>, onDismiss: () -> Unit) {
    val tracks = remember { queue.filter { it.trackId > 0 }.distinctBy { it.trackId }.map { it.toTrack() } }
    val taken = remember { LocalLibrary.playlists.value.map { it.playlist.name }.toSet() }
    var name by remember { mutableStateOf(autoPlaylistName(tracks, taken)) }
    val context = androidx.compose.ui.platform.LocalContext.current
    val free = remember { (context.getExternalFilesDir(null) ?: context.filesDir).usableSpace }
    val size = remember { estimatedBytes(tracks) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Télécharger la file ?") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("${tracks.size} titres, environ ${megabytes(size)}. ${"%.1f".format(free / 1_073_741_824.0)} Go libres sur le téléphone.")
                OutlinedTextField(name, { name = it }, label = { Text("Nom de la playlist") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                Text(
                    "La file telle qu'elle est maintenant sera gardée sur le téléphone, jouable sans le serveur.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
        confirmButton = {
            TextButton(onClick = { vm.savePlaylist(name.trim().ifEmpty { autoPlaylistName(tracks, taken) }, tracks); onDismiss() }, enabled = tracks.isNotEmpty()) {
                Text("Télécharger")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Annuler") } },
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
        Box(modifier.fillMaxSize().padding(24.dp), Alignment.Center) { Text("Cette playlist n'est plus sur le téléphone.", color = MaterialTheme.colorScheme.onSurfaceVariant) }
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
                    "${tracks.size} titres · " + when {
                        p != null && p.error == null -> if (p.queued) "en attente…" else "téléchargement ${p.done}/${p.total}"
                        pl.complete -> megabytes(pl.bytes)
                        else -> "incomplète ${pl.tracksOnPhone}/${tracks.size}"
                    },
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (p != null && p.error == null) {
                    LinearProgressIndicator(progress = { if (p.total > 0) p.done.toFloat() / p.total else 0f }, modifier = Modifier.fillMaxWidth(0.7f).padding(vertical = 6.dp))
                }
                if (p?.error != null) Text("Interrompu : ${p.error}", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.height(8.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Button(onClick = { vm.playPlaylist(pl.playlist, 0) }, enabled = pl.tracksOnPhone > 0 || vm.api != null) {
                        Icon(Icons.Default.PlayArrow, null)
                        Spacer(Modifier.width(6.dp))
                        Text("Tout lire")
                    }
                    when {
                        p != null && p.error == null -> TextButton(onClick = { vm.cancelPlaylistDownload(id) }) { Text("Annuler") }
                        !pl.complete -> OutlinedButton(onClick = { vm.downloadPlaylist(id) }) {
                            Icon(Icons.Default.Download, null)
                            Spacer(Modifier.width(6.dp))
                            Text("Reprendre")
                        }
                        else -> Icon(Icons.Default.DownloadDone, "Sur le téléphone", tint = MaterialTheme.colorScheme.primary)
                    }
                }
                Row {
                    TextButton(onClick = { renaming = true }) { Text("Renommer") }
                    TextButton(onClick = { confirmDelete = true }) { Text("Supprimer") }
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
            title = { Text("Supprimer du téléphone ?") },
            text = { Text("« ${pl.playlist.name} » (${megabytes(pl.bytes)}) ne sera plus disponible hors connexion. Les titres restent sur le serveur.") },
            confirmButton = { TextButton(onClick = { vm.deletePlaylist(id); confirmDelete = false; vm.pop() }) { Text("Supprimer") } },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Garder") } },
        )
    }
    if (renaming) {
        var name by remember { mutableStateOf(pl.playlist.name) }
        AlertDialog(
            onDismissRequest = { renaming = false },
            title = { Text("Renommer la playlist") },
            text = { OutlinedTextField(name, { name = it }, singleLine = true, modifier = Modifier.fillMaxWidth()) },
            confirmButton = { TextButton(onClick = { vm.renamePlaylist(id, name.trim()); renaming = false }, enabled = name.isNotBlank()) { Text("Renommer") } },
            dismissButton = { TextButton(onClick = { renaming = false }) { Text("Annuler") } },
        )
    }
}
