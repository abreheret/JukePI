# Architecture de l'appli Android

🇬🇧 [English](ARCHITECTURE.md)

## Principe

Une seule activité Compose. Un `AppViewModel` unique garde l'adresse du serveur, la navigation et l'état du lecteur. Les écrans chargent leurs données eux-mêmes avec `LaunchedEffect`. Pas de bibliothèque de navigation : une pile d'écrans simple suffit.

```
 MainActivity ── PermissionGate ── Root (Scaffold)
                                     ├─ onglets : Albums / Téléchargés / Artistes / Recherche
                                     ├─ pile de détails : AlbumDetail / ArtistPage / NowPlaying / Settings
                                     └─ MiniPlayer (au-dessus de la barre d'onglets)
 AppViewModel ── Api (OkHttp, JSON, SSE) ──HTTP──▶ music-server (Pi) ──HEOS──▶ ampli Denon
              ├─ discoverServer (NsdManager)
              └─ LocalLibrary (albums téléchargés) · ExoPlayer (lecture sur le téléphone)
```

## Fichiers (`app/src/main/java/app/musicremote/`)

| Fichier | Rôle |
| --- | --- |
| `MainActivity.kt` | Porte d'entrée de la permission, `Root` : barre du haut, barre d'onglets, mini-lecteur, navigation, fenêtre de volume, connexion du contrôleur média |
| `PlayerRepository.kt` | État du lecteur partagé (flux SSE), `has_cover` mémorisé |
| `RemotePlayer.kt` | Lecteur Media3 dont l'état et les commandes sont ceux du serveur |
| `PlaybackService.kt` | Service `MediaSessionService` : notification, écran verrouillé, ExoPlayer du téléphone, bascule des lecteurs |
| `PhoneControl.kt`, `MediaItems.kt` | Commandes directes d'ExoPlayer ; titre → `MediaItem` (fichier local ou flux) et retour |
| `LocalLibrary.kt`, `LocalCatalog.kt` | Stockage des albums téléchargés ; parcours et recherche locaux |
| `DownloadWorker.kt` | Téléchargement d'un album (WorkManager) |
| `AppViewModel.kt` | Adresse (mémorisée), `Api`, découverte, pile `Detail`, commandes, flux SSE avec reconnexion |
| `Api.kt` | Client du serveur : listes, album, recherche, recommandations, commandes du lecteur (dont enqueue), flux SSE |
| `Models.kt` | Modèles JSON (`Album`, `Track`, `PlayerSnapshot`…) et `PlayerUi` |
| `Discovery.kt` | Recherche du serveur par mDNS (`_music-server._tcp`) |
| `Screens.kt` | Grille d'albums, page album, artistes, recherche, réglages |
| `PlayerUi.kt` | Mini-lecteur, lecteur plein écran, extrapolation de la position, volume |
| `Components.kt` | `Cover` (pochette ou icône), barre du haut |
| `Format.kt` | Durées `m:ss` |

## Permission réseau local (Android 17)

Pour une appli qui **cible l'API 37**, tout le trafic vers une adresse du réseau local (TCP vers le Pi, mDNS, résolution des noms `.local`) est **bloqué tant que `ACCESS_LOCAL_NETWORK` n'est pas accordée**. Sans elle, les connexions **expirent sans message d'erreur**. Source : <https://developer.android.com/privacy-and-security/local-network-permission>.

- Déclarée dans le manifeste, demandée au premier lancement **avant tout accès réseau** (`PermissionGate`). Si elle est refusée : explication + bouton vers les réglages de l'appli ; l'accord donné depuis les réglages est repris au retour (`ON_RESUME`).
- La permission fait partie du groupe « Appareils à proximité » (si une autre permission du groupe est déjà accordée, pas de nouvelle demande).
- Les autres voies (sélecteur système `FLAG_SHOW_PICKER`) ne couvrent pas une adresse saisie à la main ou mémorisée, d'où le choix de la permission.
- HTTP en clair : `network_security_config` avec `cleartextTrafficPermitted="true"` globalement. Le serveur parle en HTTP simple et son adresse peut changer ; l'appli ne manipule aucun identifiant.

## Découverte du serveur

`discoverServer` : `NsdManager.discoverServices("_music-server._tcp")`, puis pour chaque service trouvé `registerServiceInfoCallback` (la méthode `resolveService` est dépréciée depuis l'API 34), on prend la première adresse IPv4 et le port. On utilise l'**adresse IP**, pas le nom `.local`.

Au premier lancement (aucune adresse mémorisée), la découverte démarre toute seule ; en cas d'échec, l'écran Réglages s'ouvre pour saisir l'adresse. L'adresse trouvée est mémorisée et la saisie manuelle reste toujours disponible.

**Piège réglé** : fermer (`shutdown`) l'exécuteur donné à `NsdManager` fait planter l'appli (`RejectedExecutionException` sur le thread `ConnectivityThread`), car le système y poste encore des tâches de nettoyage après le désenregistrement. L'exécuteur est donc partagé et jamais fermé.

## Lecteur et temps réel

- **Flux SSE** (`GET /player/events`), ouvert seulement quand l'appli est visible (`ON_START`/`ON_STOP`). Client OkHttp **dédié sans délai de lecture** : avec le délai normal de 15 s, le flux serait coupé en boucle (le serveur n'envoie un keep-alive que toutes les ~15 s). Reconnexion avec attente croissante (1 s à 10 s).
- **Position** : le Denon ne la donne que toutes les ~5 s. L'appli ajoute le temps écoulé depuis l'**arrivée** du dernier événement sur le téléphone (`SystemClock.elapsedRealtime()`), jamais une comparaison entre l'horloge du Pi et celle du téléphone.
- Les commandes `POST /player/*` répondent `202` tout de suite : l'interface se met à jour **uniquement** avec le flux SSE, jamais avec la réponse. Une commande refusée affiche un message (barre en bas).
- `loading` (~0,5 s à chaque changement de titre) s'affiche comme un chargement, pas comme une erreur. `heos_connected: false` et la perte du flux affichent un avertissement.
- Le volume suit celui de l'ampli (télécommande, appli HEOS) sauf pendant qu'on déplace le curseur.

## Écran verrouillé, notification et volume (Media3)

Quand la sortie est l'ampli, le téléphone ne joue rien, mais Android affiche les commandes de l'écran verrouillé et la notification multimédia pour toute **`MediaSession`**. On en publie une dont l'état est celui du serveur.

```
 PlayerRepository (flux SSE, partagé) ──▶ RemotePlayer (SimpleBasePlayer) ──▶ MediaSession ──▶ PlaybackService
        ▲                                        │                                  │ (service premier plan "mediaPlayback")
        └─────── commandes POST /player/* ◀──────┘                      notification · écran verrouillé · volume
```

- **`PlayerRepository`** (objet unique) possède le flux SSE. Il reste ouvert tant qu'un « détenteur » en a besoin : l'interface quand elle est visible, le service tant que le lecteur n'est pas `stopped`. Sans cela, l'état se figerait dès que l'appli passe en arrière-plan (vérifié : le titre de la notification suit les changements de piste avec l'appli en arrière-plan).
- **`RemotePlayer`** (`SimpleBasePlayer`) : l'état (liste de lecture = file d'attente du serveur, titre, artiste, pochette, durée, position extrapolée, `playWhenReady`) est reconstruit à partir de l'instantané du serveur. Les commandes Media3 sont traduites : lecture/pause, précédent/suivant, saut à un titre, arrêt, volume. Après une commande, le lecteur garde l'état optimiste jusqu'à la prochaine donnée du serveur (1,5 s au plus) pour ne pas clignoter. Les déplacements (`COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM`, `SEEK_BACK`, `SEEK_FORWARD`, pas de 10 s) sont envoyés au serveur (`/player/seek`) : la barre de progression de l'écran verrouillé est donc déplaçable. L'appareil distant expose aussi l'état muet (`setIsDeviceMuted`, `handleSetDeviceMuted`).
- **`PlaybackService`** (`MediaSessionService`, type de service `mediaPlayback`) : crée la session et la notification (icône `ic_stat_music`, canal « Lecture sur le Denon »). L'interface connecte un `MediaController` à la session pour démarrer (lier) le service ; Media3 le passe au premier plan quand quelque chose joue. La notification est de catégorie transport, visibilité publique : c'est ce qui l'affiche sur l'écran verrouillé.
- **Permissions** : `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MEDIA_PLAYBACK`, `POST_NOTIFICATIONS` (demandée une fois, après la permission réseau local ; un refus supprime seulement la notification).
- **Volume** : le lecteur se déclare **appareil distant** (`DeviceInfo.PLAYBACK_TYPE_REMOTE`, volume 0 à 100). Android envoie alors les touches de volume du téléphone à la session au lieu de changer le volume du téléphone : **+2 par appui**. La valeur demandée est conservée jusqu'à confirmation du Denon, sinon plusieurs appuis rapides partiraient tous de la même valeur périmée. Cela ne marche que tant que le service existe (appli ouverte, ou lecture en cours).
- **Réglage dans l'appli** : icône de volume dans la barre du haut (fenêtre avec −, +, curseur) et curseur dans le lecteur plein écran.
- **Touches multimédia matérielles** : voir « Limites connues » du README. Un `MediaButtonReceiver` est déclaré (le système en tient compte) mais ne suffit pas, parce que le routage dépend de l'audio joué sur le téléphone.

## Lecture sur le téléphone, téléchargements, hors connexion

**Deux sorties, une seule couche de commandes.** `Output` = `Denon` (le serveur pilote le Denon) ou `Phone` (ExoPlayer dans `PlaybackService`). L'interface n'appelle jamais l'API ou ExoPlayer directement : elle passe par `AppViewModel` (`playAlbum`, `pause`, `seekTo`, `setVolume`, `setMute`…), qui route vers l'une ou l'autre. L'état affiché (`vm.player`) vient lui aussi de la sortie active : instantané du serveur pour le Denon, instantané reconstruit à partir d'ExoPlayer pour le téléphone (`publishPhone`, même forme `PlayerSnapshot`) : tous les écrans servent donc aux deux.

- **Session média** : `MediaSession.setPlayer` échange `RemotePlayer` et `ExoPlayer` quand la sortie change. Notification, écran verrouillé et boutons ±10 s restent identiques ; le volume des touches du téléphone suit (volume local pour ExoPlayer, volume distant pour `RemotePlayer`).
- **Déplacement dans les MP3 sans en-tête** : beaucoup de fichiers de la bibliothèque n'ont ni table Xing/Info ni VBRI (sur les 12 titres téléchargés lors des tests, 6 : un album entier). ExoPlayer **refuse de s'y déplacer** par défaut : les boutons ±10 s et la barre de progression restaient sans effet sur le téléphone. `DefaultExtractorsFactory().setConstantBitrateSeekingEnabled(true)` (passé à `DefaultMediaSourceFactory`) lui fait supposer un débit constant quand il n'a rien de mieux : légèrement approximatif pour un VBR sans en-tête (un appui à ~1/4 de la barre d'un titre de 12 min est tombé à 3:11 au lieu de 3:07), sans conséquence audible. Les fichiers avec en-tête gardent leur table exacte.
- **Réglages d'ExoPlayer** : focus audio, pause quand on débranche le casque (`setHandleAudioBecomingNoisy`), pas de saut **10 s** dans les deux sens (le défaut est 5 s / 15 s : les boutons ±10 s mentiraient), `WAKE_MODE_LOCAL` pour des fichiers, `WAKE_MODE_NETWORK` pour du flux, permission `WAKE_LOCK`.
- **Les touches multimédia matérielles** (casque Bluetooth) fonctionnent quand la sortie est le téléphone : Android les envoie à l'appli qui joue réellement de l'audio (`Media button session is app.musicremote`).
- **Origine des titres sur le téléphone** (`phoneItem`) : le fichier téléchargé s'il existe, sinon `GET /stream/{id}.mp3` du serveur (requêtes `Range` gérées par le serveur) si le serveur est joignable, sinon rien.

**Stockage** (`LocalLibrary`). `getExternalFilesDir/music/album_<id>/` : `album.json` (l'album et ses titres tels que le serveur les a donnés), `cover.jpg` (600 px), `t<id>.mp3`, marqueur `.complete` écrit à la toute fin. Les fichiers sont écrits en `*.part` puis renommés : un fichier au nom définitif est toujours entier. Espace propre à l'appli : jamais purgé comme un cache, pas de permission de stockage, supprimé avec l'appli. L'état (liste, progression) est exposé en `StateFlow`.

**Playlists** (la file gardée sur le téléphone). Un bouton à côté de la file dans le lecteur plein écran (`SavePlaylistDialog`, `Playlists.kt`) enregistre la file telle qu'elle est à cet instant, sans doublons, dans `playlist_<id>/` (`id` = heure de création en ms) : `playlist.json` (nom + titres, construits à partir des éléments de la file : aucune route serveur n'est nécessaire), `t<id>.mp3`, `cover_<albumId>.jpg`, `.complete`. La liste est écrite d'abord, la playlist apparaît donc tout de suite « en attente » ; `DownloadWorker.enqueuePlaylist` la remplit ensuite sur la même chaîne `downloads` que les albums (tag `dlp_<id>`, flux de progression `playlistProgress`). Une playlist possède ses fichiers : un titre déjà sur le téléphone (dans un album) est copié, pas retéléchargé, donc supprimer un album ne casse jamais une playlist. Pochettes : copiées si l'album est sur le téléphone, sinon téléchargées seulement si `GET /albums/{id}` dit `has_cover`. `LocalLibrary.trackFile` et `coverFile` regardent aussi dans les playlists. Le nom automatique est l'artiste le plus présent (« et autres » s'ils sont plusieurs) + la date, avec « (2) » en cas de conflit (`autoPlaylistName`) ; la taille affichée avant le téléchargement est une estimation (192 kbit/s supposés : le serveur ne donne pas les tailles). `PlaylistScreen` la joue (`playPlaylist` : téléphone, ou ampli quand le serveur est là), reprend ou annule le téléchargement, renomme, supprime.

**Téléchargement** (`DownloadWorker`, WorkManager). Une tâche par album, **en chaîne** (`enqueueUniqueWork("downloads", APPEND_OR_REPLACE)`, jamais en parallèle), contrainte réseau, service de premier plan de type `dataSync` avec notification (un gros album ou le livre audio de 200 titres dépasse les ~10 min d'une tâche ordinaire). Avant chaque titre : vérification de l'espace libre (`Content-Length` + 300 Mo de marge). Interrompu (réseau, serveur redémarré) : nouvel essai avec attente croissante (5 tentatives) ; la reprise repart du **premier titre manquant**. Le `.part` d'un titre interrompu est supprimé. Les pochettes manquantes n'échouent pas l'album.

**Hors connexion.** `Server` = `Unknown | Online | Offline` (3 états pour éviter un clignotement « hors connexion » au démarrage).
- Sonde : `GET /health` avec délais de 1,5 s et **vérification du corps** (`{"status":"ok"}`) : sur un autre réseau, `192.168.1.50` peut être un autre appareil. Au démarrage, au changement de réseau (`ConnectivityManager`), à « Réessayer », et toutes les 8 s tant qu'on est hors connexion (30 s sinon). Le premier échec compte tout de suite ; ensuite il en faut **deux de suite** (un clignotement du Wi-Fi à la maison ne bascule pas l'appli). Un flux SSE ouvert prouve que le serveur est là.
- Tant que l'état n'est pas connu, les écrans attendent (pas d'appel réseau qui traîne 5 s). Hors connexion, **aucun appel au serveur** : albums, artistes et recherche lisent `LocalLibrary` (`LocalCatalog` : recherche sans accents ni casse), la page d'un album lit son `album.json`, les pochettes viennent des fichiers locaux, la liste des artistes est limitée aux albums téléchargés. Un bandeau « Hors connexion » avec « Réessayer » est affiché, et l'appli s'ouvre sur l'onglet « Téléchargés ».
- **Règle de la sortie** : hors connexion → téléphone ; sinon le choix de l'utilisateur. **Jamais de changement pendant qu'une lecture est en cours** : si on rentre à la maison pendant que le téléphone joue, il continue jusqu'à l'arrêt ; si on part pendant que le Denon joue, rien n'est volé tant que le serveur répond. Seule exception : serveur disparu, le Denon ne peut de toute façon plus être piloté.
- **Changement manuel de sortie** (sélecteur du lecteur ou réglages) : le côté quitté s'arrête (rien ne joue en double) et l'autre **reprend la même file, le même titre, la même position**. Denon → téléphone : instantané. Téléphone → Denon : le serveur lance la file puis, 1,8 s plus tard, déplace la lecture (environ une seconde de silence). Impossible vers le Denon hors connexion (bouton grisé).

## Boutons de saut, coupure du son, barre déplaçable

- **Barre de progression** (lecteur plein écran) : un `Slider`. On n'envoie la position **qu'au relâchement** du doigt : chaque envoi relance le flux du Denon (~1 s de silence). La valeur choisie reste affichée jusqu'au prochain instantané du serveur, pour que le curseur ne revienne pas en arrière. Désactivée si la durée est inconnue ou à l'arrêt/en chargement.
- **Boutons** : −30 / −10 / +10 / +30 s dans le lecteur plein écran ; −10 / +10 s dans le mini-lecteur ; envoi de `POST /player/seek {"delta_ms":±N}` (le serveur calcule à partir de **sa** position, pas de celle de l'écran).
- **Notification et écran verrouillé** : `setMediaButtonPreferences` avec deux `CommandButton` (`ICON_SKIP_BACK_10`, `ICON_SKIP_FORWARD_10`) placés dans l'emplacement **`SLOT_OVERFLOW`**. Sans emplacement explicite, ils prenaient la place de Précédent/Suivant (vérifié avec `dumpsys notification`). Résultat : Précédent, Pause, Suivant, −10 s, +10 s (cinq boutons, le maximum d'Android). Le bouton +10 s de la notification a été testé par un vrai appui.
- **Coupure du son** : bouton haut-parleur dans le lecteur plein écran (à côté du curseur de volume, qui est grisé et affiche « muet ») et dans la fenêtre de volume ; `POST /player/mute`.

## Suggestions (« même style »)

`AppViewModel.suggestSimilar()` demande `GET /recommendations` pour le titre en cours (en excluant ce qui est en file) et ajoute la réponse : `POST /player/enqueue` sur l'ampli, `addMediaItems` sur le téléphone (si le lecteur du téléphone était arrivé au bout, il saute au premier nouveau titre ; sur l'ampli, s'il était arrêté, il saute à l'ancienne taille de la file). Elle demande le serveur (`Server.Online`), affiche le résultat dans la barre de message et ne tourne jamais deux fois en même temps (`suggesting`).

**Avec l'ampli, c'est le serveur qui s'en charge** (`suggest.rs`) : l'instantané porte `auto_suggest` et un `reason` sur chaque titre ajouté par le serveur ; `maybeAutoSuggest` ne fait donc rien quand `auto_suggest` est présent, la file marque les titres d'après `reason`, et l'interrupteur des réglages suit le réglage du serveur (`followServerSetting`) et l'écrit (`POST /player/auto_suggest`), commun à tous les téléphones. Le reste de ce paragraphe décrit ce que l'appli fait elle-même : sur le lecteur du téléphone, et avec un serveur sans `auto_suggest`.

**Retirer un titre** : chaque ligne de la file de `NowPlayingScreen` est un `SwipeToDismissBox` (dans les deux sens, désactivé pour le titre en cours) ; un glissement appelle `AppViewModel.removeFromQueue(index, trackId)` (`POST /player/remove` avec l'ampli, `removeMediaItem` sur le téléphone). Le serveur refuse si le titre à `index` n'est plus `trackId` ; la clé de la ligne contient `queueRevision`, incrémenté en cas d'échec pour que la ligne glissée revienne.

**Défilement sans fin** : `NowPlayingScreen` surveille sa `LazyColumn` (`snapshotFlow` sur le dernier élément visible, seulement une fois que l'utilisateur a défilé) ; près de la fin de la file, elle appelle `AppViewModel.loadMoreSuggestions()`, qui en demande 10 de plus (`POST /player/suggest_more` avec l'ampli, `suggestSimilar` à partir du dernier titre de la file sur le téléphone), affiche une ligne « Recherche de suggestions… » pendant l'attente, ne fait rien si le réglage est coupé ou si le serveur n'est pas `Online`, et ne demande qu'une fois par état de la file (`moreKey` = taille + dernier titre : une réponse vide ne boucle pas, et un lot dont les titres ne sont pas encore arrivés n'est pas demandé deux fois).

Suggestions automatiques (`autoSimilar`, activées par défaut, dans les réglages) : un collecteur de l'état du lecteur l'appelle (`maybeAutoSuggest`) **juste après le début d'une file** (aucun titre de la file n'est connu comme suggéré) : 10 titres proches du dernier de la file sont ajoutés ; et de nouveau quand le dernier titre de la file démarre, pour qu'elle ne s'épuise jamais. Chaque tentative n'est faite qu'une fois (`autoDoneKey` : rien n'est retenté quand rien n'a été trouvé). Les titres suggérés sont retenus (`suggested` : id -> raison, gardés dans les préférences, 400 au plus) et la file du lecteur plein écran les marque : une étoile à la place du numéro, la raison (Last.fm, même genre, même artiste) et un titre « Suggestions » avant le premier. C'est le serveur qui décide d'où viennent les suggestions ; l'appli indique seulement dans les réglages que le serveur peut interroger Last.fm.

## Icône

Icône adaptative en vecteurs (`res/drawable/ic_launcher_*.xml`, `res/mipmap-anydpi-v26/`) : une framboise (20 grains et trois feuilles vert foncé, le Raspberry Pi) sur fond noir, et devant une note de musique dorée en forme de J (J comme jukebox : barre du haut, hampe, la crosse finit en tête de note) cernée de noir pour rester lisible sur le rouge. La framboise est dans la couche d'arrière-plan, réduite pour tenir dans la fenêtre visible de 72 dp ; le J reste dans la zone de sécurité de 66 dp. Une couche `monochrome` est fournie pour les icônes thématiques d'Android 13+ (le J en plein, la framboise en forme pâle). Piège rencontré : la tête de note et la hampe doivent être des tracés séparés, sinon leurs sens de dessin opposés s'annulent là où elles se recouvrent (trou visible).

## Pochettes

Coil 3 (`AsyncImage`) charge `GET /cover/{id}?size=` avec 64 px (listes), 300 px (grille), 600 px (page album, lecteur). **On ne demande jamais une pochette quand `has_cover` est faux** : le serveur répondrait `404`. Le ViewModel retient `has_cover` de chaque album vu (`remember`) pour que le mini-lecteur sache si l'album en cours en a une ; inconnu = on tente, avec l'icône de note comme repli en cas d'erreur.

## Langues

Tous les textes sont des ressources : `res/values/strings.xml` est en anglais (repli pour toute autre langue), `values-fr`, `-de`, `-it`, `-es`, `-pt` (+ `-pt-rBR` pour les différences), `-nl`, `-ca`, `-sv`, `-da`, `-nb`, `-fi`, `-pl` sont les traductions. L'appli suit la langue du téléphone ; `res/xml/locales_config.xml` (`android:localeConfig`) les liste pour le réglage de langue par appli d'Android 13+, et `androidResources.localeFilters` empêche les bibliothèques d'ajouter des textes dans d'autres langues (l'appli serait à moitié traduite pour ces utilisateurs). Compose utilise `stringResource` / `pluralStringResource` ; le view model et le worker de téléchargement utilisent `getString`. Ce qui n'est pas traduit volontairement : les valeurs de protocole (`status`, les raisons comme `same_genre` sont transformées en libellé à l'affichage), le nom du produit, la marque `Denon`, et les messages d'erreur venus du serveur ou du système. Les tailles passent par `Formatter.formatShortFileSize` ; la date du nom automatique d'une playlist par `DateFormat.getBestDateTimePattern`. Un échec que le code doit reconnaître est un type (`NotEnoughSpace`), jamais un message.

## Listes

Grille d'albums : 60 albums par page, page suivante demandée quand l'un des 12 derniers éléments devient visible. Le tri « au hasard » n'a pas de pagination (bouton pour mélanger). Recherche : 150 ms d'attente après la dernière frappe, un appel `GET /search` ensuite (le serveur répond en 10 à 17 ms).

## Ce qui n'est volontairement pas fait

- Pas de bibliothèque d'injection, de base de données locale ni de bibliothèque de navigation : pas nécessaires à cette taille (les albums téléchargés sont des fichiers + un `album.json`).
- Pas de téléchargement automatique ni de playlists : on télécharge album par album.
- Hors connexion, seul le contenu téléchargé est disponible : les listes du serveur ne sont pas mises en cache.
