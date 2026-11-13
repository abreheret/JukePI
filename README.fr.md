# JukePI

🇬🇧 [English](README.md)

[![Server](https://github.com/abreheret/JukePI/actions/workflows/server.yml/badge.svg)](https://github.com/abreheret/JukePI/actions/workflows/server.yml) [![Android](https://github.com/abreheret/JukePI/actions/workflows/android.yml/badge.svg)](https://github.com/abreheret/JukePI/actions/workflows/android.yml) [![Release](https://img.shields.io/github/v/release/abreheret/JukePI)](https://github.com/abreheret/JukePI/releases/latest) [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Serveur de musique auto-hébergé pour Raspberry Pi, avec une télécommande Android qui pilote un ampli Denon HEOS ou joue hors connexion sur le téléphone.**

Votre bibliothèque MP3 vit sur un Raspberry Pi. Le Pi l'indexe et la sert en HTTP ; un ampli **Denon HEOS** télécharge lui-même les titres et les joue, pendant qu'une **appli Android** affiche vos albums (pochettes, recherche, artistes) et commande l'ampli. En dehors de chez vous, la même appli joue des **albums téléchargés sur le téléphone**.

```
 Appli Android ──HTTP/JSON──▶  Raspberry Pi : serveur JukePI (Rust)  ──HEOS CLI──▶  ampli Denon HEOS
   │  (télécommande, et lecteur                 │  index SQLite + fichiers MP3                 │
   │   à elle quand elle est hors connexion)    └────────── GET /stream/{id}.mp3 ◀─────────────┘
   └─ albums téléchargés sur le téléphone
```

## Aperçu

<table><tr><td align="center"><img src="docs/screenshots/albums.png?v=4" width="190" alt="Albums"><br><sub>Albums</sub></td><td align="center"><img src="docs/screenshots/album.png?v=2" width="190" alt="Album"><br><sub>Album</sub></td><td align="center"><img src="docs/screenshots/search.png?v=2" width="190" alt="Recherche"><br><sub>Recherche</sub></td><td align="center"><img src="docs/screenshots/player.png?v=2" width="190" alt="Lecture en cours"><br><sub>Lecture en cours</sub></td><td align="center"><img src="docs/screenshots/downloaded.png?v=2" width="190" alt="Téléchargés"><br><sub>Téléchargés</sub></td></tr></table>

Captures de l'appli Android. Les pochettes appartiennent à leurs ayants droit et ne sont montrées qu'à titre d'illustration.

## Contenu du dépôt

| Dossier | Quoi | Langage |
| --- | --- | --- |
| [`music-server/`](music-server/README.fr.md) | Le serveur : scan + index, API, streaming, pochettes, recherche plein texte, client HEOS, lecteur, mDNS | Rust (`axum`, `tokio`, SQLite) |
| [`android-app/`](android-app/README.fr.md) | L'appli Android : parcourir, piloter l'ampli, jouer sur le téléphone, télécharger des albums, mode hors connexion | Kotlin, Jetpack Compose, Media3 |

Chaque dossier a son README, un document d'architecture et des notes de développement. La documentation est en **anglais par défaut** et aussi en **français** (`*.fr.md`).

## Points forts

- **Aucun audio ne passe par le téléphone** quand on joue sur l'ampli : l'ampli va chercher les MP3 sur le Pi. Le téléphone est une télécommande.
- Indexe des dizaines de milliers de MP3 en quelques minutes, se remet à jour en quelques secondes ; gère les tags brouillons, les compilations, les pochettes manquantes.
- Pochettes du dossier de l'album ou intégrées aux fichiers, miniatures en cache ; recherche en ligne facultative (désactivée par défaut).
- Recherche plein texte insensible aux accents, en environ 15 ms, tolérante aux fautes de frappe (« beatels » trouve The Beatles), une seule entrée par artiste quelle que soit l'orthographe.
- Le serveur s'annonce en mDNS : l'appli le trouve sans saisir d'adresse.
- Déplacement, coupure du son, volume et file d'attente pour l'ampli (HEOS n'a pas de commande de déplacement : le serveur relance le flux à la trame voulue).
- **« Même style »** : dès que tu lances un album, quelques titres de ta bibliothèque qui y ressemblent (similarité Last.fm si elle est configurée, sinon tes genres, en favorisant les artistes que tu écoutes le plus) s'ajoutent à la suite dans la file, repérés pour que tu puisses passer ceux que tu ne veux pas ; d'autres suivent quand la file se termine. Ce que joue l'ampli peut être **scrobblé** sur ton compte Last.fm.
- Android : commandes de l'écran verrouillé et de la notification, touches de volume qui pilotent l'ampli, **téléchargements** avec reprise, **mode hors connexion** avec lecture locale, sortie commutable (ampli / téléphone) qui emporte la file avec elle.
- Développé pour **Android 17**, y compris sa nouvelle permission de réseau local.

## Pré-requis

- Un Raspberry Pi (testé sur un Pi 4, 4 Go, Raspberry Pi OS / Debian 13) avec la bibliothèque MP3 sur un disque.
- Un ampli Denon (ou autre) parlant le protocole **HEOS CLI** sur le même réseau. Développé et testé sur un **Denon DRA-900H** ; les autres appareils HEOS ne sont pas testés.
- Un téléphone Android, Android 14 ou plus récent pour compiler tel quel (`minSdk 34`, développé sur Android 17).
- Pour compiler : Rust (+ `zig` et `cargo-zigbuild` pour la compilation croisée vers le Pi), un JDK 21 et le SDK Android.

## Démarrage rapide

1. **Serveur** : voir [`music-server/README.fr.md`](music-server/README.fr.md) et [`music-server/docs/OPERATIONS.fr.md`](music-server/docs/OPERATIONS.fr.md).
   ```bash
   cd music-server
   cp deploy/music-server.env.example deploy/music-server.env   # git ignore ce fichier : vos réglages et secrets vont ici
   $EDITOR deploy/music-server.env                              # MUSIC_PUBLIC_URL (adresse du Pi vue par l'ampli), MUSIC_ROOT, Last.fm...
   PI=pi@raspberrypi.local ./deploy/deploy.sh                   # tests, compilation, envoi, installation, redémarrage
   ```
   **Facultatif, Last.fm** (« même style » et scrobbling) : créer une application sur <https://www.last.fm/api/account/create>, mettre sa clé et son « shared secret » dans `deploy/music-server.env` (`LASTFM_API_KEY`, `LASTFM_SECRET`), redéployer, puis lier son compte une fois : `MUSIC_SERVER=<ip-du-pi>:8765 ./scripts/lastfm-link.sh`.
2. **Appli** : voir [`android-app/README.fr.md`](android-app/README.fr.md) : `./android-app/install.sh` avec un téléphone branché en USB.
3. Ouvrir l'appli : elle trouve le serveur toute seule (ou saisir son adresse dans les réglages), accorder la permission de réseau local, et jouer.

## État

Fonctionne et sert au quotidien sur l'installation de l'auteur (une bibliothèque de 43 794 titres, un Pi 4 et un DRA-900H). Certains chemins n'ont pas été exercés de bout en bout ; la feuille de route de chaque projet les liste. Les contributions et retours d'autres installations sont bienvenus.

## Vie privée et sécurité

- L'API n'a **aucune authentification** : à n'exécuter que sur un réseau local de confiance.
- La recherche de pochettes en ligne facultative (`COVERS_ONLINE=1`) envoie des noms d'artistes et d'albums à MusicBrainz et au Cover Art Archive. Elle est désactivée par défaut.
- Les fonctions Last.fm facultatives (`LASTFM_API_KEY`, `LASTFM_SECRET`) envoient à Last.fm l'artiste et le titre en cours, et avec le scrobbling la liste de ce que joue l'ampli. Elles sont inactives sans clé ; les secrets sont dans l'unité systemd du Pi, jamais dans le dépôt. Voir [OPERATIONS](music-server/docs/OPERATIONS.fr.md#activer-lastfm-vie-privée).
- Les adresses IP de la documentation (`192.168.1.50`, `192.168.1.60`) sont des exemples. Rien dans le dépôt ne contient d'identifiants.

## Licence et marques

Publié sous [licence MIT](LICENSE). *Denon* et *HEOS* sont des marques de leurs propriétaires ; ce projet est indépendant et n'est ni affilié à eux ni approuvé par eux.
