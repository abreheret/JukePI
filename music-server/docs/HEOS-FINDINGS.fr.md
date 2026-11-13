# Ce qui a été mesuré sur le Denon DRA-900H

🇬🇧 [English](HEOS-FINDINGS.md)

Tout ce document vient de tests faits sur le vrai appareil les 2026-11-11 et 2026-11-13. Les scripts qui ont produit les chiffres sont dans `../experiments/`. La spec officielle (HEOS CLI v1.16, 2020, sans support de Sound United) ne couvre pas le DRA-900H : ces mesures sont la seule source fiable.

## 1. L'appareil

| Élément | Valeur |
| --- | --- |
| Modèle | Denon DRA-900H |
| Firmware | 3.139.173 |
| IP | 192.168.1.60 (bail DHCP) |
| Réseau | **Wi-Fi** (`"network": "wifi"`) ; le manuel Denon recommande le filaire pour lire depuis un serveur |
| pid HEOS | 123456789 |
| Port HEOS CLI | TCP 1255, commandes `heos://groupe/commande?attr=valeur` terminées par `\r\n`, réponses JSON |
| Découverte | SSDP `M-SEARCH`, `ST: urn:schemas-denon-com:device:ACT-Denon:1`. La réponse porte `LOCATION: http://192.168.1.60:60006/...` |
| Compte HEOS | inutile pour la musique locale |
| Volume au repos | 25 |

## 2. `play_stream` fonctionne (étape 1 du plan, validée)

`heos://browse/play_stream?pid=X&url=http://192.168.1.50:8765/test.mp3` avec un MP3 servi par un simple `python3 -m http.server` :

- Réponse `success`, puis `state=play`.
- Le journal du serveur montre un `GET` venant de `192.168.1.60` : **c'est bien le Denon qui télécharge le fichier** (un seul `GET`, sans `HEAD` préalable).
- Le serveur doit répondre `Content-Type: audio/mpeg`.
- Confirmé à l'oreille sur deux fichiers (un MP3 de test 192 kbps de 8,9 Mo et un MP3 128 kbps de 5,6 Mo).
- **Délai entre l'envoi de `play_stream` et `state=play` : 0,43 à 0,55 s.**
- **Ce que le Denon affiche** (titre, album, artiste dans `get_now_playing_media`, et sur sa façade) vient du **tag ID3 tout au début du flux**, lu 3 à 5 secondes après le début du titre (`Url Stream` d'ici là). Ce ne sont pas des métadonnées ICY/Shoutcast (le Denon n'envoie pas d'en-tête `Icy-MetaData` ; son User-Agent est `GStreamer curlhttpsrc`), et cela ne dépend ni d'`Accept-Ranges`, ni de `Last-Modified`, ni de HTTP/1.0 contre 1.1. `image_url` est **toujours vide** : pas de pochette sur l'ampli (il faudrait un serveur multimédia DLNA/UPnP).
- Beaucoup de fichiers n'ont **aucun tag utilisable** (un fichier encodé par Lavf n'avait que le nom de l'encodeur) : l'ampli affichait donc `Url Stream`. Le serveur commence donc chaque flux par un tag ID3v2.3 construit depuis son index (`id3.rs`) : titre, artiste, album tels que l'appli les montre, le tag propre du fichier étant sauté.
- **Une trame d'en-tête Xing / Info / VBRI juste après le tag empêche l'affichage du tag** (même fichier : `Url Stream` pendant 19 s avec la trame, tags au bout de quelques secondes sans elle, et quand le flux démarre plus loin dans le fichier). Le Denon n'en a pas besoin pour jouer : le flux la saute (`mp3seek::audio_start`).
- Sans cette trame, le Denon ne peut qu'**estimer la durée** (315 s pour un titre de 259 s), ce qui fausserait la détection de fin de titre. La durée exacte est lue dans l'en-tête lui-même au scan (colonne `duration_exact`) et le lecteur lui fait confiance plutôt qu'à celle du Denon. Ce même en-tête a montré que l'estimation de lofty était fausse pour ces fichiers (elle lisait 32 kbps dans la trame d'en-tête : 36 minutes pour une chanson de 4 minutes).
- Le Denon semble aussi **retenir ce qu'il a trouvé par URL** (une URL jouée une fois sans tags continuait à afficher `Url Stream` ; avec une chaîne de requête, les tags apparaissaient) : chaque démarrage utilise donc un `?n=<ms>` neuf.
- Les très petits fichiers (un extrait de 13 s, 325 Ko) ne sont pas fiables : le Denon les télécharge d'un coup et affiche souvent `Url Stream` quand même. Les titres normaux (3 à 12 Mo) sont fiables.
- La durée donnée par le Denon (`duration=18000`) est plus fiable que l'estimation de `lofty` (18442) pour les MP3 sans en-tête VBR.

## 3. Encodage des URL : encoder UNE seule fois

Test avec `Artist/Album (2016)/07. Artist - Très haut.mp3` (accent, espaces, parenthèses) :

| URL envoyée dans `play_stream` | Ce que le Denon demande au Pi | Résultat |
| --- | --- | --- |
| `.../Album%20%282016%29/07.%20Artist%20-%20Tr%C3%A8s%20haut.mp3` (encodée une fois) | idem | `200`, `state=play` |
| `.../Album%2520%25282016%2529/...Tr%25C3%25A8s...` (doublement encodée, comme la spec le suggère pour `%`) | idem, `%2520` tel quel | `404`, `state=unknown` |

Le Denon ne décode pas un niveau. **Règle :** envoyer l'URL telle qu'un navigateur la demanderait.

Le serveur évite le problème en servant par identifiant : `http://192.168.1.50:8765/stream/1768.mp3`. Aucun caractère à encoder.

## 4. Enchaînement de deux titres : l'écart mesuré

Test avec deux clips de 20 s (`experiments/chaining_gap.py`). Les temps viennent des événements HEOS (précision ±0,2 s) : l'écart est mesuré entre la fin estimée du titre A et l'événement `state=play` du titre B.

| Méthode | Écart | Erreurs |
| --- | --- | --- |
| Naïve : envoyer le suivant à réception de `state=stop` | **~0,3 s** | 0 |
| Anticipée : envoyer le suivant 0,55 s avant la fin | ~0,5 s | 0 |
| Anticipée : envoyer le suivant 1 s avant la fin | ~0,55 s | 0 |

Conclusions :

- **Pas de lecture sans blanc** avec `play_stream`, mais l'écart est petit.
- **L'envoi anticipé n'apporte rien** : `play_stream` pendant une lecture coupe le titre en cours puis charge le suivant, il n'y a pas de préchargement.
- La méthode naïve suffit. C'est ce qu'implémente `player.rs`.
- Le plan B du document d'origine (serveur DLNA et file native HEOS) n'est pas nécessaire. Il pourrait réduire l'écart, sans preuve. À reconsidérer seulement si le blanc gêne sur des albums enchaînés (live, concepts).
- Mesuré aussi avec le vrai serveur sur 3 titres courts : enchaînement détecté à `pos 14132 ms / 15000 ms` et `12906 / 13000`, passage `loading → playing` en ~0,4 s.

## 5. File d'attente interne du Denon et `clear_queue`

Le Denon garde une file d'attente interne alimentée par chaque `play_stream`, et il y **avance tout seul** à la fin d'un titre. Constats :

- Avant d'avoir vidé la file, après la fin d'un clip il a **redemandé d'anciennes URL de tests précédents** (`/old1.mp3`, `/old2.mp3`, devenues des `404`) en rafale toutes les ~0,5 s. Les événements correspondants : `player_playback_error` avec `Playback error. Unsupported format.`, `Could not decode the audio stream`, `Unable to play media. Please try again later.`
- **`clear_queue` envoyé pendant une lecture arrête le titre** : `state=stop` arrive en ~0,1 s (mesuré).
- `clear_queue` sur une file déjà vide répond `result: fail`, `eid=4&text=Requested data not available`. Sans conséquence.
- **Chaque `play_stream` émet d'abord un faux `state=stop`** (+0,14 s) puis `state=play` (+0,45 s), plus `player_queue_changed` et `player_now_playing_changed`.
- Après `clear_queue` puis deux `play_stream`, `get_queue` renvoie `count=1`.

Conséquences dans `player.rs` :

1. Séquence pour lancer un titre : `stop` → `clear_queue` → `play_stream`. On ne vide jamais en cours de lecture.
2. Tous les `stop` sont ignorés jusqu'au `state=play` qui suit notre propre commande.
3. Les erreurs de lecture répétées sont dédupliquées sur 3 s.

## 6. Événements utiles

Activés par `heos://system/register_for_change_events?enable=on` (réponse immédiate).

| Événement | Attributs | Usage |
| --- | --- | --- |
| `player_state_changed` | `pid`, `state` = `play\|pause\|stop` | machine d'états |
| `player_now_playing_progress` | `pid`, `cur_pos`, `duration` (en **millisecondes**) | **toutes les ~5 s** (positions 2000, 7000, 12000, 17000…) ; première valeur 0 au démarrage |
| `player_now_playing_changed` | `pid` | ignoré |
| `player_queue_changed` | `pid` | ignoré |
| `player_playback_error` | `pid`, `error` | saut au titre suivant |
| `player_volume_changed` | `pid`, `level`, `mute` | affichage du volume |

Fin de titre naturelle : `state=stop` sans `playback_error`, sans commande de notre part. Pas d'événement dédié « fin de titre ».

**Pas de commande de seek** dans HEOS CLI, et rien dans la spec pour avancer dans un flux. Contournement mis en œuvre côté serveur : relancer `play_stream` sur une URL qui commence à l'instant voulu (`/seek/{id}/{ms}.mp3`). Mesuré le 2026-11-13 : le Denon accepte ces flux coupés **sans en-tête Xing ni `Range`**, reporte sa position depuis le début du flux coupé (d'où `offset_ms` côté serveur) et reporte la durée du flux coupé ; la reprise du son demande **~1 s** (commande reçue → premier `GET` du Denon environ 0,9 s plus tard, ce qui inclut `stop`, `clear_queue` et `play_stream`).

## 7. Autres commandes vérifiées

| Commande | Résultat |
| --- | --- |
| `player/get_players` | `name`, `pid`, `model`, `version`, `ip`, `network`, `serial` |
| `player/get_play_state` | `state=play\|pause\|stop` (ou `unknown` après une erreur) |
| `player/get_now_playing_media` | `type`, `song`, `album`, `artist`, `image_url` (vide), `qid`, `sid` (1024 pour les URL) |
| `player/set_play_state` | `play`, `pause`, `stop` |
| `player/get_volume` | `level` |
| `player/get_queue` | `returned`, `count` |

## 8. Mute

`player/set_mute?pid=X&state=on|off` : `success`, effet immédiat (testé : activé puis désactivé, l'état est relu par `player_volume_changed`/`get_mute`). `player/get_mute` renvoie `state=on|off`. Le volume reste inchangé.

## 9. Pas encore testé

- Lecture d'un mix continu de 189 Mo en entier (le `Range` est vérifié, pas la lecture longue).
- Comportement après un redémarrage du Denon pendant la lecture.
- Passage du Denon en Ethernet (écart d'enchaînement attendu plus stable).
