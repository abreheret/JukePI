# Guide pour l'appli Android

🇬🇧 [English](ANDROID-CLIENT.md)

Ce que l'appli doit savoir du serveur. Le contrat complet des endpoints est dans le [README](../README.fr.md) ; ici, la découverte, les conventions et les pièges.

## 1. Trouver le serveur (mDNS)

Le serveur s'annonce en DNS-SD : type **`_music-server._tcp`**, instance « Music Server », TXT `version`, `api` (=`1`), `path` (=`/`).

```kotlin
val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
nsd.discoverServices("_music-server._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
// onServiceFound -> nsd.resolveService(info, resolveListener)
// onServiceResolved(info) -> http://${info.host.hostAddress}:${info.port}
```

- Utiliser l'**adresse IP** résolue (`info.host`), pas le nom `raspberrypi-music.local` : Android ne résout pas toujours les noms `.local` en dehors de NSD.
- **Android 17 (API 37)** : pour une appli qui cible l'API 37, `ACCESS_LOCAL_NETWORK` est obligatoire (déclarée dans le manifeste **et** demandée à l'exécution) avant toute connexion au réseau local : TCP vers le Pi, mDNS, noms `.local`. Sans elle, les connexions **expirent sans message d'erreur**. Voir <https://developer.android.com/privacy-and-security/local-network-permission>. (`NEARBY_WIFI_DEVICES` n'était que la règle provisoire d'Android 16.)
- Fournir à `registerServiceInfoCallback` un exécuteur partagé qu'on ne ferme jamais : le fermer fait planter l'appli (`RejectedExecutionException`).
- Prévoir une saisie manuelle de l'adresse (`http://192.168.1.50:8765`) quand la découverte échoue (réseau invité, répéteur Wi-Fi) et mémoriser la dernière adresse qui a marché.
- Vérifier `GET /health` après la résolution.
- Testé sur un Pixel 8a sous Android 17 avec l'appli `../../android-app` : le serveur est trouvé automatiquement.

## 2. Conventions de l'API

- JSON, UTF-8. HTTP clair : ajouter `android:usesCleartextTraffic="true"` (ou une `network-security-config` pour le réseau local).
- Pas d'authentification : réseau local uniquement.
- Erreurs : `{"error": "..."}` avec `400`, `404`, `409` ou `500`.
- Listes paginées : `{total, limit, offset, items}` (`/albums`, `/artists`). La recherche renvoie trois listes.
- Identifiants numériques stables : un `track_id` ou `album_id` reste valide après un rescan tant que le fichier existe.
- Le serveur ne dit jamais « aucun résultat » par une erreur : liste vide.

## 3. Écrans et endpoints

| Écran | Endpoints |
| --- | --- |
| Accueil (grille) | `GET /albums?sort=recent\|random\|name&limit=60&offset=` ; pochette `GET /cover/{id}?size=300` |
| Album | `GET /albums/{id}` (pistes triées disque/numéro) ; `POST /player/play {"album_id":N,"start":0}` |
| Artistes | `GET /artists` ; `GET /albums?artist=Nom` |
| Recherche | `GET /search?q=...` à chaque frappe (réponse en 10 à 17 ms sur le réseau local), avec un anti-rebond de ~150 ms côté appli |
| Lecture en cours | `GET /player/events` (SSE) ; `POST /player/pause`, `/resume`, `/next`, `/previous`, `/volume`, `/mute`, `/seek` |

## 4. Pochettes

- `GET /cover/{album_id}?size=` avec `size` ∈ {64, 150, 300, 600}. Vignette de grille : 300 ; liste : 64 ou 150 ; plein écran : 600.
- Les albums portent `has_cover` et `cover_path`. **Si `has_cover` est faux, ne pas faire la requête** : afficher un visuel de remplacement. Un `404` signifie aussi « pas de pochette ».
- Les miniatures partent avec `ETag` et `Cache-Control: max-age=86400` : laisser Coil / OkHttp les mettre en cache.
- La première requête d'une pochette génère la miniature (~60 ms par image sur le Pi, 2 à la fois) ; les suivantes viennent du cache disque. Une grille de 24 pochettes s'est affichée en 1,5 s à froid, 0,16 s ensuite.

## 5. Lecteur et temps réel

`GET /player/events` est un flux **Server-Sent Events** : chaque événement `data:` contient l'instantané complet (`status`, `index`, `current`, `position_ms`, `duration_ms`, `volume`, `queue`, `error`, `heos_connected`, `updated_at_ms`).

- **La position n'est rafraîchie que toutes les ~5 s** (c'est le rythme du Denon). Tant que `status == "playing"`, l'appli extrapole : `position = position_ms + (maintenant - updated_at_ms)`, bornée par `duration_ms`. L'horloge du téléphone et celle du Pi peuvent différer : mesurer le décalage à la réception d'un événement plutôt que de comparer des heures absolues.
- Les commandes `POST /player/*` répondent `202` avec l'état **avant** que le Denon réagisse : se fier au flux SSE, pas à la réponse.
- Les erreurs de lecture arrivent dans `error` (le serveur passe seul au titre suivant, puis s'arrête après 3 erreurs d'affilée).
- `status == "loading"` dure ~0,5 s au changement de titre (l'ampli ne peut pas précharger) : afficher un état de chargement, pas une erreur.
- `heos_connected == false` : le Denon est injoignable ; désactiver les commandes.
- **Déplacement** : `POST /player/seek {"position_ms":N}` ou `{"delta_ms":±N}`. Le serveur relance le flux à l'endroit voulu : **~1 s de silence**, pendant laquelle `status` repasse par `loading`. N'envoyer la position que quand le doigt est levé (pas à chaque mouvement du curseur) et garder la valeur choisie à l'écran jusqu'au prochain instantané.
- **Coupure du son** : `POST /player/mute {"mute":bool}`, état dans `mute`.
- Un blanc de 0,3 à 0,5 s entre deux titres est normal.
- Le volume est entre 0 et 100 ; le Denon écoute aussi la télécommande et l'appli HEOS : le volume peut changer sans commande de l'appli, il est mis à jour par le flux.
- Reconnecter le SSE avec un délai croissant si le flux se ferme (redémarrage du service, coupure Wi-Fi).

## 5 bis. Lecture sur le téléphone et téléchargements

- Le serveur sert aussi les titres à l'appli : `GET /stream/{id}.mp3` (`Range` pris en charge, ce qu'ExoPlayer utilise pour se déplacer) et `GET /cover/{id}?size=600`. Pour télécharger un album : `GET /albums/{id}` (pistes), puis chaque `/stream/{id}.mp3`.
- **Sonde « le serveur est-il là ? »** : `GET /health` doit répondre `{"status":"ok"}`. Vérifier le corps, pas seulement la connexion : l'adresse du Pi peut appartenir à un autre appareil sur un autre réseau. Délais courts (1,5 s).
- Sur le téléphone, la lecture et le déplacement sont locaux : le `/player/seek` du serveur ne concerne que le Denon.

## 5 ter. « Même style » (suggestions)

- `GET /recommendations?track_id=<en cours>&limit=20&exclude=<ids en file, séparés par des virgules>` renvoie des titres de la bibliothèque qui ressemblent au titre en cours (voir le [README](../README.fr.md)). Chaque élément a un `reason` ; `source` indique si Last.fm a contribué (`lastfm`) ou seulement les tags locaux (`local`).
- Les ajouter : ampli `POST /player/enqueue {"track_ids":[...]}` (la file ne doit pas être vide) ; téléphone `addMediaItems`. Si la lecture était terminée (statut `stopped` sur le dernier titre), sauter au premier nouveau titre.
- **Retirer un titre de la file** : `POST /player/remove {"index":N,"track_id":ID}` (ampli ; `400` si la file a changé ou si c'est le titre en cours ; sur le téléphone `removeMediaItem`). L'appli de référence le fait par un glissement à gauche ou à droite sur une ligne de la file, et reconstruit les lignes si l'appel échoue.
- **Le serveur ajoute lui-même les suggestions** pour l'ampli : l'instantané contient `auto_suggest` (à basculer avec `POST /player/auto_suggest {"enabled":false}`, commun à tous les clients) et chaque titre ajouté par le serveur a un `reason`. Les afficher comme suggestions et ne pas demander de lots soi-même quand `auto_suggest` est présent. **Défilement sans fin** : quand l'utilisateur approche de la fin de la file, `POST /player/suggest_more {"limit":10}` en ajoute 10 de plus (ils arrivent par le flux d'événements) ; ne redemander qu'une fois la file agrandie.
- **Suggestions automatiques sur le téléphone** (et avec un serveur sans champ `auto_suggest` ; l'appli de référence le fait par défaut) : juste après le début d'une file, demander une fois, avec le dernier titre de la file comme point de départ, environ 10 titres et les ajouter, pour que l'utilisateur voie la suite et puisse la passer ; redemander quand le dernier titre de la file démarre. Une seule fois par file / par titre, seulement si le serveur est `Online`, et garder les ids reçus (`reason`) pour que la file puisse les marquer comme suggestions.
- Dire à l'utilisateur d'où viennent les suggestions : avec une clé Last.fm sur le serveur, l'artiste et le titre sont envoyés à Last.fm.
- L'appel est léger (cache : quelques dizaines de millisecondes ; premier appel environ 0,5 s).

## 6. Pièges

- **Deux sorties** : l'ampli (le serveur le pilote, l'appli ne fait que commander) ou le téléphone lui-même (l'appli lit `/stream/{id}.mp3` ou un fichier téléchargé). L'API `/player/*` ne concerne que l'ampli.
- Les titres, artistes et albums viennent des tags des fichiers : on y trouve des préfixes de numéro (`01. Help!`), des variantes d'orthographe (`AC/DC`, `Ac-Dc`) et des années dans les noms d'album. Ne pas « nettoyer » côté appli sans règle explicite.
- `various_artists: true` : compilation, afficher « Artistes divers » plutôt que l'artiste majoritaire renvoyé.
- Le serveur peut être en train de scanner (`GET /scan`, `running: true`) : les listes se complètent au fil de l'eau.
