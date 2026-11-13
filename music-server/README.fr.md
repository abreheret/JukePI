# Serveur JukePI (`music-server`)

🇬🇧 [English](README.md)

[![Server](https://github.com/abreheret/JukePI/actions/workflows/server.yml/badge.svg)](https://github.com/abreheret/JukePI/actions/workflows/server.yml)

Serveur de musique pour Raspberry Pi qui indexe une bibliothèque MP3 et pilote un ampli **Denon HEOS** via le protocole **HEOS CLI** (développé et testé sur un Denon DRA-900H). L'ampli va chercher lui-même les MP3 en HTTP sur le Pi : aucun audio ne passe par le téléphone.

C'est la moitié serveur de JukePI ; la télécommande Android est dans [`../android-app`](../android-app/README.fr.md).

## État

| Brique | État |
| --- | --- |
| Scan + index SQLite (testé sur 43 794 titres / 3 339 albums) | fait, testé sur une vraie bibliothèque |
| API albums / artistes / pistes / stream (Range) | fait |
| Client HEOS (connexion, SSDP, événements, reconnexion) | fait, testé sur un vrai Denon |
| Lecteur (file d'attente, enchaînement auto, pause, suivant, volume) | fait, testé sur un vrai Denon |
| Service systemd | fait |
| Pochettes (fichier du dossier, image intégrée, miniatures en cache) | fait, testé |
| Déplacement dans un titre (`/player/seek`) et coupure du son (`/player/mute`) | fait, testé sur un vrai Denon |
| Annonce mDNS (`_music-server._tcp`) pour que l'appli trouve le serveur | fait, vérifié |
| Recherche plein texte (titres, albums, artistes ; accents, préfixes) | fait, testée (10 à 17 ms) |
| Pochettes en ligne (MusicBrainz / Cover Art Archive) | fait, **désactivé par défaut**, testé avec un album public |
| Recommandations (« même style ») : similarité Last.fm + genres locaux, pondérées par ton historique d'écoute | fait, testé sur la vraie bibliothèque ; l'enchaînement automatique en fin d'album n'est pas encore vérifié par une écoute complète |
| Scrobbling de ce que joue l'ampli vers Last.fm | fait, compte lié ; premier scrobble pas encore vérifié sur le profil |

## Architecture en une image

```
 Téléphone ──HTTP/JSON──▶  Pi : music-server (Rust, port 8765)
                              │  SQLite (index)          │ HEOS CLI (TCP 1255)
                              │  bibliothèque (disque)   ▼
                              │                       ampli Denon HEOS
                              └──── GET /stream/{id}.mp3 ◀──┘  (l'ampli télécharge le MP3)
```

Détails : [docs/ARCHITECTURE.md](docs/ARCHITECTURE.fr.md).

## Configuration (variables d'environnement)

| Variable | Défaut | Rôle |
| --- | --- | --- |
| `MUSIC_ROOT` | `/mnt/music/music` | Racine de la bibliothèque (lecture seule suffit) |
| `MUSIC_DB` | `~/.local/share/music-server/index.db` | Base SQLite (sur la carte SD : le SSD est monté en lecture seule) |
| `MUSIC_BIND` | `0.0.0.0:8765` | Adresse d'écoute |
| `MUSIC_PUBLIC_URL` | `http://192.168.1.50:8765` | **À régler sur l'adresse de votre Pi.** URL de base que l'**ampli** utilise pour joindre le Pi (IP fixe, jamais auto-détectée : `hostname -I` liste aussi les ponts Docker) |
| `HEOS_HOST` | (vide : découverte SSDP) | IP fixe de l'ampli si la découverte ne marche pas |
| `HEOS_PLAYER_NAME` | (vide : premier lecteur) | Sous-chaîne du nom du lecteur HEOS à piloter |
| `MUSIC_CACHE_DIR` | `~/.cache/music-server` | Cache des miniatures (`covers/<taille>/`) et des pochettes téléchargées (`covers/online/`) |
| `COVERS_ONLINE` | (absent) | `1` pour activer la recherche de pochettes en ligne. **Envoie les noms d'artistes et d'albums sans pochette locale à musicbrainz.org et coverartarchive.org** |
| `MUSICBRAINZ_CONTACT` | (vide) | Adresse ou URL de contact ajoutée au User-Agent (MusicBrainz la demande ; à renseigner avec votre adresse) |
| `COVERS_ONLINE_DELAY_S` | `60` | Délai avant la première recherche en ligne |
| `LASTFM_API_KEY` | (vide) | Active la similarité Last.fm. **Envoie à ws.audioscrobbler.com l'artiste et le titre du morceau pour lequel on demande une suggestion.** À garder dans un fragment systemd, jamais dans le dépôt |
| `LASTFM_SECRET` | (vide) | « Shared secret » de ton application Last.fm : nécessaire (avec un compte lié) pour scrobbler. Mêmes précautions que la clé |
| `LASTFM_USER` | (vide) | Compte dont l'historique d'écoute pondère les suggestions quand aucun compte n'est lié ; une fois un compte lié, son nom est utilisé |
| `MUSIC_MDNS` | `1` | `0` pour ne pas annoncer le serveur en mDNS |
| `MUSIC_MDNS_NAME` | `Music Server` | Nom de l'instance affiché dans l'appli |
| `MUSIC_NO_SCAN` | (absent) | Si défini, pas de scan au démarrage |
| `RUST_LOG` | `info,lofty=error` | Niveau de logs (lofty est très bavard) |

Les adresses IP de ce dépôt (`192.168.1.50` pour le Pi, `192.168.1.60` pour l'ampli) sont des **exemples** : utilisez les vôtres.

## API

Toutes les réponses sont en JSON. Pas d'authentification : réseau local de confiance uniquement.

### Bibliothèque

| Requête | Réponse |
| --- | --- |
| `GET /health` | `{"status":"ok"}` |
| `GET /albums?limit=50&offset=0&sort=name\|artist\|recent\|random&q=texte&artist=nom` | `{total, limit, offset, items:[{id,name,artist,various_artists,year,track_count,has_cover,cover_path}]}` |
| `GET /albums/{id}` | `{album, tracks:[{id,title,artist,genre,track_no,disc_no,year,duration_ms,stream_path}]}` |
| `GET /artists` | `{total, items:[{name, album_count}]}` (hors compilations) |
| `GET /tracks/{id}` | une piste |
| `GET /search?q=texte&limit=10&types=albums,artists,tracks` | recherche plein texte : `{query, albums, artists, tracks}`. Insensible aux accents et à la casse, chaque mot est un préfixe (`bea hel` trouve *The Beatles – Help!*), classement par pertinence (le titre pèse plus que l'artiste, qui pèse plus que l'album). `limit` par rubrique : 1 à 50 (défaut 10). Les pistes portent en plus `album_name` |
| `GET /cover/{album_id}?size=300` | miniature JPEG de l'album. `size` ∈ {64, 150, 300, 600} (défaut 300) ; ratio conservé, jamais agrandie ; `ETag` + `Cache-Control: max-age=86400` (`304` sur `If-None-Match`) ; `404` si l'album n'a pas de pochette |
| `GET /covers` | statistiques : albums par origine de pochette (`file`, `embedded`, `online`, `none`) |
| `GET /stream/{id}.mp3` | le fichier MP3 : `audio/mpeg`, `Accept-Ranges: bytes`, `206` sur `Range` |
| `GET /seek/{id}/{ms}.mp3` | le même titre **à partir de l'instant `ms`** (la réponse commence à la trame MP3 la plus proche, au plus 26 ms après). Sert au déplacement dans un titre ; pas de `Range` |
| `GET /scan` | état du dernier scan |
| `POST /scan` | relance un scan (`202`, ou `409` si déjà en cours) |

Les titres sont servis **par identifiant numérique** : toutes les URL données à l'ampli sont en ASCII pur (voir pourquoi dans [docs/HEOS-FINDINGS.md](docs/HEOS-FINDINGS.fr.md), section 3). Chacune se termine par un `?n=<ms>` jetable, et le flux commence par un tag ID3 construit depuis l'index : l'ampli affiche ainsi titre, artiste et album même pour les fichiers sans tags (voir [docs/ARCHITECTURE.md](docs/ARCHITECTURE.fr.md#ce-que-lampli-affiche)).

### Recommandations et Last.fm

| Requête | Effet |
| --- | --- |
| `GET /recommendations?track_id=N&limit=20&exclude=1,2` | titres de la bibliothèque qui ressemblent au titre `N` : `{seed_track_id, source: "lastfm"\|"local", lastfm_enabled, taste, items}`. Chaque élément est un titre plus `reason` (`lastfm_track`, `lastfm_artist`, `same_genre`, `same_artist`). `exclude` : ids déjà en file. `limit` 1 à 100 (20 par défaut). N'échoue jamais à cause de Last.fm : sans clé, il répond d'après les tags locaux |
| `GET /lastfm` | `{similarity, scrobbling, user, needs_secret, auth_pending}`. Ne contient jamais de clé, de secret ni de session |
| `POST /lastfm/auth/start` | lier un compte, étape 1 : renvoie `{url}` à ouvrir dans un navigateur pour autoriser. Demande `LASTFM_API_KEY` et `LASTFM_SECRET` |
| `POST /lastfm/auth/finish` | étape 2, une fois autorisé : enregistre la session, le scrobbling démarre |
| `DELETE /lastfm/session` | délier le compte (`204`) |

La construction des suggestions est décrite dans [docs/ARCHITECTURE.md](docs/ARCHITECTURE.fr.md#recommandations-et-lastfm) ; la mise en place de Last.fm dans [docs/OPERATIONS.md](docs/OPERATIONS.fr.md#activer-lastfm-vie-privée). Comme le reste de l'API, `/lastfm/*` n'a **aucune authentification** : quiconque est sur le réseau peut délier le compte.

### Lecteur (ampli)

| Requête | Effet |
| --- | --- |
| `GET /player` | instantané : `status` (`stopped\|loading\|playing\|paused`), `index`, `current`, `position_ms`, `duration_ms`, `volume`, `mute`, `queue`, `error`, `heos_connected`, `updated_at_ms` |
| `GET /player/events` | flux SSE : l'instantané complet maintenant, puis à chaque changement. La position n'est rafraîchie que toutes les ~5 s : le client extrapole depuis `updated_at_ms` |
| `POST /player/play` | corps `{"album_id":N}` ou `{"track_id":N}` ou `{"track_ids":[...],"start":0}` : remplace la file et lance la lecture |
| `POST /player/next`, `/previous`, `/pause`, `/resume`, `/stop` | commandes (réponse `202` + instantané) |
| `POST /player/jump` | corps `{"index":N}` |
| `POST /player/enqueue` | corps `{"track_ids":[...]}` : ajoute à la fin de la file sans toucher au titre en cours (`400` si la file est vide : utiliser `/player/play`) |
| `POST /player/volume` | corps `{"level":0-100}` |
| `POST /player/mute` | corps `{"mute":true\|false}` (le volume est conservé) |
| `POST /player/seek` | corps `{"position_ms":N}` (position absolue) ou `{"delta_ms":±N}` (relatif à la position actuelle). **Environ 1 s de silence** : le flux est relancé à l'endroit voulu |

Les commandes sont asynchrones : la réponse `202` contient l'état *avant* que le Denon ait réagi. Les erreurs arrivent dans le champ `error` de l'instantané.

Exemple :

```bash
curl -s -X POST http://192.168.1.50:8765/player/play -H 'Content-Type: application/json' -d '{"album_id":131}'
curl -s http://192.168.1.50:8765/player | python3 -m json.tool
curl -N http://192.168.1.50:8765/player/events      # flux temps réel
curl -s -X POST http://192.168.1.50:8765/player/volume -H 'Content-Type: application/json' -d '{"level":20}'
```

## Développer

```bash
cargo test                 # parseur d'attributs HEOS, faux Denon TCP, détection de fin de titre
cargo run                  # en local ; MUSIC_ROOT=/chemin/de/test MUSIC_DB=/tmp/idx.db MUSIC_BIND=127.0.0.1:18765
PI=pi@raspberrypi.local ./deploy/deploy.sh   # compile pour le Pi, envoie, redémarre le service
```

Pré-requis de la compilation croisée : `brew install zig`, `cargo install cargo-zigbuild --locked`, `rustup target add aarch64-unknown-linux-gnu`.

## Documentation

| Fichier | Contenu |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.fr.md) | Modules, flux de données, schéma SQLite, décisions et leurs raisons |
| [docs/HEOS-FINDINGS.md](docs/HEOS-FINDINGS.fr.md) | Tout ce qui a été mesuré sur le vrai Denon (avec chiffres) |
| [docs/ANDROID-CLIENT.md](docs/ANDROID-CLIENT.fr.md) | Guide pour écrire l'appli : découverte mDNS, contrat d'API, flux temps réel, pochettes |
| [docs/OPERATIONS.md](docs/OPERATIONS.fr.md) | Le Pi, le SSD, le service, le déploiement, le dépannage, les pièges |
| [docs/NOTES.fr.md](docs/NOTES.fr.md) | Notes de développement : décisions clés, enseignements, feuille de route |
| `experiments/` | Scripts Python qui ont produit les mesures (reproductibles) |
| `scripts/test_player.py` | Test de bout en bout du lecteur sur un vrai ampli |
