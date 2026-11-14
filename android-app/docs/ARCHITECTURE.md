# Android app architecture

🇫🇷 [Français](ARCHITECTURE.fr.md)

## Principle

A single Compose activity. One `AppViewModel` holds the server address, the navigation and the player state. Screens load their own data with `LaunchedEffect`. No navigation library: a simple screen stack is enough.

```
 MainActivity ── PermissionGate ── Root (Scaffold)
                                     ├─ tabs: Albums / Downloaded / Artists / Search
                                     ├─ detail stack: AlbumDetail / ArtistPage / NowPlaying / Settings / Downloads
                                     └─ MiniPlayer (above the tab bar)
 AppViewModel ── Api (OkHttp, JSON, SSE) ──HTTP──▶ music-server (Pi) ──HEOS──▶ Denon amplifier
              ├─ discoverServer (NsdManager)
              └─ LocalLibrary (downloaded albums) · ExoPlayer (playing on the phone)
```

## Files (`app/src/main/java/app/musicremote/`)

| File | Role |
| --- | --- |
| `MainActivity.kt` | Permission gate, `Root`: top bar, tab bar, mini player, navigation, volume dialog, media controller connection |
| `PlayerRepository.kt` | Shared player state (SSE stream), remembered `has_cover`, `Output` and `Server` state |
| `RemotePlayer.kt` | Media3 player whose state and commands are the server's (the amplifier's) |
| `PlaybackService.kt` | `MediaSessionService`: notification, lock screen, the phone's ExoPlayer, player switching |
| `PhoneControl.kt`, `MediaItems.kt` | Direct ExoPlayer commands; track → `MediaItem` (local file or stream) and back |
| `LocalLibrary.kt`, `LocalCatalog.kt` | Storage of downloaded albums; local browsing and search |
| `DownloadWorker.kt` | Downloading one album (WorkManager) |
| `AppViewModel.kt` | Address (remembered), `Api`, discovery, `Detail` stack, command layer, connectivity probe, output rules |
| `Api.kt` | Server client: lists, album, search, recommendations, player commands (incl. enqueue), SSE stream, downloads |
| `Models.kt` | JSON models (`Album`, `Track`, `PlayerSnapshot`…) and `PlayerUi` |
| `Discovery.kt` | Finds the server through mDNS (`_music-server._tcp`) |
| `Screens.kt` | Album grid, album page, "Downloaded" tab, artists, search, settings, downloads management |
| `PlayerUi.kt` | Mini player, full-screen player, position extrapolation, volume |
| `Components.kt` | `Cover` (picture or icon), top bar, offline banner |
| `Format.kt` | `m:ss` durations |

## Local-network permission (Android 17)

For an app that **targets API 37**, all traffic to a local-network address (TCP to the Pi, mDNS, resolving `.local` names) is **blocked until `ACCESS_LOCAL_NETWORK` is granted**. Without it, connections **time out without any error message**. Source: <https://developer.android.com/privacy-and-security/local-network-permission>.

- Declared in the manifest, requested at first launch **before any network access** (`PermissionGate`). If refused: an explanation plus a button to the app's settings; a grant made from the settings is picked up on return (`ON_RESUME`).
- The permission belongs to the "Nearby devices" group (if another permission of the group is already granted, there is no new prompt).
- The other route (the system picker `FLAG_SHOW_PICKER`) does not cover a manually typed or remembered address, hence the choice of the permission.
- Plain HTTP: `network_security_config` with `cleartextTrafficPermitted="true"` globally. The server speaks plain HTTP and its address may change; the app handles no credentials.

## Server discovery

`discoverServer`: `NsdManager.discoverServices("_music-server._tcp")`, then for each service found `registerServiceInfoCallback` (`resolveService` is deprecated since API 34); the first IPv4 address and the port are taken. The **IP address** is used, not the `.local` name.

At first launch (no remembered address) discovery starts by itself; on failure the Settings screen opens so the address can be typed. The address found is remembered and manual entry is always available.

**Pitfall fixed**: shutting down the executor given to `NsdManager` crashes the app (`RejectedExecutionException` on the `ConnectivityThread`), because the system keeps posting cleanup tasks to it after unregistering. The executor is therefore shared and never shut down.

## Player and real time

- **SSE stream** (`GET /player/events`), open only while the app is visible (`ON_START`/`ON_STOP`). A **dedicated OkHttp client with no read timeout**: with the normal 15 s timeout the stream would be cut in a loop (the server only sends a keep-alive every ~15 s). Reconnection with an increasing delay (1 s to 10 s).
- **Position**: the amplifier only reports it every ~5 s. The app adds the time elapsed since the last event **arrived** on the phone (`SystemClock.elapsedRealtime()`), never a comparison between the Pi's clock and the phone's.
- `POST /player/*` commands answer `202` at once: the UI updates **only** from the SSE stream, never from the response. A refused command shows a message (snackbar).
- `loading` (~0.5 s at every track change) is shown as a loading state, not as an error. `heos_connected: false` and losing the stream show a warning.
- The volume follows the amplifier's (remote, HEOS app) except while the slider is being dragged.

## Lock screen, notification and volume (Media3)

When the output is the amplifier, the phone plays nothing, but Android shows the lock-screen controls and the media notification for any **`MediaSession`**. We publish one whose state is the server's.

```
 PlayerRepository (shared SSE stream) ──▶ RemotePlayer (SimpleBasePlayer) ──▶ MediaSession ──▶ PlaybackService
        ▲                                        │                                  │ (foreground service "mediaPlayback")
        └─────── POST /player/* commands ◀───────┘                      notification · lock screen · volume
```

- **`PlayerRepository`** (a singleton) owns the SSE stream. It stays open as long as a "holder" needs it: the UI while visible, the service while the player is not `stopped`. Without it the state would freeze as soon as the app goes to the background (verified: the notification title follows track changes with the app in the background).
- **`RemotePlayer`** (`SimpleBasePlayer`): the state (playlist = the server's queue, title, artist, cover, duration, extrapolated position, `playWhenReady`) is rebuilt from the server snapshot. Media3 commands are translated: play/pause, previous/next, jump to a track, stop, volume. After a command the player keeps the optimistic state until the next server data (1.5 s at most) so it does not flicker. Seeks (`COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM`, `SEEK_BACK`, `SEEK_FORWARD`, 10 s step) are sent to the server (`/player/seek`): the lock-screen progress bar is therefore draggable. The remote device also exposes the muted state (`setIsDeviceMuted`, `handleSetDeviceMuted`).
- **`PlaybackService`** (`MediaSessionService`, service type `mediaPlayback`): creates the session and the notification (icon `ic_stat_music`). The UI connects a `MediaController` to the session to start (bind) the service; Media3 moves it to the foreground when something plays. The notification has the transport category and public visibility: that is what shows it on the lock screen.
- **Permissions**: `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MEDIA_PLAYBACK`, `POST_NOTIFICATIONS` (asked once, after the local-network permission; a refusal only removes the notification).
- **Volume**: the player declares itself a **remote device** (`DeviceInfo.PLAYBACK_TYPE_REMOTE`, volume 0–100). Android then sends the phone's volume keys to the session instead of changing the phone's volume: **+2 per press**. The requested value is kept until the amplifier confirms it, otherwise several quick presses would all start from the same stale value. This only works while the service exists (app open, or playing).
- **In-app control**: a volume icon in the top bar (dialog with −, +, slider) and a slider in the full-screen player.

## Playing on the phone, downloads, offline

**Two outputs, one command layer.** `Output` = `Denon` (the server drives the amplifier) or `Phone` (ExoPlayer in `PlaybackService`). The UI never calls the API or ExoPlayer directly: it goes through `AppViewModel` (`playAlbum`, `pause`, `seekTo`, `setVolume`, `setMute`…), which routes to one or the other. The displayed state (`vm.player`) also comes from the active output: the server snapshot for the amplifier, a snapshot rebuilt from ExoPlayer for the phone (`publishPhone`, same `PlayerSnapshot` shape): every screen therefore serves both.

- **Media session**: `MediaSession.setPlayer` swaps `RemotePlayer` and `ExoPlayer` when the output changes. Notification, lock screen and ±10 s buttons stay identical; the phone's volume keys follow (local volume for ExoPlayer, remote volume for `RemotePlayer`).
- **Seeking in MP3 files without a header**: many real-world MP3 files have neither a Xing/Info table nor VBRI (6 of the 12 tracks downloaded during testing: a whole album). ExoPlayer **refuses to seek in them** by default: the ±10 s buttons and the progress bar had no effect on the phone. `DefaultExtractorsFactory().setConstantBitrateSeekingEnabled(true)` (passed to `DefaultMediaSourceFactory`) makes it assume a constant bitrate when it has nothing better: slightly approximate for a header-less VBR (a tap at ~1/4 of the bar of a 12-minute track landed at 3:11 instead of 3:07), with no audible consequence. Files with a header keep their exact table.
- **ExoPlayer settings**: audio focus, pause when headphones are unplugged (`setHandleAudioBecomingNoisy`), a **10 s** skip step both ways (the default is 5 s / 15 s: the ±10 s buttons would lie), `WAKE_MODE_LOCAL` for files, `WAKE_MODE_NETWORK` for streams, `WAKE_LOCK` permission.
- **Hardware media keys** (Bluetooth headset) work when the output is the phone: Android sends them to the app that actually plays audio (`Media button session is app.musicremote`). With the amplifier as output they do not reach the session: Android routes them to apps playing audio locally. A `MediaButtonReceiver` is declared (the system takes it into account) but is not enough on its own.
- **Where phone tracks come from** (`phoneItem`): the downloaded file if there is one, otherwise the server's `GET /stream/{id}.mp3` (the server handles `Range` requests) if the server is reachable, otherwise nothing.

**Storage** (`LocalLibrary`). `getExternalFilesDir/music/album_<id>/`: `album.json` (the album and its tracks as the server sent them), `cover.jpg` (600 px), `t<id>.mp3`, and a `.complete` marker written at the very end. Files are written as `*.part` then renamed: a file with its final name is always whole. App-private space: never purged like a cache, no storage permission, removed with the app. The state (list, progress) is exposed as `StateFlow`s.

**Playlists** (the queue kept on the phone). A button next to the queue in the full-screen player (`SavePlaylistDialog`, `Playlists.kt`) saves the queue as it is at that moment, de-duplicated, as `playlist_<id>/` (`id` = creation time in ms): `playlist.json` (name + tracks, built from the queue items, so no server route is needed), `t<id>.mp3`, `cover_<albumId>.jpg`, `.complete`. The list is written first, so the playlist shows up at once as "pending"; `DownloadWorker.enqueuePlaylist` then fills it on the same `downloads` chain as the albums (own tag `dlp_<id>`, own progress flow `playlistProgress`). A playlist owns its files: a track already on the phone (in an album) is copied, not downloaded again, so deleting an album never breaks a playlist. Covers: copied when the album is on the phone, otherwise downloaded only if `GET /albums/{id}` says `has_cover`. `LocalLibrary.trackFile` and `coverFile` look in the playlists too. The automatic name is the most frequent artist ("et autres" when there are several) + the date, with " (2)" on a clash (`autoPlaylistName`); the size shown before downloading is an estimate (192 kbit/s assumed: the server does not give file sizes). `PlaylistScreen` plays it (`playPlaylist`: phone, or amplifier when the server is there), resumes or cancels the download, renames, deletes.

**Downloading** (`DownloadWorker`, WorkManager). One task per album, **chained** (`enqueueUniqueWork("downloads", APPEND_OR_REPLACE)`, never in parallel), network constraint, a foreground service of type `dataSync` with a notification (a big album, or a 200-track audiobook, outlasts the ~10 minutes of an ordinary background task). Before each track: a free-space check (`Content-Length` + 300 MB margin). Interrupted (network, server restarting): retried with an increasing delay (5 attempts); the resume starts at the **first missing track**. The `.part` file of an interrupted track is deleted. A missing cover does not fail the album.

**Offline.** `Server` = `Unknown | Online | Offline` (three states, to avoid an "offline" flash at start-up).
- Probe: `GET /health` with 1.5 s timeouts and a **body check** (`{"status":"ok"}`): on another network the same address may be another device. At start-up, on network change (`ConnectivityManager`), on "Retry", and every 8 s while offline (30 s otherwise). The first failure counts at once; afterwards **two in a row** are needed (a Wi-Fi blink at home must not flip the app). An open SSE stream proves the server is there.
- While the state is unknown, screens wait (no network call hanging 5 s). Offline, **no call to the server**: albums, artists and search read `LocalLibrary` (`LocalCatalog`: accent- and case-insensitive search), an album page reads its `album.json`, covers come from local files, the artist list is limited to downloaded albums. An "Offline" banner with "Retry" is shown, and the app opens on the "Downloaded" tab.
- **Output rule**: offline → phone; otherwise the user's choice. **Never switch while something is playing**: if you come home while the phone is playing, it carries on until it stops; if you leave while the amplifier plays, nothing is taken away as long as the server answers. The one exception: server gone, the amplifier can no longer be driven anyway.
- **Manual output change** (player selector or settings): the side being left stops (nothing plays twice) and the other **takes over the same queue, track and position**. Amplifier → phone: instant. Phone → amplifier: the server starts the queue then, 1.8 s later, moves the playback (about a second of silence). Not possible towards the amplifier while offline (button disabled).

## Skip buttons, mute, draggable timeline

- **Progress bar** (full-screen player): a `Slider`. The position is sent **only when the finger is lifted**: each send restarts the amplifier's stream (~1 s of silence). The chosen value stays displayed until the next server snapshot, so the thumb does not jump back. Disabled if the duration is unknown or when stopped/loading.
- **Buttons**: −30 / −10 / +10 / +30 s in the full-screen player; −10 / +10 s in the mini player; they send `POST /player/seek {"delta_ms":±N}` (the server computes from **its** position, not the screen's).
- **Notification and lock screen**: `setMediaButtonPreferences` with two `CommandButton`s (`ICON_SKIP_BACK_10`, `ICON_SKIP_FORWARD_10`) placed in the **`SLOT_OVERFLOW`** slot. Without an explicit slot they took the place of Previous/Next (verified with `dumpsys notification`). Result: Previous, Pause, Next, −10 s, +10 s (five buttons, Android's maximum). The notification's +10 s button was tested with a real tap.
- **Mute**: a speaker button in the full-screen player (next to the volume slider, which is greyed out and shows "muted") and in the volume dialog; `POST /player/mute`.

## Suggestions ("same style")

`AppViewModel.suggestSimilar()` asks `GET /recommendations` for the current track (excluding what is queued) and appends the answer: `POST /player/enqueue` on the amplifier, `addMediaItems` on the phone (if the phone's player had ended, it jumps to the first new track; on the amplifier, if it had stopped, it jumps to the old queue size). It needs the server (`Server.Online`), shows what happened in the snackbar and never runs twice at once (`suggesting`).

**With the amplifier, the server does it** (`suggest.rs`): the snapshot carries `auto_suggest` and a `reason` on every track the server added, so `maybeAutoSuggest` does nothing when `auto_suggest` is present, the queue labels the tracks from `reason`, and the Settings switch follows the server's setting (`followServerSetting`) and writes it (`POST /player/auto_suggest`), shared by all phones. The rest of this paragraph is what the app does itself: on the phone's player, and with a server that has no `auto_suggest`.

**Removing a track**: each queue row of `NowPlayingScreen` is a `SwipeToDismissBox` (both directions, disabled for the track being played); a swipe calls `AppViewModel.removeFromQueue(index, trackId)` (`POST /player/remove` with the amplifier, `removeMediaItem` on the phone). The server refuses when the track at `index` is no longer `trackId`; the row key contains `queueRevision`, bumped on failure so the swiped row comes back.

**Endless scrolling**: `NowPlayingScreen` watches its `LazyColumn` (`snapshotFlow` on the last visible item, only once the user has scrolled); close to the end of the queue it calls `AppViewModel.loadMoreSuggestions()`, which asks for 10 more (`POST /player/suggest_more` with the amplifier, `suggestSimilar` seeded with the queue's last track on the phone), shows a "Recherche de suggestions…" row while waiting, does nothing when the setting is off or the server is not `Online`, and asks once per queue state (`moreKey` = size + last track: an empty answer does not loop, and a batch whose tracks have not arrived yet is not asked twice).

Automatic suggestions (`autoSimilar`, on by default, in Settings): a collector on the UI player state calls it (`maybeAutoSuggest`) **right after a queue starts** (no track of the queue is known as suggested yet): 10 tracks close to the queue's last track are appended; and again when the queue's last track starts, so it never runs dry. Each attempt is made once (`autoDoneKey`: nothing is retried when nothing was found). The suggested tracks are remembered (`suggested`: id -> why, kept in the preferences, 400 at most) and the queue of the full-screen player marks them: a star instead of the number, the reason (Last.fm, same genre, same artist) and a "Suggestions" heading before the first one. The server decides where the suggestions come from; the app only says in Settings that the server may query Last.fm.

## Icon

Adaptive icon in vectors (`res/drawable/ic_launcher_*.xml`, `res/mipmap-anydpi-v26/`): a raspberry (20 drupelets and three dark green leaves, the Raspberry Pi) on a black background, and in front a gold music note shaped like a J (J for jukebox: top bar, stem, the hook ends in the note head) with a black outline so it stays readable on the red. The raspberry is drawn in the background layer, scaled to stay inside the visible 72 dp window; the J stays inside the 66 dp safe zone. A `monochrome` layer is provided for Android 13+ themed icons (the J in full, the raspberry as a faint shape). Pitfall met: the note head and the stem must be separate paths, otherwise their opposite drawing directions cancel where they overlap (visible hole).

## Covers

Coil 3 (`AsyncImage`) loads `GET /cover/{id}?size=` at 64 px (lists), 300 px (grid), 600 px (album page, player). **A cover is never requested when `has_cover` is false**: the server would answer `404`. The ViewModel remembers `has_cover` of each album seen (`remember`) so the mini player knows whether the current album has one; unknown = try, with the note icon as a fallback on error. Downloaded albums show their local `cover.jpg`, with no network.

## Languages

All texts are resources: `res/values/strings.xml` is English (the fallback for any other language), `values-fr`, `-de`, `-it`, `-es`, `-pt` (+ `-pt-rBR` overrides), `-nl`, `-ca`, `-sv`, `-da`, `-nb`, `-fi`, `-pl` are the translations. The app follows the phone's language; `res/xml/locales_config.xml` (`android:localeConfig`) lists them for the per-app language setting of Android 13+, and `androidResources.localeFilters` keeps the libraries from adding strings in other languages (the app would be half translated for those users). Compose uses `stringResource` / `pluralStringResource`; the view model and the download worker use `getString`. What is not translated on purpose: protocol values (`status`, reasons such as `same_genre` are mapped to a label at display time), the product name, the `Denon` brand, and error messages that come from the server or from the system. Sizes use `Formatter.formatShortFileSize`; the date of an automatic playlist name uses `DateFormat.getBestDateTimePattern`. A failure that the code must recognise is a type (`NotEnoughSpace`), never a message.

## Lists

Album grid: 60 albums per page, the next page is requested when one of the last 12 items becomes visible. The "random" sort has no pagination (a button reshuffles). Search: 150 ms wait after the last keystroke, then one `GET /search` call (the server answers in 10–17 ms).

## What is deliberately not done

- No dependency-injection framework, no local database and no navigation library: not needed at this size (downloaded albums are files plus an `album.json`).
- No automatic downloading and no playlists: albums are downloaded one by one.
- Offline, only downloaded content is available: the server's lists are not cached.
