# Architecture

🇬🇧 [English](ARCHITECTURE.md)

## Vue d'ensemble

Un seul binaire Rust (`axum` + `tokio`), lancé par systemd sur le Raspberry Pi. Il fait quatre choses :

1. **Indexer** la bibliothèque MP3 dans SQLite (`scan.rs`, `db.rs`).
2. **Servir** l'index en JSON et les MP3 en HTTP avec requêtes `Range` (`api.rs`).
3. **Parler au Denon** en HEOS CLI sur TCP (`heos.rs`).
4. **Piloter la lecture** : file d'attente, enchaînement des titres, état en temps réel (`player.rs`).

Le pilotage est côté Pi et pas côté téléphone, pour que la lecture continue quand le téléphone se met en veille.

```
 appli / curl ──▶ api.rs ──▶ player.rs (acteur) ──▶ heos.rs ──TCP 1255──▶ Denon
                    │              ▲                    │
                    │              └── événements ◀─────┘ (state_changed, progress, error…)
                    ▼
                 SQLite ◀── scan.rs ◀── /mnt/music/music (SSD NTFS, lecture seule)

 Denon ──GET /stream/{id}.mp3──▶ api.rs ──ServeFile (Range)──▶ fichier MP3
```

## Modules

| Fichier | Rôle | Points à retenir |
| --- | --- | --- |
| `main.rs` | Config par variables d'environnement, démarre scan, client HEOS, lecteur, serveur HTTP | L'API répond tout de suite, le scan tourne en arrière-plan |
| `db.rs` | Ouvre SQLite (WAL, `synchronous=NORMAL`), crée le schéma | `open` pour le scan, `connect` (léger) pour chaque requête API |
| `id3.rs` | Construit le tag ID3v2.3 (titre, artiste, album ; UTF-16 hors ASCII simple) placé devant les flux donnés à l'ampli | Voir « Ce que l'ampli affiche » |
| `mp3seek.rs` | Trouve l'octet où commence la trame MP3 d'un instant donné | Parcourt les trames une à une (CBR et VBR) |
| `mdns.rs` | Annonce le serveur sur le réseau local (mDNS / DNS-SD) | Une seule interface, celle de `MUSIC_PUBLIC_URL` |
| `fuzzy.rs` | Rapprochement tolérant aux fautes (distance d'édition, accents ignorés, mots collés) | Repli de la recherche, voir plus bas |
| `search.rs` | Index plein texte (SQLite FTS5) | Reconstruit après un scan qui a changé quelque chose |
| `covers.rs` | Trouve la source de pochette de chaque album, fabrique et met en cache les miniatures | Voir la section « Pochettes » |
| `online.rs` | Recherche optionnelle de pochettes en ligne | Désactivée par défaut |
| `lastfm.rs` | Client Last.fm : appels signés, liaison du compte, fichier de session | Secrets jamais journalisés ; fichier de session en mode 600 |
| `recommend.rs` | « Même style » : similarité Last.fm + tags locaux, pondérés par l'historique d'écoute | Classement pur, testé unitairement |
| `scrobble.rs` | Envoie à Last.fm ce que joue l'ampli | Inactif tant qu'il manque la clé, le secret ou un compte lié |
| `scan.rs` | Parcourt le disque, lit les tags (`lofty`), met l'index à jour | Incrémental sur (chemin, taille, mtime) ; voir ci-dessous |
| `api.rs` | Routes HTTP, erreurs JSON, flux SSE | Les accès SQLite passent par `spawn_blocking` |
| `heos.rs` | Connexion TCP au Denon, commandes, événements, SSDP | Une tâche possède la socket ; reconnexion automatique |
| `player.rs` | Acteur unique qui possède la file et l'état de lecture | Évite toute course entre commandes et événements |

## Index SQLite

Deux tables (`db.rs`).

- `albums(id, dir UNIQUE, dir_name, name, artist, artist_count, year, track_count, added_at)`
- `tracks(id, path UNIQUE, album_id, title, artist, album_tag, genre, track_no, disc_no, year, duration_ms, bitrate_kbps, size, mtime)`

Choix et raisons :

- **Un album = un dossier** (`Artiste/Album/`). Les tags ne servent qu'à l'affichage. Grouper par tag éclate les compilations (« 80's Hits ») en dizaines de faux albums. Le nom affiché est le tag album majoritaire du dossier, sinon le nom du dossier.
- **Les identifiants de pistes sont stables** : `INSERT … ON CONFLICT(path) DO UPDATE … RETURNING id` conserve l'`id` d'une piste modifiée. Les URL `/stream/{id}.mp3` et les files d'attente restent valides après un rescan.
- **Chemins relatifs à `MUSIC_ROOT`**, pour pouvoir déplacer la bibliothèque.
- **Artiste de repli** : si aucun tag artiste, l'album prend le premier dossier du chemin. Sans cela, 6 143 titres sur 43 794 (14 %) n'auraient pas d'artiste (classique, BO…).
- **Compilation** : `artist_count > 3` donne `various_artists: true` dans l'API (130 albums).
- La base est sur la **carte SD** (`~/.local/share/music-server/`) car le SSD est monté en lecture seule.

### Scan (`scan.rs`)

- Marche avec `walkdir`, ne garde que les `.mp3` (insensible à la casse). Les `.mood`, `.ini`, `.nfo`, `.m3u`, `.cue`, `.db`… sont ignorés.
- **Premier scan : 157 s** pour 43 794 fichiers (~280/s à travers FUSE/NTFS). **Rescan : 2 à 4 s.**
- Insertions par lots de 1 000 dans une transaction, sinon SQLite sur carte SD rame.
- **Fichiers aux tags illisibles** (175 au premier passage : frames ID3 invalides, UTF-16 tronqué…) : le fichier est quand même indexé, titre = nom de fichier. On relit alors seulement les propriétés audio (durée) sans les tags. Les fichiers sans durée sont retentés à chaque scan. Il en reste 3 (probablement tronqués).
- **Garde-fou de suppression** : si le parcours ne trouve aucun MP3 alors que l'index en contient, rien n'est supprimé et le scan finit en erreur. Un disque démonté ne doit jamais vider la base.
- Le scan tourne dans un thread dédié ; `GET /scan` donne les compteurs (`files_seen`, `indexed`, `unchanged`, `tag_errors`, `removed`, `duration_s`, `error`).

## Déplacement dans un titre (`mp3seek.rs`, `player.rs`)

HEOS n'a **aucune commande de déplacement** et le Denon ne sait pas avancer dans une URL qu'il lit (mesuré). Le serveur le fait donc à sa place : il donne au Denon une URL dont le contenu **commence à l'instant voulu**, `GET /seek/{id}/{ms}.mp3`, et relance la lecture dessus.

- **Repérage** (`mp3seek::find_offset`) : saute l'étiquette ID3v2, puis lit les en-têtes de trames Layer III (4 octets) en sautant de trame en trame (`seek_relative` dans un tampon de 256 Ko) en additionnant leurs durées, jusqu'à la première trame qui commence à `ms` ou après. Fonctionne pour le **CBR et le VBR** sans se fier à une table Xing absente ou fausse ; resynchronise octet par octet sur des déchets ; refuse (`400`) un fichier sans trame Layer III. Mesuré : exact à 10–20 ms près en CBR et à moins de 0,1 s en VBR, décodage sans erreur (`ffmpeg`). Le tout premier cadre après la coupe peut produire un défaut de quelques millisecondes (réservoir de bits).
- **Réponse** : `200`, `Content-Type: audio/mpeg`, `Content-Length` exact, `Accept-Ranges: none`, corps lu en flux depuis l'octet trouvé. Pas d'étiquette ID3 (le Denon n'en affiche rien d'utile), pas de `Range`. URL en ASCII pur, sans `?` ni `=` (le Denon ne décode rien dans l'URL).
- **Lecteur** : `Cmd::Seek(ms)` / `Cmd::SeekBy(delta)` passent par `start_at(index, offset, keep_duration)` : même séquence que pour un changement de titre (arrêt, vidage de la file, `play_stream`), donc **~1 s de silence**, et les faux `stop` sont ignorés par `transition`. Le Denon compte sa position **depuis le début du flux coupé** : le serveur garde `offset_ms` et affiche `offset_ms + cur_pos`. La durée envoyée par le Denon pour un flux coupé est ignorée (c'est celle du morceau restant) : on garde la durée totale connue. Un déplacement vise au plus `durée − 1,5 s` (au-delà, on passerait au titre suivant) ; moins d'une seconde du début = flux normal. La détection de fin de titre fonctionne telle quelle (position estimée = décalage + position du Denon) : testé, un saut à 3:16 d'un titre de 3:23 enchaîne sur le titre suivant.
- Se déplacer pendant une pause relance la lecture.

## Coupure du son

`POST /player/mute {"mute": true|false}` → HEOS `player/set_mute?state=on|off`. L'état vient aussi de l'événement `player_volume_changed` (attribut `mute`) et de `player/get_mute` à la connexion ; il est publié dans l'instantané (`mute`). Le volume n'est pas modifié : rétablir le son revient au niveau précédent.

## Recherche (`search.rs`)

Trois tables virtuelles FTS5 (`fts_tracks`, `fts_albums`, `fts_artists`), tokeniseur `unicode61 remove_diacritics 2` (insensible aux accents et à la casse) et index de préfixes de 2 et 3 lettres (la saisie au fil de l'eau reste rapide). Ce sont des **données dérivées** de `tracks` et `albums` : elles sont reconstruites entièrement (3 s pour 43 794 titres sur le Pi) quand un scan a ajouté, modifié ou supprimé quelque chose, ou quand leur nombre de lignes ne correspond pas à l'index. Un rescan sans changement ne les touche pas.

Requête : le texte de l'utilisateur est découpé en mots (caractères alphanumériques seulement, 8 mots au maximum), chaque mot devient un terme préfixe entre guillemets et tous doivent correspondre (`"bea"* "hel"*`). Comme seuls des caractères alphanumériques survivent, aucune syntaxe FTS5 ne peut être injectée. Classement `bm25` avec pondération : titre 10, artiste 4, album 2 pour les pistes ; nom 10, artiste 5, dossier 3 pour les albums.

**Un artiste, une entrée** : `albums.artist_key` est le nom de l'artiste sans casse, accents, ponctuation ni « The » initial (`recommend::artist_key`) : « Queen »/« QUEEN » et « AC/DC »/« Ac-Dc »/« ACDC » ne font qu'un. `/artists` et la recherche montrent l'orthographe la plus utilisée avec les albums de toutes, et `/albums?artist=` filtre sur la clé. La colonne est remplie au démarrage et après chaque scan.

**Fautes de frappe** (`fuzzy.rs`) : quand la recherche plein texte trouve moins de 3 résultats (ou aucun artiste), les résultats proches s'ajoutent après les exacts. Chaque mot de la requête doit être à une distance d'édition (insertions, suppressions, substitutions, lettres échangées) de 1 pour 4 à 6 lettres et 2 à partir de 7 lettres, les mots courts doivent être exacts, le dernier mot peut être inachevé, et les mots d'un nom comptent aussi collés (« acdc »). Les artistes se comparent en environ une milliseconde ; les albums et titres demandent un passage sur l'index (environ une demi-seconde pour 43 794 titres sur un Pi), donc ils ne sont parcourus que quand presque rien n'a été trouvé. La réponse porte `fuzzy: true` pour que l'appli puisse dire « résultats approchants ».

Les artistes viennent de `albums.artist` (artiste majoritaire du dossier), hors compilations ; un artiste qui n'apparaît que sur des titres de compilation se trouve par ses titres.

## mDNS (`mdns.rs`)

Le serveur s'annonce comme service DNS-SD **`_music-server._tcp`**, instance « Music Server », avec les enregistrements TXT `version`, `api=1` et `path=/`. Crate `mdns-sd` (en Rust pur, dans le binaire : rien à installer sur le Pi).

- **Une seule interface** : celle qui porte l'adresse de `MUSIC_PUBLIC_URL` (192.168.1.50). Le Pi a aussi des ponts Docker (172.17/172.18) qui ne doivent jamais être annoncés. L'hôte de `MUSIC_PUBLIC_URL` doit donc être une adresse IP.
- **Nom d'hôte distinct** (`raspberrypi-music.local.`) : Avahi, déjà actif sur Raspberry Pi OS, possède `raspberrypi.local`. Annoncer le même nom provoquerait un conflit.
- À l'arrêt (`SIGINT`/`SIGTERM` via systemd), le serveur envoie les paquets d'adieu : le service disparaît tout de suite chez les clients (vérifié avec `dns-sd`, `Rmv`).
- Une panne mDNS ne doit pas empêcher le serveur de démarrer : l'erreur est journalisée (« mDNS disabled »).

## Pochettes

**Trouver la source** (`covers::refresh`, appelé à la fin de chaque scan). Priorité :

1. **Image dans le dossier de l'album.** On note chaque fichier `jpg/jpeg/png/webp` : `cover` (100), `folder` (95), `front` (92), `albumart` (90), `album` (85) ; les noms contenant ces mots comptent un peu moins ; `large` ajoute 5 points ; `small`, `thumb`, `mini` ou `tiny` retirent 60 ; `back`, `rear`, `inlay`, `inside`, `tray`, `cd`, `disc`, `booklet`, `label`, `insert` ou `spine` retirent 50 ; un fichier de moins de 4 Ko retire 30 ; la taille ajoute jusqu'à 10 points. Cela couvre les restes de Windows Media Player : `AlbumArt_{GUID}_Large.jpg` gagne contre `AlbumArtSmall.jpg`. Les fichiers cachés (`._x.jpg`) sont ignorés.
2. **Dossier de disque** (`CD1`, `Disc 2`, `Disk_3`, `DVD1`) : si rien n'est trouvé, on regarde le dossier parent (`Artiste/Album/CD1/…` avec la pochette dans `Artiste/Album/`).
3. **Image intégrée** dans l'un des 3 premiers MP3 de l'album (la pochette « avant » en priorité, sinon la première image).
4. **Rien** (`none`). Éventuellement complété par `online.rs`.

Le résultat est stocké dans `album_covers` (`kind`, `source`, taille et date de la source). Un album n'est **résolu à nouveau que s'il a changé** : date du dernier fichier audio (`tracks_stamp`), date du dossier (`dir_mtime`), ou source disparue. Un scan sans changement coûte un `stat` par dossier d'album.

**Miniatures** (`GET /cover/{id}?size=`) : générées à la demande puis mises en cache sur la carte SD, `covers/<taille>/<album>-<clé>.jpg`. La clé est un hachage FNV-1a de (origine, source, date, taille de la source, taille demandée) : si l'image change, la clé change et l'ancienne miniature n'est plus servie. Décodage avec la crate `image` (JPEG, PNG, WebP seulement), redimensionnement `CatmullRom` en conservant le ratio, jamais d'agrandissement, ré-encodage JPEG qualité 82. Garde-fous : 30 Mo maximum par image source, 12 000 px maximum par côté, 512 Mo d'allocation maximum.

Concurrence : **2 générations à la fois** (un Pi qui décode de gros fichiers ne doit pas affamer les autres services) et un verrou par miniature, pour qu'une grille qui demande la même pochette deux fois n'effectue le travail qu'une seule fois.

Échecs : une image que le décodeur refuse fait passer l'album en `none` (le client cesse de la réclamer). Une erreur d'accès au disque (SSD absent) ne change **rien** : elle est transitoire.

**Pochettes en ligne** (`online.rs`, `COVERS_ONLINE=1` seulement). Pour les albums `none` ayant un artiste (hors compilations), toutes les 6 h et au plus une fois par album et par mois : recherche MusicBrainz (`release:"…" AND artist:"…"`), on n'accepte que `score ≥ 95` **et** un titre identique après normalisation (minuscules, sans ponctuation ni parenthèses), puis téléchargement de `front-500` depuis le Cover Art Archive : **d'abord le `release-group`** (pochette de référence), ensuite chaque édition. Constat à l'essai : sans cela, *OK Computer* tombait sur une édition japonaise. Une requête MusicBrainz au plus toutes les 1,1 s. L'image est vérifiée (décodage) avant d'être enregistrée dans `covers/online/<album>.jpg`. Un échec réseau interrompt la passe sans marquer les albums comme essayés.

## Streaming

`GET /stream/{id}.mp3` cherche le chemin par `id` puis délègue à `tower_http::services::ServeFile`, qui gère `Range` (`206`), `HEAD`, `Content-Length`, `Last-Modified` et `audio/mpeg`. `Range` est indispensable : la bibliothèque contient des mixes continus jusqu'à 189 Mo.

## Client HEOS (`heos.rs`)

- Une tâche possède le `TcpStream`. Les commandes arrivent par un canal, **une seule en vol à la fois** (HEOS répond dans l'ordre, éventuellement après une réponse intermédiaire `command under process` qu'on ignore).
- Les lignes `event/...` partent sur un canal `broadcast` (`player_state_changed`, `player_now_playing_progress`, `player_playback_error`, `player_volume_changed`…). Le client ajoute deux événements synthétiques : `connected` et `disconnected`.
- **Reconnexion** automatique (2 s, doublée jusqu'à 15 s). Pendant une coupure, `command()` échoue tout de suite (« HEOS not connected ») ; les requêtes en file d'attente à ce moment sont refusées au lieu d'être rejouées plus tard.
- **Découverte** : SSDP (`M-SEARCH`, cible `urn:schemas-denon-com:device:ACT-Denon:1`), sinon `HEOS_HOST`. Vérifié depuis le Pi : le Denon est trouvé en moins de 2 s.
- Délai maximal par commande : 10 s.
- Les valeurs d'attributs ne sont pas échappées : nos URL sont en ASCII pur (`http://192.168.1.50:8765/stream/123.mp3`). Voir HEOS-FINDINGS section 3 pour le piège d'encodage.

## Lecteur (`player.rs`)

Un **acteur** (une tâche, un état) reçoit les commandes de l'API et les événements HEOS dans un seul `select!`. Pas de verrou, pas de course.

Machine d'états : `stopped → loading → playing ⇄ paused`.

Lancer un titre (`start_track`) : `set_play_state stop` → `clear_queue` → `play_stream`. Pendant cette séquence, `transition` est actif et **tous les `stop` sont ignorés** (le Denon en émet un faux à chaque `play_stream`). `transition` retombe quand le Denon dit `state=play`, ou après 12 s (erreur « le Denon n'a jamais démarré »).

Fin de titre = un `stop` que nous n'avons pas provoqué, avec la position estimée à moins de 3 s de la durée (`is_natural_end`). La position est extrapolée entre deux événements de progression (toutes les ~5 s). Dans ce cas on enchaîne sur le titre suivant. Sinon (arrêt depuis l'appli HEOS par exemple), on passe en `stopped` sans enchaîner.

Erreurs de lecture : le Denon répète la même erreur plusieurs fois. On la traite une fois par fenêtre de 3 s, on saute au titre suivant, et après 3 erreurs d'affilée on arrête avec le message d'erreur.

Position et durée : la durée vient de l'événement de progression du Denon (la plus fiable). L'estimation `lofty` (basée sur le débit) sert avant le premier événement.

## Recommandations et Last.fm

`GET /recommendations?track_id=N` (`recommend.rs`) répond à « qu'y a-t-il dans MA bibliothèque qui ressemble à ce titre ? ».

1. **Point de départ** : l'artiste et le titre du morceau `N`.
2. **Last.fm** (seulement avec `LASTFM_API_KEY`) : `track.getSimilar` (titres proches, jusqu'à 100) et `artist.getSimilar` (artistes proches, jusqu'à 60) en parallèle, plus `user.getTopArtists` pour le compte (lié, ou `LASTFM_USER`). Les réponses sont mises en cache dans la table `lastfm_cache` : 7 jours pour la similarité, 1 jour pour l'historique. Un échec de Last.fm est journalisé et donne des listes vides, jamais une erreur.
3. **Rapprochement** : un passage sur la bibliothèque construit `artiste normalisé -> titres`. Les noms sont comparés sans casse, sans accents ni ponctuation, en ignorant un « The » initial, `&` = « and » et les parties entre parenthèses des titres (`(Remastered 2009)`). Ce que la bibliothèque ne possède pas est écarté.
4. **Classement** : un titre exact proche vaut `1 + match`, un titre d'un artiste proche vaut `match`. Les goûts de l'utilisateur multiplient : `1 + 0,6 * racine(écoutes / écoutes de l'artiste le plus écouté)`. Au plus 2 titres par artiste, pas de doublon, rien de ce qui est déjà en file, jamais le titre de départ ni son artiste.
5. **Repli local** quand la liste est courte (pas de clé, artiste inconnu) : même genre (artistes les plus écoutés d'abord), puis autres titres de l'artiste de départ (5 au plus). L'ordre est déterministe pour un titre donné.

**Le serveur prolonge la file tout seul** (`suggest.rs`, ampli seulement) : une tâche surveille les instantanés du lecteur et une fonction pure `wanted` décide. Une file sans suggestion reçoit un lot de 10 juste après son démarrage ; quand l'avant-dernier ou le dernier titre joue, un autre lot suit (graine : le dernier titre de la file), si bien que la musique ne s'arrête jamais, même sans appli ouverte. Les titres ajoutés portent un `reason` dans l'instantané : c'est ainsi que tous les clients savent ce qui est une suggestion. Un lot à la fois (`busy`) ; un déclencheur qui n'a rien trouvé n'est pas réessayé tant que la file ne change pas ; les titres déjà en file et les 300 derniers joués ne sont jamais reproposés ; rien n'est ajouté tant que 200 titres ou plus attendent ; une file qui atteint 1000 oublie les titres joués il y a longtemps (50 sont gardés). Le réglage `auto_suggest` (`POST /player/auto_suggest`) est commun à tous les clients et enregistré dans un fichier à côté de l'index. `POST /player/suggest_more` lance la même extension pour une appli dont l'utilisateur a fait défiler la file jusqu'au bout. Sur le téléphone, l'appli ajoute elle-même les lots (`addMediaItems`).

**Scrobbling** (`scrobble.rs`, ampli seulement) observe les instantanés du lecteur. Un `Tracker` pur décide : « en cours d'écoute » au début de la lecture d'un titre ; scrobble après la moitié du titre (4 minutes au plus), seulement s'il dure plus de 30 s ; pauses et trous ne comptent pas ; l'horodatage est le début du titre. Les écoutes sont d'abord écrites dans la table `scrobble_queue`, puis envoyées par lots de 50 ; un échec les laisse en attente pour l'essai suivant (toutes les 5 minutes, abandon après 14 jours, limite de Last.fm).

**Lier un compte** (`lastfm.rs`) : `POST /lastfm/auth/start` demande un jeton à Last.fm et renvoie la page d'autorisation ; une fois « Autoriser » cliqué, `POST /lastfm/auth/finish` échange le jeton contre une clé de session, enregistrée dans `lastfm_session.json` à côté de l'index (mode 600). Les appels sont signés par `md5(paramètres triés + secret partagé)`. Le mot de passe Last.fm n'atteint jamais le serveur.

## Ce que l'ampli affiche

Le Denon affiche le titre, l'artiste et l'album qu'il trouve dans le tag ID3 au début du flux (détails et mesures dans [HEOS-FINDINGS.md](HEOS-FINDINGS.fr.md), section 2). Beaucoup de fichiers n'ont pas de tag utilisable : les flux donnés à l'ampli (`/stream/{id}.mp3?n=<ms>` et `/seek/...`) sont donc construits par `api.rs::tagged_audio` : **un tag neuf tiré de l'index, puis l'audio** du fichier, en sautant le tag propre du fichier et sa trame d'en-tête Xing/Info/VBRI, **avec une trame de silence (26 ms) insérée environ une demi-seconde plus loin** (`mp3seek::bitrate_nudge`) : le Denon ne lit le tag que lorsqu'il voit les en-têtes de trames changer, ce qu'un fichier à débit constant ne fait jamais. La trame a un autre débit et le bit CRC inversé, et se place là où elle n'abîme aucun son ; les 96 premiers Ko de l'audio sont lus pour la placer. Sans elle, tout fichier CBR affichait `Url Stream`. Un titre qui n'est que le nom du fichier (« 01. Come Together - The Beatles ») est affiché sans son numéro ni l'artiste (`id3::display_title`) ; un vrai titre de tag n'est jamais touché. Les requêtes du téléphone (sans `?n=`) reçoivent le fichier intact, avec `Range`.

Supprimer la trame d'en-tête fait perdre la durée exacte à l'ampli. Le scan la lit donc dans l'en-tête (`tracks.duration_exact` : 1 = exacte, 2 = pas d'en-tête, estimation, 0 = pas encore vérifiée) et `player.rs` lui fait confiance plutôt qu'à celle de l'ampli. Une passe unique en arrière-plan (`scan::repair_durations`) corrige les titres indexés avant l'existence de la colonne (environ 2,5 minutes pour 43 794 titres).

## Ce qui n'est volontairement pas fait

- Pas de file native HEOS ni de DLNA : mesure faite, le plan B n'est pas nécessaire (HEOS-FINDINGS section 4).
- Pas de pré-génération des miniatures : elles sont créées au premier affichage (à ajouter si la première ouverture de la grille est lente sur le Pi).
- Pas de purge du cache des miniatures orphelines (une pochette qui change laisse l'ancienne miniature, quelques dizaines de Ko).
- Pas d'authentification sur l'API : réseau local seulement (y compris `/lastfm/*` : quiconque est sur le réseau peut délier le compte).
- Le scrobbling ne couvre pas le lecteur du téléphone (l'appli ne remonte pas ses écoutes au serveur).
- Pas de surveillance du disque (`notify`) : `POST /scan` ou redémarrage du service suffisent pour l'instant.
- Pas de lecture sans blanc entre deux titres : ~0,3 à 0,5 s, c'est la limite de `play_stream`.
- Pas de déplacement sans silence : ~1 s (arrêt, vidage de la file, nouvelle URL, téléchargement par le Denon).
