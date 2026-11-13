# Notes de développement

🇬🇧 [English](NOTES.md)

Enseignements, conseils de test et feuille de route de l'appli Android. La conception est dans [ARCHITECTURE.fr.md](ARCHITECTURE.fr.md).

## Enseignements

- **Android 17 bloque le réseau local par défaut** pour les applis qui ciblent l'API 37 : déclarer *et* demander `ACCESS_LOCAL_NETWORK` avant toute connexion (TCP, mDNS, noms `.local`). Sans elle, les connexions expirent simplement, sans erreur. (La règle `NEARBY_WIFI_DEVICES` n'était que l'activation provisoire d'Android 16.)
- **Ne jamais fermer l'exécuteur donné à `NsdManager`** : le système y poste encore des tâches de nettoyage, et l'appli plante avec une `RejectedExecutionException`.
- **Le flux SSE demande un client sans délai de lecture**, sinon il est coupé toutes les quelques secondes.
- **ExoPlayer ne sait pas se déplacer dans un MP3 sans en-tête Xing/Info/VBRI** tant que le déplacement à débit constant n'est pas activé. La moitié des fichiers de la bibliothèque de test sont dans ce cas.
- **Les pas de saut d'ExoPlayer sont 5 s en arrière / 15 s en avant par défaut** : les régler tous deux à 10 s, sinon les boutons « ±10 s » mentent.
- **`setMediaButtonPreferences` sans emplacement explicite remplace Précédent/Suivant** dans la notification ; mettre les boutons de saut dans `SLOT_OVERFLOW` pour garder cinq boutons.
- **Les touches multimédia matérielles vont à l'appli qui joue de l'audio en local.** Avec une `MediaSession` qui ne fait que télécommander, elles n'arrivent jamais ; avec la lecture locale d'ExoPlayer, si (les boutons d'un casque Bluetooth fonctionnent).
- **Android mémorise le volume média par sortie audio** (haut-parleur, Bluetooth A2DP, casque…) : `dumpsys audio` les montre une à une.
- **Après `adb install -r`, forcer l'arrêt de l'appli avant de la lancer**, sinon `am start` répond « task brought to the front » et aucun processus n'existe.
- **Les positions de `dumpsys media_session` ne sont pas fiables** : lire les temps dans l'interface (`uiautomator dump`).
- **Icônes Compose** : dessiner la tête de note et la hampe en tracés séparés, sinon des sens de tracé opposés s'annulent là où ils se recouvrent.
- Un téléchargement WorkManager qui doit dépasser ~10 minutes demande un service de premier plan de type `dataSync` (et sa surcharge dans le manifeste).

## Conseils de test (adb)

- Captures d'écran : `adb exec-out screencap -p > capture.png`. Le téléphone peut afficher autre chose que l'appli (accueil, notifications, messages) : ne regarder que ce qui concerne l'appli.
- Ne jamais capturer le volet de notifications ni l'écran verrouillé : utiliser `adb shell dumpsys notification` filtré sur `pkg=app.musicremote`, ou `uiautomator dump` en n'analysant que les éléments de l'appli.
- Piloter l'interface avec `adb shell input tap/swipe/text` ; vérifier l'état d'abord si quelqu'un d'autre peut utiliser le téléphone.
- Accorder les permissions : `adb shell pm grant app.musicremote android.permission.ACCESS_LOCAL_NETWORK` (et `POST_NOTIFICATIONS`).
- **`adb shell pm clear` supprime aussi tous les albums téléchargés.**
- Fichiers téléchargés : `adb shell ls -l /sdcard/Android/data/app.musicremote/files/music/`.
- Tests qui perturbent la personne qui tient le téléphone (couper le Wi-Fi avec `adb shell svc wifi disable`, déplacer ou arrêter la lecture, changer la sortie, faire du bruit) : baisser le volume média avant, rétablir le réseau et le volume après.
- Plantages : `adb logcat -d | grep -E "FATAL|AndroidRuntime"`.

## Feuille de route

- [ ] Vérifier visuellement l'écran verrouillé (commandes, pochette, titre) et la fenêtre de demande des notifications sur une installation neuve.
- [ ] Tester le transfert de sortie en cours de lecture (ampli → téléphone et retour) et un téléchargement interrompu (tuer l'appli en plein milieu, vérifier la reprise).
- [ ] Un réglage du pas de volume des touches (2 aujourd'hui) et un plafond de volume de sécurité facultatif.
- [ ] La coupure du son côté serveur est faite ; une minuterie de sommeil et des playlists ne le sont pas.
- [ ] Trier les albums téléchargés par artiste ; télécharger plusieurs albums d'un coup.
- [ ] Build de release signé, pour installer sans câble.
