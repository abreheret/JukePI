package app.musicremote

import android.os.SystemClock
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.VolumeOff
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material.icons.filled.Forward10
import androidx.compose.material.icons.filled.Forward30
import androidx.compose.material.icons.filled.Replay10
import androidx.compose.material.icons.filled.Replay30
import androidx.compose.material.icons.filled.Smartphone
import androidx.compose.material.icons.filled.Speaker
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.Remove
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

/**
 * Playback position, extrapolated locally. The Denon only reports it every ~5 s, so between events we add the time
 * elapsed since the event ARRIVED on this phone (never compare the Pi's clock with the phone's).
 */
@Composable
fun rememberPositionMs(ui: PlayerUi): Long {
    var now by remember { mutableLongStateOf(SystemClock.elapsedRealtime()) }
    LaunchedEffect(ui.snap.status, ui.receivedAt) {
        while (ui.snap.status == "playing") {
            now = SystemClock.elapsedRealtime()
            delay(250)
        }
        now = SystemClock.elapsedRealtime()
    }
    val pos = ui.snap.positionMs + if (ui.snap.status == "playing") (now - ui.receivedAt).coerceAtLeast(0) else 0
    return if (ui.snap.durationMs > 0) pos.coerceIn(0, ui.snap.durationMs) else pos
}

private fun title(s: PlayerSnapshot) = s.current?.title ?: "Aucun titre"

/** Compact bar above the tab bar: cover, title, play/pause, next. Tap to open the full player. */
@Composable
fun MiniPlayer(vm: AppViewModel, modifier: Modifier = Modifier) {
    val ui by vm.player.collectAsState()
    val cur = ui.snap.current ?: return
    val playing = ui.snap.status == "playing"
    Surface(modifier.fillMaxWidth().clickable { vm.push(Detail.NowPlaying) }, tonalElevation = 3.dp) {
        Column {
            val duration = ui.snap.durationMs
            val position = rememberPositionMs(ui)
            if (duration > 0) LinearProgressIndicator(progress = { position.toFloat() / duration }, modifier = Modifier.fillMaxWidth())
            Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Cover(vm.api, cur.albumId, vm.hasCover(cur.albumId), 64, Modifier.size(44.dp))
                Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                    Text(cur.title, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.Medium)
                    Text(
                        when {
                            ui.snap.status == "loading" -> "Chargement…"
                            !ui.live -> "Connexion au serveur…"
                            else -> listOfNotNull(if (vm.output.collectAsState().value == Output.Phone) "Téléphone" else null, cur.artist).joinToString(" · ")
                        },
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                val canSeek = ui.snap.status == "playing" || ui.snap.status == "paused"
                IconButton(onClick = { vm.seekBy(-10_000) }, enabled = canSeek) { Icon(Icons.Default.Replay10, "Reculer de 10 secondes") }
                IconButton(onClick = { if (playing) vm.pause() else vm.resume() }) {
                    Icon(if (playing) Icons.Default.Pause else Icons.Default.PlayArrow, if (playing) "Pause" else "Lecture")
                }
                IconButton(onClick = { vm.seekBy(10_000) }, enabled = canSeek) { Icon(Icons.Default.Forward10, "Avancer de 10 secondes") }
                IconButton(onClick = { vm.next() }) { Icon(Icons.Default.SkipNext, "Suivant") }
            }
        }
    }
}

@Composable
fun NowPlayingScreen(vm: AppViewModel, modifier: Modifier = Modifier) {
    val ui by vm.player.collectAsState()
    val s = ui.snap
    val cur = s.current
    val playing = s.status == "playing"
    val pos = rememberPositionMs(ui)

    LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            Column(Modifier.fillMaxWidth().padding(20.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                if (cur != null) Cover(vm.api, cur.albumId, vm.hasCover(cur.albumId), 600, Modifier.fillMaxWidth(0.85f))
                Spacer(Modifier.height(16.dp))
                Text(title(s), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
                Text(cur?.artist.orEmpty(), style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
                Spacer(Modifier.height(4.dp))
                Text(
                    when {
                        s.status == "loading" -> "Chargement…"
                        s.status == "stopped" -> "Arrêté"
                        s.status == "paused" -> "En pause"
                        else -> " "
                    },
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (!ui.live || !s.heosConnected) {
                    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 6.dp)) {
                        Icon(Icons.Default.Warning, null, tint = MaterialTheme.colorScheme.error, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text(if (!ui.live) "Connexion au serveur perdue, reconnexion…" else "Le Denon est injoignable.", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                    }
                }
                s.error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 4.dp)) }

                // Where the music comes out. Switching carries the queue and position over (the Denon side
                // restarts the stream: about a second of silence). Away from home only the phone is possible.
                val output by vm.output.collectAsState()
                val server by vm.server.collectAsState()
                SingleChoiceSegmentedButtonRow(Modifier.padding(top = 10.dp)) {
                    SegmentedButton(
                        selected = output == Output.Denon, onClick = { vm.chooseOutput(Output.Denon) },
                        shape = SegmentedButtonDefaults.itemShape(0, 2),
                        enabled = server == Server.Online || output == Output.Denon,
                        icon = { SegmentedButtonDefaults.Icon(output == Output.Denon) { Icon(Icons.Default.Speaker, null, Modifier.size(18.dp)) } },
                    ) { Text("Denon") }
                    SegmentedButton(
                        selected = output == Output.Phone, onClick = { vm.chooseOutput(Output.Phone) },
                        shape = SegmentedButtonDefaults.itemShape(1, 2),
                        icon = { SegmentedButtonDefaults.Icon(output == Output.Phone) { Icon(Icons.Default.Smartphone, null, Modifier.size(18.dp)) } },
                    ) { Text("Téléphone") }
                }

                if (cur != null) {
                    FilledTonalButton(onClick = { vm.suggestSimilar() }, enabled = !vm.suggesting && server == Server.Online, modifier = Modifier.padding(top = 8.dp)) {
                        if (vm.suggesting) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Icon(Icons.Default.AutoAwesome, null, Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Même style")
                    }
                }

                // Draggable timeline. The Denon cannot seek in a stream: the server restarts it at the wanted frame
                // (about one second of silence), so we only send the position when the finger is lifted.
                Spacer(Modifier.height(8.dp))
                val canSeek = s.durationMs > 0 && (s.status == "playing" || s.status == "paused")
                var scrub by remember { mutableStateOf<Float?>(null) }
                var committed by remember { mutableStateOf(false) }
                // Keep showing the chosen position until the server confirms it, so the thumb does not jump back.
                LaunchedEffect(ui.receivedAt) { if (committed) { scrub = null; committed = false } }
                val fraction = scrub ?: if (s.durationMs > 0) (pos.toFloat() / s.durationMs).coerceIn(0f, 1f) else 0f
                Slider(
                    value = fraction,
                    onValueChange = { scrub = it; committed = false },
                    onValueChangeFinished = {
                        val f = scrub
                        if (f != null) { committed = true; vm.seekTo((f * s.durationMs).toLong()) }
                    },
                    enabled = canSeek,
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(formatMs(if (scrub != null) (scrub!! * s.durationMs).toLong() else pos), style = MaterialTheme.typography.bodySmall)
                    Text(formatMs(s.durationMs), style = MaterialTheme.typography.bodySmall)
                }

                Spacer(Modifier.height(8.dp))
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    IconButton(onClick = { vm.previous() }, modifier = Modifier.size(56.dp)) { Icon(Icons.Default.SkipPrevious, "Précédent", Modifier.size(34.dp)) }
                    FilledIconButton(onClick = { if (playing) vm.pause() else vm.resume() }, modifier = Modifier.size(72.dp)) {
                        Icon(if (playing) Icons.Default.Pause else Icons.Default.PlayArrow, if (playing) "Pause" else "Lecture", Modifier.size(40.dp))
                    }
                    IconButton(onClick = { vm.next() }, modifier = Modifier.size(56.dp)) { Icon(Icons.Default.SkipNext, "Suivant", Modifier.size(34.dp)) }
                    IconButton(onClick = { vm.stop() }) { Icon(Icons.Default.Stop, "Arrêter") }
                }

                Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = { vm.seekBy(-30_000) }, enabled = canSeek) { Icon(Icons.Default.Replay30, "Reculer de 30 secondes") }
                    IconButton(onClick = { vm.seekBy(-10_000) }, enabled = canSeek) { Icon(Icons.Default.Replay10, "Reculer de 10 secondes") }
                    IconButton(onClick = { vm.seekBy(10_000) }, enabled = canSeek) { Icon(Icons.Default.Forward10, "Avancer de 10 secondes") }
                    IconButton(onClick = { vm.seekBy(30_000) }, enabled = canSeek) { Icon(Icons.Default.Forward30, "Avancer de 30 secondes") }
                }

                VolumeSlider(vm, s.volume, s.mute)
            }
        }
        if (s.queue.size > 1) {
            item { Text("File d'attente", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 20.dp, bottom = 4.dp)) }
            itemsIndexed(s.queue, key = { i, q -> "$i-${q.trackId}" }) { i, q ->
                val current = i == s.index
                ListItem(
                    modifier = Modifier.clickable { vm.jump(i) },
                    leadingContent = { Text("${i + 1}", color = MaterialTheme.colorScheme.onSurfaceVariant) },
                    headlineContent = {
                        Text(q.title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            color = if (current) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
                            fontWeight = if (current) FontWeight.SemiBold else FontWeight.Normal)
                    },
                    supportingContent = q.artist?.let { { Text(it, maxLines = 1, overflow = TextOverflow.Ellipsis) } },
                    trailingContent = { Text(formatMs(q.durationMs), color = MaterialTheme.colorScheme.onSurfaceVariant) },
                )
            }
        }
    }
}

@Composable
private fun VolumeSlider(vm: AppViewModel, volume: Int?, muted: Boolean) {
    // Follow the Denon's volume (remote control, HEOS app) unless the user is dragging.
    var value by remember { mutableFloatStateOf(volume?.toFloat() ?: 0f) }
    var dragging by remember { mutableStateOf(false) }
    LaunchedEffect(volume) { if (!dragging && volume != null) value = volume.toFloat() }
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth().padding(top = 8.dp)) {
        IconButton(onClick = { vm.setMute(!muted) }) {
            Icon(if (muted) Icons.AutoMirrored.Filled.VolumeOff else Icons.AutoMirrored.Filled.VolumeUp, if (muted) "Rétablir le son" else "Couper le son")
        }
        Slider(
            value = value, onValueChange = { dragging = true; value = it },
            onValueChangeFinished = { dragging = false; vm.setVolume(value.toInt()) },
            valueRange = 0f..100f, enabled = volume != null && !muted, modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
        )
        Text(if (muted) "muet" else "${value.toInt()}", Modifier.width(44.dp), textAlign = TextAlign.End)
    }
}

/** Small dialog to set the Denon's volume from any screen. */
@Composable
fun VolumeDialog(vm: AppViewModel, onDismiss: () -> Unit) {
    val ui by vm.player.collectAsState()
    val volume = ui.snap.volume
    var value by remember { mutableFloatStateOf(volume?.toFloat() ?: 0f) }
    var dragging by remember { mutableStateOf(false) }
    LaunchedEffect(volume) { if (!dragging && volume != null) value = volume.toFloat() }
    fun send(v: Int) { value = v.toFloat().coerceIn(0f, 100f); vm.setVolume(value.toInt()) }

    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { TextButton(onClick = onDismiss) { Text("Fermer") } },
        title = { Text(if (vm.output.collectAsState().value == Output.Phone) "Volume du téléphone" else "Volume du Denon") },
        text = {
            Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
                Text(if (volume == null) "—" else if (ui.snap.mute) "Son coupé" else "${value.toInt()}", style = MaterialTheme.typography.displaySmall)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    FilledTonalIconButton(onClick = { send(value.toInt() - 1) }, enabled = volume != null) { Icon(Icons.Default.Remove, "Moins fort") }
                    Slider(
                        value = value, onValueChange = { dragging = true; value = it },
                        onValueChangeFinished = { dragging = false; vm.setVolume(value.toInt()) },
                        valueRange = 0f..100f, enabled = volume != null, modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                    )
                    FilledTonalIconButton(onClick = { send(value.toInt() + 1) }, enabled = volume != null) { Icon(Icons.Default.Add, "Plus fort") }
                }
                Spacer(Modifier.height(8.dp))
                val muted = ui.snap.mute
                FilledTonalButton(onClick = { vm.setMute(!muted) }, enabled = volume != null) {
                    Icon(if (muted) Icons.AutoMirrored.Filled.VolumeUp else Icons.AutoMirrored.Filled.VolumeOff, null)
                    Spacer(Modifier.width(8.dp))
                    Text(if (muted) "Rétablir le son" else "Couper le son")
                }
                if (!ui.live || !ui.snap.heosConnected) {
                    Text(
                        if (!ui.live) "Connexion au serveur…" else "Le Denon est injoignable.",
                        color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        },
    )
}
