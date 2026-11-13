package app.musicremote

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Album
import androidx.compose.material.icons.filled.DownloadForOffline
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import androidx.compose.ui.platform.LocalLifecycleOwner

// Android 17 (API 37): connections to the LAN, including mDNS, are blocked until the user grants this.
private const val LOCAL_NETWORK_PERMISSION = "android.permission.ACCESS_LOCAL_NETWORK"

class MainActivity : ComponentActivity() {
    private val vm: AppViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val ctx = LocalContext.current
            val dark = isSystemInDarkTheme()
            MaterialTheme(colorScheme = if (dark) dynamicDarkColorScheme(ctx) else dynamicLightColorScheme(ctx)) {
                Surface(Modifier.fillMaxSize()) { PermissionGate { Root(vm) } }
            }
        }
    }
}

@Composable
private fun PermissionGate(content: @Composable () -> Unit) {
    val ctx = LocalContext.current
    var granted by remember { mutableStateOf(hasLocalNetwork(ctx)) }
    var asked by remember { mutableStateOf(false) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> granted = ok; asked = true }
    // The user may also grant it from the system settings and come back.
    val owner = LocalLifecycleOwner.current
    DisposableEffect(owner) {
        val obs = LifecycleEventObserver { _, e -> if (e == Lifecycle.Event.ON_RESUME) granted = hasLocalNetwork(ctx) }
        owner.lifecycle.addObserver(obs)
        onDispose { owner.lifecycle.removeObserver(obs) }
    }
    LaunchedEffect(Unit) { if (!granted) launcher.launch(LOCAL_NETWORK_PERMISSION) }

    if (granted) { content(); return }
    Column(Modifier.fillMaxSize().safeDrawingPadding().padding(24.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Text("Accès au réseau local", style = MaterialTheme.typography.headlineSmall)
        Spacer(Modifier.height(12.dp))
        Text("Android 17 demande ton accord pour que l'appli joigne ton serveur de musique sur le réseau de la maison.", style = MaterialTheme.typography.bodyLarge)
        Spacer(Modifier.height(20.dp))
        Button(onClick = { launcher.launch(LOCAL_NETWORK_PERMISSION) }) { Text("Autoriser") }
        if (asked) {
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = {
                ctx.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${ctx.packageName}")))
            }) { Text("Ouvrir les réglages de l'application") }
        }
    }
}

private fun hasLocalNetwork(ctx: Context) =
    ContextCompat.checkSelfPermission(ctx, LOCAL_NETWORK_PERMISSION) == PackageManager.PERMISSION_GRANTED

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Root(vm: AppViewModel) {
    val owner = LocalLifecycleOwner.current
    // Live player stream only while the app is visible.
    DisposableEffect(owner) {
        val obs = LifecycleEventObserver { _, e ->
            when (e) { Lifecycle.Event.ON_START -> vm.onForeground(true); Lifecycle.Event.ON_STOP -> vm.onForeground(false); else -> {} }
        }
        owner.lifecycle.addObserver(obs)
        if (owner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) vm.onForeground(true)
        onDispose { owner.lifecycle.removeObserver(obs); vm.onForeground(false) }
    }
    // Connecting a controller binds PlaybackService: that is what publishes the lock screen / notification controls.
    val appCtx = LocalContext.current
    DisposableEffect(Unit) {
        val token = SessionToken(appCtx, ComponentName(appCtx, PlaybackService::class.java))
        val future = MediaController.Builder(appCtx, token).buildAsync()
        onDispose { MediaController.releaseFuture(future) }
    }
    // Android 13+: the media notification needs this permission. Asked once the network permission is settled;
    // a refusal only removes the notification, the app keeps working.
    val notificationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    LaunchedEffect(Unit) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(appCtx, android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) notificationLauncher.launch(android.Manifest.permission.POST_NOTIFICATIONS)
    }
    // First launch: look for the server on the network.
    LaunchedEffect(Unit) { if (vm.api == null) vm.discover() }

    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(vm.message) { vm.message?.let { snackbar.showSnackbar(it); vm.message = null } }

    var showVolume by remember { mutableStateOf(false) }
    if (showVolume) VolumeDialog(vm) { showVolume = false }

    val server by vm.server.collectAsState()
    // Opening the app away from home: land on what can actually be played.
    var landed by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(server) {
        if (server == Server.Offline && !landed) { vm.tab = Tab.Downloaded; landed = true }
    }

    val detail = vm.details.lastOrNull()
    val volumeAction: @Composable RowScope.() -> Unit = {
        IconButton(onClick = { showVolume = true }) { Icon(Icons.AutoMirrored.Filled.VolumeUp, "Volume") }
    }
    BackHandler(enabled = detail != null) { vm.pop() }

    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            when (detail) {
                null -> TopBar(
                    when (vm.tab) { Tab.Albums -> "Albums"; Tab.Downloaded -> "Sur le téléphone"; Tab.Artists -> "Artistes"; Tab.Search -> "Recherche" },
                    actions = {
                        volumeAction()
                        IconButton(onClick = { vm.push(Detail.Settings) }) { Icon(Icons.Default.Settings, "Serveur") }
                    },
                )
                is Detail.AlbumPage -> TopBar(detail.title, onBack = vm::pop, actions = volumeAction)
                is Detail.ArtistPage -> TopBar(detail.name, onBack = vm::pop, actions = volumeAction)
                Detail.NowPlaying -> TopBar("Lecture en cours", onBack = vm::pop, actions = volumeAction)
                Detail.Settings -> TopBar("Serveur", onBack = vm::pop)
                Detail.Downloads -> TopBar("Albums téléchargés", onBack = vm::pop)
            }
        },
        bottomBar = {
            Column {
                if (detail != Detail.NowPlaying && detail != Detail.Settings && detail != Detail.Downloads) MiniPlayer(vm)
                if (detail == null) {
                    NavigationBar {
                        NavigationBarItem(vm.tab == Tab.Albums, { vm.tab = Tab.Albums }, { Icon(Icons.Default.Album, null) }, label = { Text("Albums") })
                        NavigationBarItem(vm.tab == Tab.Downloaded, { vm.tab = Tab.Downloaded }, { Icon(Icons.Default.DownloadForOffline, null) }, label = { Text("Téléchargés") })
                        NavigationBarItem(vm.tab == Tab.Artists, { vm.tab = Tab.Artists }, { Icon(Icons.Default.Person, null) }, label = { Text("Artistes") })
                        NavigationBarItem(vm.tab == Tab.Search, { vm.tab = Tab.Search }, { Icon(Icons.Default.Search, null) }, label = { Text("Recherche") })
                    }
                }
            }
        },
    ) { padding ->
        val m = Modifier
        Column(Modifier.padding(padding)) {
            if (server == Server.Offline) OfflineBanner(vm::probeNow)
            Box(Modifier.weight(1f)) {
                when (detail) {
                    null -> when (vm.tab) {
                        Tab.Albums -> AlbumsScreen(vm, null, m)
                        Tab.Downloaded -> DownloadedScreen(vm, m)
                        Tab.Artists -> ArtistsScreen(vm, m)
                        Tab.Search -> SearchScreen(vm, m)
                    }
                    is Detail.AlbumPage -> AlbumDetailScreen(vm, detail.id, m)
                    is Detail.ArtistPage -> AlbumsScreen(vm, detail.name, m)
                    Detail.NowPlaying -> NowPlayingScreen(vm, m)
                    Detail.Settings -> SettingsScreen(vm, m)
                    Detail.Downloads -> DownloadsScreen(vm, m)
                }
            }
        }
    }
}
