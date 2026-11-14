package app.musicremote

import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.DownloadDone
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Album
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

private class Page(val items: List<Album> = emptyList(), val total: Int = 0, val loading: Boolean = true, val error: String? = null)

@Composable
private fun ErrorBox(text: String, onRetry: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(text, style = MaterialTheme.typography.bodyMedium)
        Button(onClick = onRetry) { Text(stringResource(R.string.retry)) }
    }
}

// ---------------------------------------------------------------------------------------------
// Albums grid (home tab, and the albums of one artist)

// "listened": what was played first (the latest first), then the albums with a cover, the newest first.
private val SORTS = listOf("listened" to R.string.sort_recent, "name" to R.string.sort_az, "artist" to R.string.sort_artist, "random" to R.string.sort_random)

/** Is an album on the phone? Done = every track, Partial = some (an interrupted download). */
private enum class Stored { None, Partial, Done }

@Composable
private fun storedStates(): Map<Long, Stored> {
    val library by LocalLibrary.albums.collectAsState()
    return remember(library) { library.associate { it.id to if (it.complete) Stored.Done else Stored.Partial } }
}

@Composable
fun AlbumsScreen(vm: AppViewModel, artist: String?, modifier: Modifier = Modifier) {
    val server by vm.server.collectAsState()
    var sort by rememberSaveable(artist) { mutableStateOf(if (artist == null) "listened" else "name") }
    var seed by rememberSaveable { mutableIntStateOf(0) }
    val api = vm.api
    Column(modifier) {
        if (artist == null && server == Server.Online) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                SORTS.forEach { (key, label) ->
                    FilterChip(selected = sort == key, onClick = { sort = key }, label = { Text(stringResource(label)) })
                }
                if (sort == "random") IconButton(onClick = { seed++ }) { Icon(Icons.Default.Refresh, stringResource(R.string.shuffle)) }
            }
        }
        when {
            server == Server.Unknown -> Box(Modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator() }
            // Away from the server nothing is asked of the network: the downloaded albums are all there is.
            server == Server.Offline -> LocalAlbumsGrid(vm, artist)
            api == null -> ErrorBox(stringResource(R.string.no_server)) { vm.push(Detail.Settings) }
            else -> RemoteAlbums(vm, api, sort, seed, artist)
        }
    }
}

@Composable
private fun RemoteAlbums(vm: AppViewModel, api: Api, sort: String, seed: Int, artist: String?) {
    var page by remember(api, sort, artist, seed) { mutableStateOf(Page()) }
    var retry by remember { mutableIntStateOf(0) }
    val grid = rememberLazyGridState()
    val stored = storedStates()
    val ctx = androidx.compose.ui.platform.LocalContext.current

    suspend fun load(offset: Int) {
        try {
            val r = api.albums(60, offset, sort, artist)
            vm.remember(r.items)
            page = Page(if (offset == 0) r.items else page.items + r.items, r.total, false)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            page = Page(page.items, page.total, false, ctx.getString(R.string.err_server_unreachable, e.message ?: e.javaClass.simpleName))
        }
    }

    LaunchedEffect(api, sort, artist, seed, retry) { page = Page(); load(0) }
    // Infinite scroll: ask for the next 60 when the last visible item gets close to the end of what we have.
    LaunchedEffect(grid, page.items.size, page.total) {
        androidx.compose.runtime.snapshotFlow { grid.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0 }.collectLatest { last ->
            if (sort != "random" && !page.loading && page.error == null && page.items.size < page.total && last >= page.items.size - 12) {
                page = Page(page.items, page.total, true)
                load(page.items.size)
            }
        }
    }

    when {
        page.error != null && page.items.isEmpty() -> ErrorBox(page.error!!) { retry++ }
        page.loading && page.items.isEmpty() -> Box(Modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator() }
        else -> LazyVerticalGrid(
            columns = GridCells.Adaptive(150.dp),
            state = grid,
            contentPadding = PaddingValues(12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            items(page.items, key = { it.id }) { a ->
                AlbumCell(api, a, stored[a.id] ?: Stored.None) { vm.push(Detail.AlbumPage(a.id, a.name)) }
            }
        }
    }
}

/** The albums on the phone, usable with no network at all (also the albums of one artist). */
@Composable
fun LocalAlbumsGrid(vm: AppViewModel, artist: String?, modifier: Modifier = Modifier) {
    val library by LocalLibrary.albums.collectAsState()
    val stored = storedStates()
    val albums = remember(library, artist) { LocalCatalog.albums(artist) }
    if (albums.isEmpty()) {
        Box(modifier.fillMaxSize().padding(24.dp), Alignment.Center) {
            Text(stringResource(R.string.no_downloaded_albums), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        return
    }
    LazyVerticalGrid(
        columns = GridCells.Adaptive(150.dp),
        contentPadding = PaddingValues(12.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
        modifier = modifier.fillMaxSize(),
    ) {
        items(albums, key = { it.id }) { a -> AlbumCell(vm.api, a, stored[a.id] ?: Stored.None) { vm.push(Detail.AlbumPage(a.id, a.name)) } }
    }
}

/**
 * The "Downloaded" tab: everything kept on the phone, with the state of each album (in progress, incomplete, or
 * complete and its size). Works with or without the server.
 */
@Composable
fun DownloadedScreen(vm: AppViewModel, modifier: Modifier = Modifier) {
    val library by LocalLibrary.albums.collectAsState()
    val progress by LocalLibrary.progress.collectAsState()
    val playlists by LocalLibrary.playlists.collectAsState()
    val playlistProgress by LocalLibrary.playlistProgress.collectAsState()
    val context = androidx.compose.ui.platform.LocalContext.current
    val free = remember(library, playlists) { (context.getExternalFilesDir(null) ?: context.filesDir).usableSpace }
    // In progress first, then incomplete, then complete; each group by name.
    val sorted = remember(library, progress) {
        library.sortedWith(compareBy({ progress[it.id] == null }, { it.complete }, { it.detail.album.name.lowercase() }))
    }
    Column(modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                storageSummary(library.size, playlists.size, free, false),
                style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f),
            )
            TextButton(onClick = { vm.push(Detail.Downloads) }) { Text(stringResource(R.string.manage)) }
        }
        if (sorted.isEmpty() && playlists.isEmpty()) {
            Box(Modifier.fillMaxSize().padding(24.dp), Alignment.Center) {
                Text(
                    stringResource(R.string.nothing_on_phone),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(150.dp),
                contentPadding = PaddingValues(12.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                if (playlists.isNotEmpty()) {
                    item(span = { GridItemSpan(maxLineSpan) }) {
                        Text(stringResource(R.string.playlists_header), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
                    }
                    items(playlists, key = { "playlist-${it.id}" }, span = { GridItemSpan(maxLineSpan) }) { d ->
                        val p = playlistProgress[d.id]
                        ListItem(
                            modifier = Modifier.clickable { vm.push(Detail.PlaylistPage(d.id, d.playlist.name)) },
                            leadingContent = { Cover(vm.api, d.playlist.tracks.firstOrNull()?.albumId ?: 0L, false, 64, Modifier.size(52.dp)) },
                            headlineContent = { Text(d.playlist.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            supportingContent = {
                                Text(
                                    trackCount(d.playlist.tracks.size) + " · " + when {
                                        p != null && p.error == null -> if (p.queued) stringResource(R.string.dl_waiting) else stringResource(R.string.dl_progress, p.done, p.total)
                                        p?.error != null -> stringResource(R.string.pl_interrupted_n, d.tracksOnPhone, d.playlist.tracks.size)
                                        d.complete -> fileSize(d.bytes)
                                        else -> stringResource(R.string.pl_incomplete_n, d.tracksOnPhone, d.playlist.tracks.size)
                                    },
                                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                                )
                            },
                        )
                    }
                    if (sorted.isNotEmpty()) item(span = { GridItemSpan(maxLineSpan) }) {
                        Text(stringResource(R.string.tab_albums), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(top = 8.dp))
                    }
                }
                items(sorted, key = { it.id }) { d ->
                    val p = progress[d.id]
                    val status = when {
                        p != null && p.error == null -> if (p.queued) stringResource(R.string.dl_waiting) else stringResource(R.string.dl_progress, p.done, p.total)
                        p?.error != null -> stringResource(R.string.dl_interrupted_n, d.tracksOnPhone, d.detail.tracks.size)
                        d.complete -> stringResource(R.string.dl_complete_size, fileSize(d.bytes))
                        else -> stringResource(R.string.dl_incomplete_n, d.tracksOnPhone, d.detail.tracks.size)
                    }
                    AlbumCell(vm.api, d.detail.album, if (d.complete) Stored.Done else Stored.Partial, status) {
                        vm.push(Detail.AlbumPage(d.id, d.detail.album.name))
                    }
                }
            }
        }
    }
}

@Composable
private fun AlbumCell(api: Api?, a: Album, stored: Stored, status: String? = null, onClick: () -> Unit) {
    Column(Modifier.clickable(onClick = onClick)) {
        Box {
            Cover(api, a.id, a.hasCover, 300, Modifier.fillMaxWidth())
            if (stored != Stored.None) {
                val done = stored == Stored.Done
                Icon(
                    if (done) Icons.Default.DownloadDone else Icons.Default.Download,
                    stringResource(if (done) R.string.downloaded else R.string.download_incomplete),
                    tint = if (done) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onTertiary,
                    modifier = Modifier.align(Alignment.TopEnd).padding(6.dp).size(24.dp)
                        .background(if (done) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.tertiary, androidx.compose.foundation.shape.CircleShape)
                        .padding(3.dp),
                )
            }
        }
        Spacer(Modifier.height(6.dp))
        Text(a.name, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(a.artistText(), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (status != null) Text(status, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary, maxLines = 1)
    }
}

// ---------------------------------------------------------------------------------------------
// One album

@Composable
fun AlbumDetailScreen(vm: AppViewModel, id: Long, modifier: Modifier = Modifier) {
    val api = vm.api
    var detail by remember(id) { mutableStateOf<AlbumDetail?>(null) }
    var error by remember(id) { mutableStateOf<String?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    val ui by vm.player.collectAsState()
    val server by vm.server.collectAsState()
    val library by LocalLibrary.albums.collectAsState() // recompose when a track lands on the phone
    val ctx = androidx.compose.ui.platform.LocalContext.current

    LaunchedEffect(api, id, retry, server) {
        error = null
        when (server) {
            Server.Unknown -> {}
            // Away from the server: the copy saved with the download is all there is.
            Server.Offline -> {
                detail = LocalLibrary.detail(id)
                if (detail == null) error = ctx.getString(R.string.album_not_on_phone)
            }
            Server.Online -> try {
                detail = api!!.album(id).also { vm.remember(listOf(it.album)) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                detail = LocalLibrary.detail(id)
                if (detail == null) error = ctx.getString(R.string.err_album_load, e.message ?: e.javaClass.simpleName)
            }
        }
    }
    val d = detail
    when {
        error != null -> ErrorBox(error!!) { retry++ }
        d == null -> Box(modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator() }
        else -> LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
            item {
                Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Cover(api, d.album.id, d.album.hasCover, 600, Modifier.fillMaxWidth(0.7f))
                    Spacer(Modifier.height(12.dp))
                    Text(d.album.name, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                    Text(
                        listOfNotNull(d.album.artistText().ifEmpty { null }, d.album.year?.toString(), trackCount(d.tracks.size)).joinToString(" · "),
                        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(12.dp))
                    Button(onClick = { vm.playAlbum(d, 0) }) {
                        Icon(Icons.Default.PlayArrow, null)
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(R.string.play_all))
                    }
                    DownloadControl(vm, d)
                    // All the albums of this artist (not for a compilation, whose "artist" is only the most frequent one).
                    val artistName = d.album.artist?.takeIf { it.isNotBlank() && !d.album.variousArtists }
                    if (artistName != null) {
                        TextButton(onClick = { vm.push(Detail.ArtistPage(artistName)) }) {
                            Icon(Icons.Default.Album, null, Modifier.size(18.dp))
                            Spacer(Modifier.width(6.dp))
                            Text(stringResource(R.string.all_albums_of, artistName), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
            itemsIndexed(d.tracks, key = { _, t -> t.id }) { index, t ->
                val playing = ui.snap.current?.trackId == t.id && ui.snap.status != "stopped"
                val onPhone = remember(library, t.id) { LocalLibrary.trackFile(t.id) != null }
                // Offline, a track that was not downloaded cannot be played: shown dimmed.
                val dimmed = server == Server.Offline && !onPhone
                ListItem(
                    modifier = Modifier.clickable(enabled = !dimmed) { vm.playAlbum(d, index) }.then(if (dimmed) Modifier.alpha(0.4f) else Modifier),
                    leadingContent = { Text((t.trackNo ?: (index + 1)).toString(), color = MaterialTheme.colorScheme.onSurfaceVariant) },
                    headlineContent = {
                        Text(t.title, maxLines = 2, overflow = TextOverflow.Ellipsis,
                            color = if (playing) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
                            fontWeight = if (playing) FontWeight.SemiBold else FontWeight.Normal)
                    },
                    supportingContent = if (d.album.variousArtists && t.artist != null) { { Text(t.artist) } } else null,
                    trailingContent = {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (onPhone) Icon(Icons.Default.DownloadDone, stringResource(R.string.on_the_phone), tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size(18.dp))
                            Text(formatMs(t.durationMs), color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    },
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Artists

@Composable
fun ArtistsScreen(vm: AppViewModel, modifier: Modifier = Modifier) {
    val api = vm.api
    val server by vm.server.collectAsState()
    val library by LocalLibrary.albums.collectAsState()
    val ctx = androidx.compose.ui.platform.LocalContext.current
    if (api == null && server != Server.Offline) { ErrorBox(stringResource(R.string.no_server)) { vm.push(Detail.Settings) }; return }
    var artists by remember(api, server) { mutableStateOf<List<ArtistItem>?>(null) }
    var error by remember(api, server) { mutableStateOf<String?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    LaunchedEffect(api, retry, server, library) {
        error = null
        when (server) {
            Server.Unknown -> {}
            Server.Offline -> artists = LocalCatalog.artists() // only the artists of the downloaded albums
            Server.Online -> try { artists = api!!.artists().items } catch (e: CancellationException) { throw e } catch (e: Exception) { error = ctx.getString(R.string.err_server_unreachable, e.message ?: e.javaClass.simpleName) }
        }
    }
    val list = artists
    when {
        error != null -> ErrorBox(error!!) { retry++ }
        list == null -> Box(modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator() }
        else -> LazyColumn(modifier.fillMaxSize()) {
            var last = ' '
            list.forEach { a ->
                val letter = a.name.firstOrNull()?.uppercaseChar()?.let { if (it.isLetter()) it else '#' } ?: '#'
                if (letter != last) {
                    last = letter
                    item(key = "h$letter") {
                        Text(letter.toString(), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 2.dp))
                    }
                }
                item(key = "a${a.name}") {
                    ListItem(
                        modifier = Modifier.clickable { vm.push(Detail.ArtistPage(a.name)) },
                        headlineContent = { Text(a.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        trailingContent = { Text("${a.albumCount}", color = MaterialTheme.colorScheme.onSurfaceVariant) },
                    )
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Search (type-ahead: the server answers in ~15 ms)

@Composable
fun SearchScreen(vm: AppViewModel, modifier: Modifier = Modifier) {
    val api = vm.api
    val server by vm.server.collectAsState()
    if (api == null && server != Server.Offline) { ErrorBox(stringResource(R.string.no_server)) { vm.push(Detail.Settings) }; return }
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var query by rememberSaveable { mutableStateOf("") }
    var result by remember { mutableStateOf<SearchResult?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(api, query, server) {
        if (query.isBlank()) { result = null; error = null; return@LaunchedEffect }
        if (server == Server.Offline) { result = LocalCatalog.search(query); error = null; return@LaunchedEffect } // downloaded music only
        if (server == Server.Unknown || api == null) return@LaunchedEffect
        delay(150) // debounce: restarted at every keystroke
        try { result = api.search(query.trim()).also { vm.remember(it.albums) }; error = null }
        catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = ctx.getString(R.string.err_search, e.message ?: e.javaClass.simpleName) }
    }

    Column(modifier.fillMaxSize()) {
        OutlinedTextField(
            value = query, onValueChange = { query = it }, singleLine = true,
            placeholder = { Text(stringResource(if (server == Server.Offline) R.string.search_in_downloaded else R.string.search_hint)) },
            leadingIcon = { Icon(Icons.Default.Search, null) },
            modifier = Modifier.fillMaxWidth().padding(12.dp),
        )
        val r = result
        when {
            error != null -> Text(error!!, Modifier.padding(16.dp))
            r == null -> Text(stringResource(R.string.search_tip), Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            r.albums.isEmpty() && r.artists.isEmpty() && r.tracks.isEmpty() -> Text(stringResource(R.string.no_results), Modifier.padding(16.dp))
            else -> LazyColumn(Modifier.fillMaxSize()) {
                if (r.fuzzy) item { Text(stringResource(R.string.fuzzy_results), Modifier.padding(horizontal = 16.dp, vertical = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                if (r.artists.isNotEmpty()) {
                    item { SectionTitle(stringResource(R.string.tab_artists)) }
                    items(r.artists, key = { "ar${it.name}" }) { a ->
                        ListItem(modifier = Modifier.clickable { vm.push(Detail.ArtistPage(a.name)) },
                            headlineContent = { Text(a.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            trailingContent = { Text(pluralStringResource(R.plurals.n_albums, a.albumCount, a.albumCount), color = MaterialTheme.colorScheme.onSurfaceVariant) })
                    }
                }
                if (r.albums.isNotEmpty()) {
                    item { SectionTitle(stringResource(R.string.tab_albums)) }
                    items(r.albums, key = { "al${it.id}" }) { a ->
                        ListItem(modifier = Modifier.clickable { vm.push(Detail.AlbumPage(a.id, a.name)) },
                            leadingContent = { Cover(api, a.id, a.hasCover, 64, Modifier.size(52.dp)) },
                            headlineContent = { Text(a.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            supportingContent = { Text(a.artistText(), maxLines = 1, overflow = TextOverflow.Ellipsis) })
                    }
                }
                if (r.tracks.isNotEmpty()) {
                    item { SectionTitle(stringResource(R.string.section_tracks)) }
                    items(r.tracks, key = { "tr${it.id}" }) { t ->
                        ListItem(modifier = Modifier.clickable { vm.playTrack(t) },
                            leadingContent = { Cover(api, t.albumId, vm.hasCover(t.albumId), 64, Modifier.size(52.dp)) },
                            headlineContent = { Text(t.title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            supportingContent = { Text(listOfNotNull(t.artist, t.albumName).joinToString(" · "), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            trailingContent = { Text(formatMs(t.durationMs), color = MaterialTheme.colorScheme.onSurfaceVariant) })
                    }
                }
            }
        }
    }
}

@Composable
private fun SectionTitle(text: String) =
    Text(text, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 2.dp))

// ---------------------------------------------------------------------------------------------
// Settings: server address + automatic discovery

@Composable
fun SettingsScreen(vm: AppViewModel, modifier: Modifier = Modifier) {
    var text by remember { mutableStateOf(vm.address ?: "") }
    var health by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val ctx = androidx.compose.ui.platform.LocalContext.current
    // The text field follows the address when discovery finds one.
    LaunchedEffect(vm.address) { vm.address?.let { text = it } }

    Column(modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Text(stringResource(R.string.music_server), style = MaterialTheme.typography.titleMedium)
        Button(onClick = { vm.discover() }, enabled = !vm.searching) {
            if (vm.searching) { CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp); Spacer(Modifier.width(8.dp)) }
            Text(stringResource(if (vm.searching) R.string.searching else R.string.search_auto))
        }
        vm.discoveryNote?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
        OutlinedButton(onClick = { vm.push(Detail.Downloads) }) { Text(stringResource(R.string.downloaded_albums)) }
        HorizontalDivider()
        val output by vm.output.collectAsState()
        Text(stringResource(R.string.listen_on), style = MaterialTheme.typography.titleMedium)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(output == Output.Denon, { vm.chooseOutput(Output.Denon) }, label = { Text(stringResource(R.string.output_denon)) })
            FilterChip(output == Output.Phone, { vm.chooseOutput(Output.Phone) }, label = { Text(stringResource(R.string.output_phone)) })
        }
        HorizontalDivider()
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.suggestions_follow), style = MaterialTheme.typography.titleMedium)
                Text(
                    stringResource(R.string.suggestions_help),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Switch(vm.autoSimilar, { vm.updateAutoSimilar(it) })
        }
        HorizontalDivider()
        OutlinedTextField(text, { text = it }, label = { Text(stringResource(R.string.address_label)) }, singleLine = true, modifier = Modifier.fillMaxWidth())
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = {
                vm.saveAddress(text)
                health = ctx.getString(R.string.testing)
                scope.launch { health = ctx.getString(if (vm.api?.health() == true) R.string.connected else R.string.no_answer) }
            }) { Text(stringResource(R.string.save_and_test)) }
        }
        health?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
    }
}

// ---------------------------------------------------------------------------------------------
// Downloads

/** "3 albums · 1 playlist · 420 MB · 12 GB free", the line at the top of the download screens. */
@Composable
private fun storageSummary(albums: Int, playlists: Int, free: Long, labelled: Boolean): String {
    val used = fileSize(LocalLibrary.totalBytes())
    return listOf(
        pluralStringResource(R.plurals.n_albums, albums, albums),
        pluralStringResource(R.plurals.n_playlists, playlists, playlists),
        if (labelled) stringResource(R.string.size_on_phone, used) else used,
        stringResource(R.string.free_space, fileSize(free)),
    ).joinToString(" · ")
}

/** Download / progress / delete, under the "Play all" button of an album. */
@Composable
private fun DownloadControl(vm: AppViewModel, d: AlbumDetail) {
    val albums by LocalLibrary.albums.collectAsState()
    val progress by LocalLibrary.progress.collectAsState()
    val mine = albums.firstOrNull { it.id == d.album.id }
    val p = progress[d.album.id]
    var confirmDelete by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        when {
            p != null && p.error == null -> {
                Text(if (p.queued) stringResource(R.string.dl_waiting) else stringResource(R.string.dl_progress, p.done, p.total), style = MaterialTheme.typography.bodyMedium)
                LinearProgressIndicator(progress = { if (p.total > 0) p.done.toFloat() / p.total else 0f }, modifier = Modifier.fillMaxWidth(0.7f).padding(vertical = 6.dp))
                TextButton(onClick = { vm.cancelDownload(d.album.id) }) { Text(stringResource(R.string.cancel)) }
            }
            mine?.complete == true -> Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Default.DownloadDone, null, tint = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.on_phone_size, fileSize(mine.bytes)), style = MaterialTheme.typography.bodyMedium)
                TextButton(onClick = { confirmDelete = true }) { Text(stringResource(R.string.delete)) }
            }
            else -> {
                if (p?.error != null) Text(stringResource(R.string.interrupted_why, p.error), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                if (mine != null) Text(stringResource(R.string.dl_incomplete_n, mine.tracksOnPhone, d.tracks.size), style = MaterialTheme.typography.bodySmall)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedButton(onClick = { vm.downloadAlbum(d.album.id) }) {
                        Icon(Icons.Default.Download, null)
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(if (mine != null) R.string.resume else R.string.download))
                    }
                    if (mine != null) TextButton(onClick = { confirmDelete = true }) { Text(stringResource(R.string.delete)) }
                }
            }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text(stringResource(R.string.delete_title)) },
            text = { Text(stringResource(R.string.delete_album_text, d.album.name)) },
            confirmButton = { TextButton(onClick = { vm.deleteDownload(d.album.id); confirmDelete = false }) { Text(stringResource(R.string.delete)) } },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text(stringResource(R.string.keep)) } },
        )
    }
}

/** What is on the phone, how much room it takes, and a way to remove it. */
@Composable
fun DownloadsScreen(vm: AppViewModel, modifier: Modifier = Modifier) {
    val albums by LocalLibrary.albums.collectAsState()
    val progress by LocalLibrary.progress.collectAsState()
    val playlists by LocalLibrary.playlists.collectAsState()
    var toDelete by remember { mutableStateOf<DownloadedAlbum?>(null) }
    var playlistToDelete by remember { mutableStateOf<DownloadedPlaylist?>(null) }
    val context = androidx.compose.ui.platform.LocalContext.current
    val free = remember(albums) { (context.getExternalFilesDir(null) ?: context.filesDir).usableSpace }

    Column(modifier.fillMaxSize()) {
        Text(
            storageSummary(albums.size, playlists.size, free, true),
            style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(16.dp),
        )
        if (albums.isEmpty() && playlists.isEmpty()) {
            Text(stringResource(R.string.nothing_downloaded), Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        LazyColumn(Modifier.fillMaxSize()) {
            items(playlists, key = { "playlist-${it.id}" }) { d ->
                ListItem(
                    modifier = Modifier.clickable { vm.push(Detail.PlaylistPage(d.id, d.playlist.name)) },
                    leadingContent = { Cover(vm.api, d.playlist.tracks.firstOrNull()?.albumId ?: 0L, false, 64, Modifier.size(52.dp)) },
                    headlineContent = { Text(d.playlist.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    supportingContent = {
                        Text(
                            stringResource(R.string.playlist_word) + " · " + trackCount(d.playlist.tracks.size) + " · " + if (d.complete) fileSize(d.bytes) else stringResource(R.string.pl_incomplete_n, d.tracksOnPhone, d.playlist.tracks.size),
                            maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                    },
                    trailingContent = { IconButton(onClick = { playlistToDelete = d }) { Icon(Icons.Default.Delete, stringResource(R.string.delete)) } },
                )
            }
            items(albums, key = { it.id }) { a ->
                val p = progress[a.id]
                ListItem(
                    modifier = Modifier.clickable { vm.push(Detail.AlbumPage(a.id, a.detail.album.name)) },
                    leadingContent = { Cover(vm.api, a.id, a.detail.album.hasCover, 64, Modifier.size(52.dp)) },
                    headlineContent = { Text(a.detail.album.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    supportingContent = {
                        Text(
                            listOfNotNull(a.detail.album.artistText().ifEmpty { null },
                                if (a.complete) fileSize(a.bytes) else stringResource(R.string.dl_incomplete_n, a.tracksOnPhone, a.detail.tracks.size),
                                p?.let { stringResource(R.string.downloading_lower) }).joinToString(" · "),
                            maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                    },
                    trailingContent = { IconButton(onClick = { toDelete = a }) { Icon(Icons.Default.Delete, stringResource(R.string.delete)) } },
                )
            }
        }
    }
    playlistToDelete?.let { d ->
        AlertDialog(
            onDismissRequest = { playlistToDelete = null },
            title = { Text(stringResource(R.string.delete_title)) },
            text = { Text(stringResource(R.string.delete_item_text, d.playlist.name, fileSize(d.bytes))) },
            confirmButton = { TextButton(onClick = { vm.deletePlaylist(d.id); playlistToDelete = null }) { Text(stringResource(R.string.delete)) } },
            dismissButton = { TextButton(onClick = { playlistToDelete = null }) { Text(stringResource(R.string.keep)) } },
        )
    }
    toDelete?.let { a ->
        AlertDialog(
            onDismissRequest = { toDelete = null },
            title = { Text(stringResource(R.string.delete_title)) },
            text = { Text(stringResource(R.string.delete_item_text, a.detail.album.name, fileSize(a.bytes))) },
            confirmButton = { TextButton(onClick = { vm.deleteDownload(a.id); toDelete = null }) { Text(stringResource(R.string.delete)) } },
            dismissButton = { TextButton(onClick = { toDelete = null }) { Text(stringResource(R.string.keep)) } },
        )
    }
}
