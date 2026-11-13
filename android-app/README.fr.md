# Appli Android JukePI

🇬🇧 [English](README.md)

[![Android](https://github.com/abreheret/JukePI/actions/workflows/android.yml/badge.svg)](https://github.com/abreheret/JukePI/actions/workflows/android.yml)

La moitié Android de JukePI. Elle parcourt votre bibliothèque (pochettes, albums, artistes, recherche) et soit **pilote un ampli Denon HEOS** par l'intermédiaire du serveur JukePI, soit **joue sur le téléphone lui-même**, y compris **hors connexion** avec des albums téléchargés. Le serveur est dans [`../music-server`](../music-server/README.fr.md).

Cible : **Android 17 (API 37)** (développé sur un Pixel 8a). Kotlin, Jetpack Compose, Material 3 (couleurs dynamiques). L'identifiant de paquet est `app.musicremote`, le nom affiché est « JukePI ».

## Fonctions

| Fonction | État |
| --- | --- |
| Demande de la permission réseau local (obligatoire sur Android 17) | fait, testé |
| Découverte automatique du serveur (mDNS) et saisie manuelle de l'adresse | fait, testé |
| Grille d'albums avec pochettes, tri (récents, A–Z, artiste, au hasard), défilement infini | fait, testé |
| **« Récents »** montre d'abord ce que vous avez écouté (les lectures sur l'ampli sont enregistrées par le serveur, le téléphone signale les siennes), puis les albums avec pochette | fait, testé |
| Bouton « Tous les albums de cet artiste » sur la page d'un album et dans le lecteur plein écran | fait, testé |
| Page album, artistes, recherche plein texte (au fil de la saisie, tolérante aux fautes : « Résultats approchants » est indiqué) | fait, testé |
| Mini-lecteur et lecteur plein écran (précédent, pause, suivant, arrêt, file d'attente) | fait, testé |
| Barre de progression déplaçable, boutons ±10 s et ±30 s | fait, testé (sur l'ampli le serveur relance le flux : ~1 s de silence ; sur le téléphone le déplacement est instantané) |
| Volume (les touches de volume du téléphone pilotent l'ampli), coupure du son | fait, testé |
| Notification et commandes de l'écran verrouillé (`MediaSession` Media3, cinq boutons dont ±10 s) | fait ; boutons de la notification testés ; l'écran verrouillé lui-même n'a pas été vérifié visuellement |
| Touches multimédia matérielles (boutons d'un casque Bluetooth) | fonctionnent quand la sortie est le téléphone (voir Limites connues) |
| Icône d'application adaptative (avec couche monochrome pour les icônes thématiques) | fait |
| **Choix de la sortie ampli / téléphone** dans le lecteur plein écran (la file et la position suivent) | fait ; sélecteur vérifié, transfert pas encore testé |
| **Lecture sur le téléphone** (ExoPlayer) : fichiers téléchargés, ou en continu depuis le serveur | fait, testée |
| **Téléchargement d'albums** (Télécharger / Annuler / Retirer sur la page de l'album, reprise après interruption) | fait, testé (6 titres, 47 Mo en ~3 s) |
| **Onglet « Téléchargés »** : tout ce qui est sur le téléphone, état et taille de chaque album, espace libre, gestion | fait, vérifié |
| **Mode hors connexion** : détection automatique, bandeau, albums / artistes / recherche limités au contenu téléchargé, sortie forcée sur le téléphone | fait, testé en coupant le réseau |
| **« Même style »** (bouton du lecteur plein écran) et **suggestions à la suite de ce que vous jouez** : quand une file démarre, 10 titres de la bibliothèque proches du dernier sont ajoutés, repérés par une étoile et un titre « Suggestions » dans la file (passez ce que vous ne voulez pas) ; 10 de plus quand le dernier titre démarre (réglage, activé par défaut ; demande le serveur) | fait ; le côté serveur est testé sur la vraie bibliothèque, le repérage dans la file pas encore vérifié par une vraie écoute |
| Reprise d'un téléchargement interrompu au premier titre manquant | écrit ; pas testé en tuant l'appli en plein téléchargement |

## Limites connues

- **Les touches multimédia matérielles** (boutons d'un casque Bluetooth) n'atteignent l'appli que lorsque la sortie est le **téléphone** : Android les envoie à l'appli qui joue réellement de l'audio en local. Avec l'ampli comme sortie, les boutons de la notification et de l'écran verrouillé fonctionnent (ils parlent directement à la session) mais pas ceux du casque. Contourner cela demanderait de jouer un son silencieux sur le téléphone : non fait.
- Hors connexion, un titre non téléchargé n'est pas lisible (grisé dans la page de l'album).
- Sur l'ampli, un déplacement dans un titre coûte **~1 s de silence** (le serveur relance le flux à l'endroit voulu).
- Les boutons ±30 s n'existent que dans le lecteur plein écran ; la notification et l'écran verrouillé ont ±10 s (Android autorise cinq boutons au maximum).

## Compiler et installer

Pré-requis : un JDK 21, les outils en ligne de commande du SDK Android avec la plateforme 37, les build-tools 37 et platform-tools, et Gradle seulement pour créer le wrapper (déjà dans le dépôt). Sous macOS avec Homebrew : `brew install openjdk@21` et `brew install --cask android-commandlinetools`, puis `sdkmanager "platform-tools" "platforms;android-37.0" "build-tools;37.0.0"` et acceptation des licences (`sdkmanager --licenses`).

```bash
echo "sdk.dir=/chemin/du/sdk/android" > local.properties     # ignoré par git
./install.sh        # compile, installe sur le téléphone branché en USB, lance l'appli
```

Ou à la main :

```bash
export JAVA_HOME=/chemin/du/jdk-21 ANDROID_HOME=/chemin/du/sdk/android
./gradlew assembleDebug
$ANDROID_HOME/platform-tools/adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`JAVA_HOME` doit pointer sur le **JDK 21** : le `gradle` de Homebrew démarre sur un JDK plus récent que ce que le plugin Android accepte. Le téléphone doit avoir le **débogage USB** activé et l'ordinateur autorisé (`adb devices` doit afficher `device`).

## Build de release

`./gradlew assembleRelease` donne un APK réduit et léger (`app/build/outputs/apk/release/app-release.apk`, environ 5 Mo). Il est signé avec **votre propre clé** (gardée hors du dépôt) quand vous définissez `JUKEPI_KEYSTORE`, `JUKEPI_KEYSTORE_PASSWORD` et, au besoin, `JUKEPI_KEY_ALIAS` / `JUKEPI_KEY_PASSWORD` ; sinon avec la clé de *debug* locale, suffisante pour essayer l'appli mais jamais pour publier. Utilisez `./scripts/build-release.sh` depuis la racine du dépôt : il construit les fichiers d'une release et refuse de signer avec la clé de debug.

**Qui a signé quelle release.** La v0.1.0 est signée avec une clé de debug. À partir de la v0.2.0, les APK sont signés avec la clé de release du projet, dont le SHA-256 du certificat est :

```
f94f4141db9d973553f30b77d4b2b5469af8c038b40c94dc606d7ba36f4e4b1c
```

Vérifiez un fichier téléchargé avec `apksigner verify --print-certs JukePI-*.apk` (il affiche `CN=JukePI` et cette empreinte). Android refuse d'installer une appli par-dessus une autre signée avec une autre clé : **passer de la v0.1.0 demande une désinstallation** (elle efface les albums téléchargés, qui se re-téléchargent en quelques secondes). Ensuite, chaque release se met à jour sur place.

## Versions (novembre 2026)

Android Gradle Plugin 9.5.1 (Kotlin intégré), Kotlin 2.4.21 (plugins Compose et sérialisation), Gradle 9.9.1, Compose BOM 2026.09.00, OkHttp 5.5.1 (+ okhttp-sse), kotlinx.serialization 1.13.0, Coil 3.6.3, Media3 1.11.1 (session + ExoPlayer), WorkManager 2.12.0, `compileSdk` 37, `targetSdk` 37, `minSdk` 34 (donne `NsdManager.registerServiceInfoCallback`).

## Documentation

| Fichier | Contenu |
| --- | --- |
| [docs/ARCHITECTURE.fr.md](docs/ARCHITECTURE.fr.md) | Organisation du code, flux de données, choix et pièges |
| [docs/NOTES.fr.md](docs/NOTES.fr.md) | Notes de développement : enseignements, conseils de test, feuille de route |
| [`../music-server/docs/ANDROID-CLIENT.fr.md`](../music-server/docs/ANDROID-CLIENT.fr.md) | Le contrat côté serveur : API, mDNS, flux temps réel |
