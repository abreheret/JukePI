# Exploitation

🇬🇧 [English](OPERATIONS.md)

Ce guide décrit la **configuration de référence** sur laquelle JukePI a été développé ; adaptez les adresses, disques et utilisateurs à votre installation. Les adresses IP sont des exemples.

## Le Raspberry Pi (configuration de référence)

| Élément | Valeur |
| --- | --- |
| Modèle | Raspberry Pi 4 Model B Rev 1.1, 4 Go, 4 cœurs |
| OS | Debian 13 (trixie), noyau `6.12.75+rpt-rpi-v8`, aarch64, glibc 2.41 |
| Adresse | 192.168.1.50 (exemple ; à réserver en DHCP sur la box : l'URL donnée à l'ampli en dépend) |
| Accès | SSH (idéalement par clé : `ssh-copy-id`) ; le mot de passe de l'utilisateur reste nécessaire pour `sudo` |
| Alimentation | chargeur officiel Raspberry Pi 5,1 V / 3 A. `vcgencmd get_throttled` = `0x0` y compris pendant un scan complet |
| Autres services | d'autres services peuvent tourner sur le même Pi ; le serveur musique est léger (pas de transcodage) |
| Charge au repos | load average ~0,2 ; 3 Go de RAM disponibles |

### Ports

Le serveur musique utilise **8765**. Le port 8080 est souvent déjà pris (interfaces web d'autres services) : ne pas l'utiliser. Vérifier avec `ss -ltn`.

### Disques

| Disque | Montage | Contenu |
| --- | --- | --- |
| Carte microSD 29 Go (ext4) | `/` | système, binaire, base SQLite |
| un autre disque USB (ext4) | `/mnt/usb` | autres données |
| **SSD 1 To dans un boîtier USB 3, NTFS** (label `SSD`, UUID `XXXXXXXXXXXXXXXX`: remplacez par le vôtre, `lsblk -f`) | `/mnt/music` | bibliothèque musique, dossier `music/` (~290 Go) |

Le SSD est en NTFS pour pouvoir être branché sur un PC Windows et recevoir les MP3 directement (Windows n'écrit pas en ext4). Le Pi le monte en **lecture seule** avec `ntfs-3g`. Ligne ajoutée à `/etc/fstab` :

```
UUID=XXXXXXXXXXXXXXXX  /mnt/music  ntfs-3g  ro,uid=1000,gid=1000,umask=022,nofail,x-systemd.device-timeout=10  0  0
```

`nofail` : le Pi démarre même sans le SSD. Sauvegarde de l'ancien fichier : `/etc/fstab.bak-music`. Le montage a été testé avec `umount` + `mount -a`, **pas avec un vrai redémarrage**.

**Ajouter de la musique** : brancher le SSD sur Windows, copier dans `music/`, **« Éjecter » proprement** (sinon le volume NTFS est marqué sale et le montage peut échouer ou passer en lecture seule), rebrancher sur le Pi, puis `curl -X POST http://192.168.1.50:8765/scan`.

### Budget USB

Un Pi 4 limite l'ensemble des ports USB à **1,2 A**, quel que soit le chargeur. Avec un disque 2,5" alimenté par le bus et le SSD, `get_throttled` est resté à `0x0`, aucune erreur I/O dans `dmesg`. À recontrôler si un troisième périphérique USB est ajouté (`vcgencmd get_throttled`, `dmesg | grep -iE "under-voltage|usb"`).

### Cache

`~/.cache/music-server/covers/` sur la carte SD : miniatures (`64/`, `150/`, `300/`, `600/`) et pochettes téléchargées (`online/`). Ordre de grandeur : 20 à 60 Ko par miniature de 300 px, donc ~100 à 200 Mo pour 3 339 albums si toute la grille a été affichée. Sans danger à vider : tout se régénère (`rm -rf ~/.cache/music-server/covers/{64,150,300,600}`). Ne pas supprimer `online/` sans raison : les pochettes téléchargées n'existent nulle part ailleurs.

## Le service systemd

Fichier source : `deploy/music-server.service`, installé dans `/etc/systemd/system/music-server.service`.

- Utilisateur `pi`, `Nice=10` (laisse la priorité aux autres services), `Restart=on-failure`, `RestartSec=5`.
- `After=` et `Wants=mnt-music.mount` : le service démarre après le montage du SSD, **mais démarre quand même si le SSD est absent** (le scan finit alors en erreur « music root … is not a directory », l'API répond, les `/stream` échouent). Volontairement pas de `RequiresMountsFor`.
- Binaire : `/home/pi/music-server/music-server`. Base : `/home/pi/.local/share/music-server/index.db`.

```bash
systemctl status music-server
sudo systemctl restart music-server
journalctl -u music-server -f                  # logs en direct
journalctl -u music-server --since "-10min" -o cat
curl -s localhost:8765/health ; curl -s localhost:8765/scan
```

## Découverte mDNS

Vérifier depuis un Mac : `dns-sd -B _music-server._tcp local.` (liste) puis `dns-sd -L "Music Server" _music-server._tcp local.` (hôte, port, TXT). Résultat attendu : `raspberrypi-music.local.:8765`, `version=0.1.0 api=1 path=/`. Le multicast (UDP 5353) doit passer entre le téléphone et le Pi : même réseau local, pas de réseau « invité » isolé. Avahi (déjà présent) et le serveur cohabitent sur le port 5353 sans conflit.

## Activer les pochettes en ligne (vie privée)

Désactivé par défaut. L'activer envoie à `musicbrainz.org` et `coverartarchive.org` (qui redirige vers `archive.org`) le nom d'artiste et le nom d'album de **chaque album sans pochette locale**, ainsi que l'IP du Pi. Pour l'activer, ajouter dans `deploy/music-server.service` : `Environment=COVERS_ONLINE=1` (et, si l'on veut être poli avec MusicBrainz, `Environment=MUSICBRAINZ_CONTACT=<adresse ou URL>`), puis redéployer. Avant d'activer : `curl localhost:8765/covers` donne le nombre d'albums `none` qui seraient concernés.

## Activer Last.fm (vie privée)

Tout est inactif sans clé. Ce que chaque niveau envoie à Last.fm (`ws.audioscrobbler.com`) :

| Réglage | Active | Envoyé à Last.fm |
| --- | --- | --- |
| `LASTFM_API_KEY` | « même style » | l'artiste et le titre du morceau pour lequel on demande une suggestion ; le nom du compte pour l'historique d'écoute s'il y en a un |
| + `LASTFM_SECRET` et un compte lié | scrobbling | tout ce que joue **l'ampli**, avec l'heure (le lecteur du téléphone n'est pas couvert) |

1. Créer une application sur <https://www.last.fm/api/account/create> (n'importe quel nom). Elle donne une **clé d'API** et un **shared secret**.
2. Les mettre dans `deploy/music-server.env` (copier `deploy/music-server.env.example` ; git l'ignore) et déployer : `deploy.sh` installe le fichier sur le Pi sous `/etc/music-server/music-server.env`, propriétaire root, mode 600. Ne pas les mettre dans l'unité ni dans le dépôt. (À la main : un fragment `/etc/systemd/system/music-server.service.d/x.conf` avec `[Service]` / `Environment=LASTFM_API_KEY=...`, mode 600.)
3. Lier son compte une fois, pour le scrobbling : `MUSIC_SERVER=<ip-du-pi>:8765 ./scripts/lastfm-link.sh`. Il affiche une page Last.fm : se connecter, cliquer « Autoriser l'accès », revenir et appuyer sur Entrée. Le mot de passe n'est saisi que sur last.fm. La clé de session est stockée dans `~/.local/share/music-server/lastfm_session.json` (mode 600) sur le Pi. `./scripts/lastfm-link.sh unlink` (ou `DELETE /lastfm/session`) l'oublie.
4. Vérifier : `curl localhost:8765/lastfm` donne `{"similarity":true,"scrobbling":true,"user":"<vous>",...}` (aucun secret dedans), et `journalctl -u music-server | grep -i last.fm` montre `Last.fm ... enabled`. Après une écoute : `Last.fm: 1 scrobble(s) sent`, et le titre apparaît dans vos écoutes récentes.
5. Un profil Last.fm est public par défaut, et l'historique que construisent les scrobbles aussi : à changer dans les réglages de confidentialité de Last.fm si vous ne le voulez pas.

`/lastfm/*` n'a aucune authentification, comme le reste de l'API : gardez le serveur sur un réseau de confiance. Si un secret a fuité (collé quelque part de public), créez une nouvelle application sur Last.fm, remplacez les valeurs et redéployez.

## Déployer une nouvelle version

Depuis votre ordinateur, dans `music-server/` : `PI=pi@raspberrypi.local ./deploy/deploy.sh` (tests, compilation croisée, envoi, redémarrage ; demande le mot de passe `sudo`). Si `deploy/music-server.env` existe, il est installé sur le Pi sous `/etc/music-server/music-server.env` (root seul) et prend le pas sur les lignes `Environment=` de l'unité ; sans lui, le Pi garde ses réglages. L'unité systemd n'est installée que si elle n'existe pas encore sur le Pi (`FORCE_UNIT=1` pour l'écraser) : elle contient vos réglages. Le binaire est compilé avec `cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.36` (le `.2.36` fixe la version minimale de la glibc, le Pi a la 2.41). Les mêmes étapes ont été exécutées à la main pendant le développement ; le script lui-même n'a pas été exécuté de bout en bout. La saisie du mot de passe `sudo` est interactive : lancer le script **dans un vrai terminal**.

Après le premier déploiement qui apporte les pochettes, le scan résout tous les albums (un `read_dir` par dossier, plus la lecture de quelques MP3 pour les images intégrées) : prévoir quelques minutes, suivre avec `curl localhost:8765/covers` (`resolved_last_scan`).

## Dépannage

| Symptôme | Cause probable | Action |
| --- | --- | --- |
| `GET /player` : `heos_connected: false` | ampli éteint / autre réseau / SSDP bloqué | Mettre `Environment=HEOS_HOST=192.168.1.60` dans l'unité systemd ; vérifier les logs `HEOS discovery failed` |
| `error: HEOS not connected` à la lecture | idem, coupure Wi-Fi de l'ampli | Le client se reconnecte seul (2 à 15 s) |
| Le Denon dit `state=unknown`, journal du serveur : `404` | URL mal encodée (double encodage) ou id inexistant | Voir HEOS-FINDINGS section 3 |
| Rafale d'erreurs `Unsupported format` à la fin d'un titre | File interne de l'ampli avec d'anciennes URL | Le lecteur vide la file avant chaque titre ; si cela vient d'un test manuel, envoyer `clear_queue` |
| `/scan` : `error: music root … is not a directory` | SSD non monté | `lsblk`, `sudo mount /mnt/music` ; si NTFS « sale » : le rebrancher sur Windows et l'éjecter proprement |
| Coupures de musique | Wi-Fi du Denon | Brancher l'ampli en Ethernet (recommandation du fabricant) |
| Disque qui se déconnecte | Budget USB / alimentation | `vcgencmd get_throttled` ; hub USB alimenté pour le 2,5" |
| `has_cover: false` partout après le déploiement | le scan de pochettes n'a pas fini, ou `MUSIC_ROOT` illisible | `curl localhost:8765/covers` ; `journalctl -u music-server | grep covers` |
| Une pochette est mauvaise | mauvais fichier choisi dans le dossier | Renommer l'image voulue en `cover.jpg` dans le dossier de l'album, puis `POST /scan` (la date du dossier change, l'album est résolu à nouveau) |
| Les suggestions ne viennent que des tags locaux (`source: "local"`) | pas de `LASTFM_API_KEY`, artiste inconnu de Last.fm, ou Last.fm injoignable | `curl localhost:8765/lastfm` ; `journalctl -u music-server \| grep -i last.fm` |
| Les écoutes n'apparaissent pas sur Last.fm | aucun compte lié (`scrobbling:false`), titre de moins de 30 s ou écouté moins de la moitié, Pi hors ligne | `./scripts/lastfm-link.sh` ; les écoutes en attente partent d'elles-mêmes (table `scrobble_queue`) |
| L'appli ne voit pas le serveur | multicast bloqué (réseau invité, répéteur Wi-Fi), ou `MUSIC_PUBLIC_URL` pas en IP | Depuis un Mac : `dns-sd -B _music-server._tcp local.` ; journal : `mDNS: announcing` ou `mDNS disabled` ; en dernier recours saisir `http://192.168.1.50:8765` à la main |
| La recherche ne trouve rien alors que la bibliothèque est indexée | index FTS vide ou ancien | Il se reconstruit au démarrage si son nombre de lignes ne correspond pas ; `journalctl -u music-server \| grep "search index"` |
| Port déjà utilisé au démarrage | autre service sur 8765 | `ss -ltn` ; changer `MUSIC_BIND` |

## Pièges rencontrés (à ne pas refaire)

1. **`pkill -f motif` par SSH tue sa propre session** : le motif apparaît dans la ligne de commande du shell distant (code de sortie 255, aucune sortie). Tuer par PID : `for p in $(pgrep -x music-server); do kill $p; done`.
2. **Ne jamais envoyer du contenu dans `sudo tee` via un wrapper qui injecte le mot de passe sur stdin.** Un wrapper qui fournit le mot de passe de `sudo` sur l'entrée standard consomme ce stdin : `echo ligne | wrapper tee -a /etc/fstab` a écrit le **mot de passe** dans `/etc/fstab` à la place de la ligne (erreur d'analyse du fichier). Corrigé aussitôt depuis la sauvegarde. Méthode sûre : écrire un fichier dans `/tmp`, puis `sudo sh -c "cat /tmp/x > /etc/..."`, puis vérifier avec `grep`.
3. **Port 8080** : souvent déjà pris par d'autres services. Ne pas l'utiliser.
4. **`hostname -I` renvoie aussi les ponts Docker** (172.17.0.1, 172.18.0.1) : l'URL de base donnée à l'ampli est dans `MUSIC_PUBLIC_URL`, jamais détectée automatiquement.
5. **NTFS sous Windows** : toujours « Éjecter » avant de débrancher le SSD.
6. **`lofty` écrit ~30 000 lignes de WARN** (« Using bitrate to estimate duration ») à un scan complet. Filtré par défaut (`lofty=error`).
