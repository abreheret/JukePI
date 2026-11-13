# Notes de développement

🇬🇧 [English](NOTES.md)

Décisions clés, enseignements et feuille de route du serveur. Les mesures détaillées sont dans [HEOS-FINDINGS.fr.md](HEOS-FINDINGS.fr.md), la conception dans [ARCHITECTURE.fr.md](ARCHITECTURE.fr.md).

## Décisions clés

| Décision | Pourquoi |
| --- | --- |
| L'ampli télécharge lui-même les MP3 (`play_stream` avec une URL HTTP) ; rien ne passe par le téléphone | Validé d'abord par un test manuel de 10 minutes (un simple `http.server` + une session telnet sur le port 1255) avant d'écrire du code. Pas de serveur DLNA nécessaire |
| Les titres sont servis **par identifiant numérique** (`/stream/{id}.mp3`) | Toutes les URL données à l'ampli sont en ASCII pur : il ne décode rien dans une URL (une URL doublement encodée donnait un `404`) |
| **Un album = un dossier** | Grouper par tag éclate les compilations en dizaines de faux albums |
| Identifiants de pistes stables (upsert par chemin) | Les URL et les files d'attente survivent à un rescan |
| Le lecteur est un **acteur** unique | Pas de verrou, pas de course entre les commandes de l'API et les événements de l'ampli |
| `play_stream` + notre propre file, enchaînement sur l'événement `stop` | L'ampli n'a pas de file native exploitable pour des URL ; envoyer le titre suivant à l'avance n'apporte rien (mesuré) |
| **Déplacement en servant le titre à partir d'une trame MP3** | HEOS n'a pas de commande de déplacement ; le flux coupé est accepté par l'ampli sans en-tête ni `Range` |
| Index FTS5 reconstruit depuis les tables après un scan qui a changé quelque chose | Simple et assez rapide (3 s pour 43 794 titres sur un Pi 4) |
| Pochettes en ligne **sur option** | Elles envoient noms d'artistes et d'albums à des tiers |
| L'index est sur la carte SD, le disque de la bibliothèque en lecture seule | Un disque débranché ou « sale » ne doit jamais pouvoir corrompre quoi que ce soit |
| Les recommandations viennent de la **similarité Last.fm filtrée par la bibliothèque**, avec les tags locaux en repli | La bibliothèque est la seule chose jouable : ce que Last.fm suggère et que la bibliothèque n'a pas est inutile ; un repli local garde la fonction utilisable sans clé ou hors ligne |
| Last.fm est **facultatif à deux niveaux** (clé : suggestions ; clé + secret + compte lié : scrobbling) et le compte est lié dans le navigateur | Rien n'est envoyé sans configuration, et le mot de passe Last.fm n'atteint jamais le serveur |
| Les secrets sont dans un fichier réservé à root sur le Pi, installé depuis un fichier local ignoré par git | Le dépôt peut être public sans jamais contenir de clé |
| Compilation croisée avec `zig` + `cargo-zigbuild` | Mieux vaut laisser le Pi à ses autres tâches que lui faire compiler |

## Enseignements

- **La file interne de l'ampli boucle.** Après la fin d'un clip, le Denon a rejoué des URL d'expériences précédentes. Vider la file avant chaque titre règle le problème, mais `clear_queue` pendant une lecture arrête le titre et chaque `play_stream` émet un faux `stop` : le lecteur ignore les `stop` tant qu'une transition qu'il a lancée est en cours.
- **Le Denon ne lit le tag d'un flux que lorsqu'il voit les en-têtes de trames changer.** Les fichiers à débit variable le font d'eux-mêmes ; pour les autres, une trame de silence de 26 ms à l'en-tête différent (un autre débit et le bit CRC inversé) une demi-seconde plus loin le provoque. Trouvé en mesurant, pas en raisonnant : sept explications plausibles (accents, enchaînement, URL en cache, trames d'en-tête, encodages du tag, Range, longueur) ont été testées et écartées d'abord, et « ça marche sur un fichier » a trompé deux fois. Tester sur beaucoup de fichiers.
- **Ce que l'ampli affiche est ce que dit le tag ID3 du flux lui-même**, lu quelques secondes après le début, et une trame d'en-tête Xing/Info devant l'audio l'empêche de s'afficher. Deux correctifs qui ont un prix : le serveur construit le tag depuis son index et supprime la trame d'en-tête, ce qui rend l'estimation de durée de l'ampli peu fiable, d'où la durée exacte lue dans l'en-tête (`duration_exact`). Attention : l'observation est bruitée, seules des mesures répétées sur des fichiers de taille normale valent quelque chose.
- **Beaucoup de MP3 réels n'ont pas d'en-tête Xing/VBRI.** Les durées ne sont alors que des estimations, et certains lecteurs (ExoPlayer) refusent de se déplacer si on ne leur dit pas de supposer un débit constant. Le code de déplacement parcourt lui-même les trames MP3 et ne se fie à aucune table.
- **`ffprobe` devine la durée d'un VBR coupé d'après sa taille** : mesurer plutôt la durée décodée.
- **Un disque démonté ne doit pas vider l'index** : le scan refuse de supprimer quand il ne trouve aucun MP3.
- **La qualité des tags est ce qu'elle est** : 175 fichiers sur 43 794 avaient des tags illisibles (frames ID3 invalides, UTF-16 tronqué) ; ils sont indexés quand même, avec le nom de fichier comme titre. 14 % n'avaient pas de tag artiste : le nom du dossier sert de repli.
- **Les images de dossier sont bien plus courantes que les images intégrées** (environ 85 % contre 11 % des albums de la bibliothèque de test), et les restes de Windows Media Player (`AlbumArt_{GUID}_Large.jpg`, `AlbumArtSmall.jpg`) demandent une règle de notation pour choisir le bon fichier.
- **MusicBrainz renvoie beaucoup d'éditions d'une sortie** : la pochette du `release-group` est le bon défaut (sinon on tombait sur la pochette d'une édition régionale).
- Les pièges d'exploitation (SSH, `sudo`, ports, ponts Docker, NTFS) sont dans [OPERATIONS.fr.md](OPERATIONS.fr.md).

## Chiffres (configuration de référence : Pi 4, 4 Go, SSD USB 3)

| Mesure | Valeur |
| --- | --- |
| Premier scan, 43 794 MP3 à travers FUSE/NTFS | 157 s |
| Rescan sans changement | 2 à 4 s |
| Reconstruction de l'index de recherche | 3 s |
| Requête de recherche | 10 à 17 ms |
| Résolution des pochettes de 3 339 albums | ~11 s |
| 24 premières miniatures (à froid, 6 requêtes en parallèle) | 1,5 s |
| Écart entre deux titres | 0,3 à 0,5 s |
| Déplacement (commande → son) | ~1 s |
| `play_stream` → `state=play` | 0,43 à 0,55 s |

## Feuille de route

- [x] Recommandations (« même style ») : similarité Last.fm + tags locaux + historique d'écoute. [ ] Empreintes audio plus tard.
- [ ] Pas encore vérifié par une vraie écoute : les scrobbles qui apparaissent sur le profil, l'enchaînement automatique en fin d'album.
- [ ] Scrobbler aussi ce que joue le téléphone (l'appli devrait remonter ses écoutes au serveur).
- [x] Recherche tolérante aux fautes de frappe et orthographes d'artistes fusionnées (« Queen »/« QUEEN », « AC/DC »/« Ac-Dc »).
- [ ] Pré-générer les miniatures de 300 px après un scan si la première ouverture de la grille est lente.
- [ ] Surveillance du disque (`notify`) au lieu de `POST /scan`.
- [ ] Authentification facultative si l'API sort un jour du réseau local.
- [ ] Un vrai test de redémarrage de la chaîne `fstab` + démarrage du service ; une exécution complète de `deploy/deploy.sh`.
- [ ] Non testé : la lecture complète d'un mix continu de 189 Mo ; le comportement après un redémarrage de l'ampli en pleine lecture ; l'ampli en Ethernet.
