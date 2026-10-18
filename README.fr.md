<div align="center">

[English](README.md) · [Español](README.es.md) · **Français** · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [简体中文](README.zh_CN.md) · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**Un client TIDAL natif et rapide, avec lecture Hi-Res bit-perfect.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell affichant un album, avec le panneau de lecture en cours ancré à droite](docs/screenshots/album.png)

</div>

Swell est écrit en Rust avec [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) et
[GStreamer](https://gstreamer.freedesktop.org/). Votre bibliothèque s’ouvre
instantanément depuis un cache local, le FLAC Hi-Res parvient au serveur
audio sans être modifié, et l’interface s’adapte des fenêtres de la taille
d’un téléphone aux grands bureaux, en n’utilisant qu’une fraction de la
mémoire des autres clients ([mesures](#mesures)).

> [!NOTE]
> Swell n’est ni affilié à TIDAL, ni approuvé ou pris en charge par TIDAL. Il
> utilise les API non officielles de TIDAL, qui peuvent changer sans
> préavis, et nécessite un abonnement TIDAL payant.

## Sommaire

- [Fonctionnalités](#fonctionnalités)
- [Captures d’écran](#captures-décran)
- [Installation](#installation)
- [Connexion et qualité audio](#connexion-et-qualité-audio)
- [Lecture bit-perfect](#lecture-bit-perfect)
- [Langues](#langues)
- [Confidentialité](#confidentialité)
- [Mesures](#mesures)
- [Développement](#développement)
- [Licence](#licence)

## Fonctionnalités

**Parcourir**
- Accueil : votre fil TIDAL personnalisé (raccourcis, mix, recommandations,
  écoutés récemment), qui en charge davantage au fil du défilement.
  Explorer : pages éditoriales, genres, ambiances et décennies.
- Recherche au fil de la frappe, avec filtres (titres, albums, artistes,
  playlists), meilleur résultat, recherches récentes et pagination ; coller
  un lien TIDAL l’ouvre.
- Pages d’album, d’artiste, de playlist, de mix et de radio, avec crédits,
  critiques, biographies, artistes similaires et pages « Tout afficher ».
- Les liens `tidal://` et `https://tidal.com/…` s’ouvrent dans Swell.

**Votre collection**
- Des milliers de titres favoris, virtualisés, avec filtre et tri
  instantanés ; albums, artistes, playlists et mix favoris ; écoutés
  récemment.
- Des cœurs sur tout (titres, albums, artistes, playlists, mix), à partir
  d’une seule requête pour toute la bibliothèque.
- Créez, renommez, décrivez, publiez et supprimez des playlists ; ajoutez
  des titres depuis un menu ou en les faisant glisser sur une playlist de la
  barre latérale ; retirez-les et réorganisez-les, avec annulation.

**Lecture**
- Hi-Res Lossless (FLAC jusqu’à 24 bits/192 kHz), [bit-perfect](#lecture-bit-perfect).
- Lecture sans coupure, ReplayGain facultatif (titre ou album), choix du
  périphérique de sortie.
- Une vraie file d’attente : lire ensuite, ajouter à la file, sauter,
  retirer, réorganiser, vider avec annulation ; lecture aléatoire, répétition
  de tout ou d’un titre, historique.
- Résiste aux coupures réseau et aux URL de flux expirées, en reprenant à la
  même position.
- Paroles synchronisées qui suivent le morceau et permettent de se déplacer
  d’un clic.
- Restaure votre file d’attente et votre position au démarrage ; minuteur
  d’arrêt ; notifications de bureau ; empêche la mise en veille du système
  pendant la lecture.
- MPRIS sur Linux : les touches multimédias, les widgets de bureau et
  `playerctl` fonctionnent.

**Interface**
- Design libadwaita qui suit le style clair/sombre et la couleur d’accentuation
  du système, avec trois dispositions : large avec un panneau de lecture
  ancré, normale, et étroite avec un volet inférieur et un mini-lecteur.
- Contrôle complet au clavier (Espace, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…, Ctrl+?
  liste tous les raccourcis) et noms accessibles sur chaque contrôle.
- Anglais, espagnol, français, allemand, italien, portugais (Brésil),
  russe, japonais, coréen et chinois (simplifié et traditionnel).

## Captures d’écran

| Accueil | Recherche |
|---|---|
| ![Fil d’accueil avec raccourcis, mix et écoutés récemment](docs/screenshots/home.png) | ![Résultats de recherche pour « yoasobi » avec un meilleur résultat, des titres et des artistes](docs/screenshots/search.png) |
| **Paroles synchronisées** | **Style clair** |
| ![Panneau de lecture en cours affichant la file d’attente](docs/screenshots/now-playing.png) | ![La page d’un album dans le style clair](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="Disposition étroite avec le mini-lecteur et la barre de navigation inférieure">
</p>

## Installation

Les paquets de chaque version se trouvent sur la
[page des versions](https://github.com/oxakromax/swell-player/releases).
Swell nécessite GTK ≥ 4.18, libadwaita ≥ 1.7 et GStreamer avec les
greffons base et good ; les paquets les installent automatiquement.

### Arch Linux (AUR)

Le paquet AUR arrive bientôt. D’ici là, compilez le même PKGBUILD à partir
de ce dépôt :

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

Une fois sur l’AUR : `yay -S swell-player` (ou `swell-player-git`).

### Flatpak

Téléchargez le paquet `.flatpak` depuis la page des versions et installez-le
(il utilise l’environnement d’exécution GNOME de Flathub). Une fiche Flathub
est prévue.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian et Ubuntu

Nécessite Debian 13 (trixie), Ubuntu 25.04 ou plus récent (GTK ≥ 4.18 et
libadwaita ≥ 1.7). Téléchargez le `.deb` depuis la page des versions :

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Téléchargez le `.rpm` depuis la page des versions :

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Téléchargez l’installeur (ou le `.zip` portable) depuis la page des versions.
Les builds ne sont pas encore signés, donc SmartScreen peut avertir au sujet
d’une application non reconnue : choisissez **Informations complémentaires →
Exécuter quand même**.

### macOS

Téléchargez le `.dmg` depuis la page des versions et faites glisser Swell
dans Applications. L’app n’est pas encore notariée : la première fois,
faites un clic droit (ou Ctrl-clic) dessus et choisissez **Ouvrir** ; sur
macOS 15 et ultérieur, ouvrez-la une fois, puis autorisez-la dans
**Réglages Système → Confidentialité et sécurité → Ouvrir quand même**.
Comme chaque version est signée ad hoc, macOS peut redemander l’accès à
l’élément de trousseau de Swell après une mise à jour : choisissez
**Toujours autoriser**. Les liens `tidal://` n’ouvrent pas encore Swell sur
macOS, et l’image disque Intel (x86_64) est fournie au mieux, donc une
version peut en être dépourvue.

Les builds Windows et macOS sont plus récents que celui de Linux : MPRIS et
les remarques sur le bit-perfect ci-dessous sont propres à Linux.

### Depuis les sources

Il vous faut Rust ≥ 1.96 ([rustup](https://rustup.rs) si celui de votre
distribution est plus ancien), GTK ≥ 4.20 et libadwaita ≥ 1.8 (ou GTK 4.18 et
libadwaita 1.7 avec `--no-default-features`), GStreamer ≥ 1.24, SQLite et
`glib-compile-resources` de GLib.

<details>
<summary>Dépendances de compilation par distribution</summary>

```bash
# Arch Linux
sudo pacman -S --needed rust pkgconf gtk4 libadwaita sqlite \
    gstreamer gst-plugins-base gst-plugins-good

# Debian 13 / Ubuntu: their rustc is too old, so install Rust with rustup,
# and build with `make CARGOFLAGS=--no-default-features` where GTK is
# older than 4.20 (Debian 13 ships 4.18)
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
    libsqlite3-dev libglib2.0-dev-bin libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good

# Fedora
sudo dnf install cargo pkgconf-pkg-config gtk4-devel libadwaita-devel sqlite-devel \
    glib2-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good
```

`gst-plugins-good` fournit le démultiplexeur DASH (`dashdemux2`) utilisé pour
les flux Hi-Res.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` prépare une installation pour le
packaging, et `sudo make uninstall` la supprime. Le binaire fonctionne aussi
directement depuis `target/release/swell-player`, puisque les icônes et les
traductions sont intégrées.

## Connexion et qualité audio

TIDAL ne permet pas aux applications tierces d’enregistrer leurs propres
clients OAuth ; Swell, comme tous les clients TIDAL open source, se connecte
donc en se faisant passer pour l’une des applications de TIDAL. Le mode de
connexion que vous choisissez détermine laquelle, et donc la qualité maximale :

| Connexion | Fonctionnement | Meilleure qualité |
|---|---|---|
| **Navigateur** (recommandé) | Swell ouvre la page de connexion de TIDAL dans votre navigateur. Une fois connecté, TIDAL aboutit sur une page qui indique « Oops » ; copiez l’adresse de cette page dans Swell. | **Hi-Res Lossless** : FLAC jusqu’à 24 bits/192 kHz |
| **Code d’appareil** | Swell affiche un court code ; approuvez-le sur `link.tidal.com` depuis n’importe quel appareil. | **Lossless** : FLAC 16 bits/44,1 kHz |

Choisissez la qualité du streaming dans **Préférences → Lecture**. Le badge
de format de la barre du lecteur indique ce qui est réellement fourni.

## Lecture bit-perfect

Avec la normalisation désactivée et le volume à 100 %, Swell conserve le
format du flux jusqu’à PipeWire : un titre en 24 bits/96 kHz lui parvient en
24 bits à 96 kHz. Le badge de format de la barre du lecteur ouvre une
fenêtre contextuelle qui indique si la sortie est bit-perfect et, sinon,
pourquoi.

PipeWire décide ensuite de ce que reçoit la carte son. Par défaut, il
fonctionne à une seule fréquence (souvent 48 kHz) et rééchantillonne tout le
reste. Pour qu’il suive la fréquence de chaque titre, autorisez les
fréquences courantes, par exemple dans
`~/.config/pipewire/pipewire.conf.d/10-rates.conf` :

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

puis redémarrez PipeWire (`systemctl --user restart pipewire`). `pw-top`
montre ce qui parvient au périphérique.

## Langues

Swell parle **anglais**, **espagnol**, **français**, **allemand**,
**italien**, **portugais (Brésil)**, **russe**, **japonais**, **coréen** et
**chinois** (simplifié et traditionnel). Il suit automatiquement la langue
du système (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, comme gettext), et
vous pouvez la modifier dans les **Préférences**. Le contenu propre à TIDAL
(textes éditoriaux, titres de pages) est demandé dans la même langue, de sorte
qu’un écran ne mélange jamais deux langues.

Vous voulez Swell dans votre langue ? Consultez
[docs/translating.md](docs/translating.md).

## Confidentialité

- Les **jetons** sont conservés dans le trousseau de clés de votre système
  (sous Linux, le Secret Service : GNOME Keyring, KWallet, KeePassXC…, ou le
  portail de secrets dans Flatpak), jamais en clair sur le disque. Ils sont
  renouvelés peu avant leur expiration, et un problème réseau ne vous
  déconnecte jamais.
- **Mis en cache sur votre ordinateur :** les réponses de TIDAL et votre
  bibliothèque (`~/.cache/swell-player/library.sqlite3`), les pochettes,
  vos réglages, la file d’attente enregistrée et les recherches récentes.
  Les préférences permettent de vider le cache ; la déconnexion supprime les
  jetons, le cache, la file d’attente enregistrée et l’historique de
  recherche.
- **Aucune télémétrie.** Swell ne se connecte qu’aux serveurs de TIDAL, pour
  ce que vous lui demandez d’afficher ou de lire. Il n’a ni analyse
  d’audience, ni rapport de plantage, ni autre service tiers.

## Mesures

Swell face à [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject) sur la même machine, tous deux dans une session KWin
sans affichage (`kwin_wayland --virtual`, 1280×860) avec une session
enregistrée. Médianes de trois exécutions ; mémoire lue depuis `/proc`
(RssAnon est le tas propre de l’application ; RSS compte aussi les
bibliothèques partagées et environ 250 Mo de mappages du pilote graphique
que toute application GTK 4 a ici). Les chiffres au repos sont des moyennes
sur 10 s sans lecture.

| | Swell | High Tide |
|---|---|---|
| Accueil à l’écran : RssAnon / RSS | **61 Mo / 360 Mo** | 131 Mo / 504 Mo |
| 1 295 titres favoris à l’écran, depuis le cache | **0,42 s** après le lancement (synchronisé avec TIDAL à 0,50 s) | — ¹ |
| … sans le cache de Swell | les 100 premiers en **0,59 s**, tous en 1,2–1,9 s | — ¹ |
| Vue Favoris : RssAnon / RSS | 85 Mo / 371 Mo | — ¹ |
| CPU au repos / réveils du fil principal | ~0 % / 1,1 par s | ~0 % / 0,1 par s |
| Threads | 15 | 19 |
| Titre Hi-Res choisi → audio | **0,67 s** (24 bits/96 kHz, parvient à PipeWire en S24_32 à 96 kHz) | — |

¹ High Tide ne met pas les favoris en cache et n’a aucun moyen d’ouvrir
directement sa liste de favoris ; avec un banc de test, la preuve de concept
de Swell a mesuré 13,3 s pour que High Tide affiche la même liste, contre
0,3 s pour Swell.

Machine : Arch Linux, KDE Plasma 6 (Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, build de release.

## Développement

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) explique comment compiler, tester, les
variables d’environnement de débogage (ouvrir une page, lire un titre ou
produire la fenêtre en PNG sans écran) et comment proposer des modifications.
[docs/architecture.md](docs/architecture.md) explique l’organisation du code ;
[docs/features.md](docs/features.md) décrit le périmètre fonctionnel et
[docs/design.md](docs/design.md) la spécification de l’interface (en
espagnol). Les notes de version se trouvent dans [CHANGELOG.md](CHANGELOG.md).

Vous avez trouvé un problème de sécurité ? Merci de le signaler en privé :
voir [SECURITY.md](SECURITY.md).

## Licence

Swell est un logiciel libre : vous pouvez le redistribuer et/ou le modifier
selon les termes de la licence publique générale GNU telle que publiée par la
Free Software Foundation, soit la version 3 de la licence, soit (à votre
choix) toute version ultérieure. Voir [COPYING](COPYING).

TIDAL est une marque déposée de son propriétaire, utilisée ici uniquement
pour indiquer avec quoi Swell fonctionne.
