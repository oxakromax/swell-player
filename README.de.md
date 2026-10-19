<div align="center">

[English](README.md) · [Español](README.es.md) · [Français](README.fr.md) · **Deutsch** · [Italiano](README.it.md) · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [简体中文](README.zh_CN.md) · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**Ein schneller, nativer TIDAL-Client mit bit-perfect Hi-Res-Wiedergabe.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell zeigt ein Album, das Now-Playing-Panel ist rechts angedockt](docs/screenshots/album.png)

</div>

Swell ist in Rust mit [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) und
[GStreamer](https://gstreamer.freedesktop.org/) geschrieben. Ihre Bibliothek
öffnet sich sofort aus einem lokalen Cache, Hi-Res-FLAC erreicht den
Soundserver unverändert, und die Oberfläche passt sich an, von Fenstern in
Smartphone-Größe bis zu breiten Desktops – bei einem Bruchteil des
Speicherbedarfs anderer Clients ([Messungen](#messungen)).

> [!NOTE]
> Swell steht in keiner Verbindung zu TIDAL und wird weder von TIDAL
> unterstützt noch empfohlen. Es nutzt die inoffiziellen APIs von TIDAL, die
> sich ohne Vorankündigung ändern können, und benötigt ein kostenpflichtiges
> TIDAL-Abonnement.

## Inhalt

- [Funktionen](#funktionen)
- [Screenshots](#screenshots)
- [Installation](#installation)
- [Anmeldung und Audioqualität](#anmeldung-und-audioqualität)
- [Bit-perfect-Wiedergabe](#bit-perfect-wiedergabe)
- [Sprachen](#sprachen)
- [Datenschutz](#datenschutz)
- [Messungen](#messungen)
- [Entwicklung](#entwicklung)
- [Lizenz](#lizenz)

## Funktionen

**Entdecken**
- Startseite: Ihr personalisierter TIDAL-Feed (Schnellzugriffe, Mixe,
  Empfehlungen, zuletzt gehört), der beim Scrollen weitere Inhalte nachlädt.
  Entdecken: redaktionelle Seiten, Genres, Stimmungen und Jahrzehnte.
- Suche während der Eingabe, mit Filtern (Titel, Alben, Künstler,
  Playlists), einem besten Treffer, letzten Suchanfragen und Blättern; das
  Einfügen eines TIDAL-Links öffnet ihn.
- Seiten für Alben, Künstler, Playlists, Mixe und Radios, mit Credits,
  Rezensionen, Biografien, ähnlichen Künstlern und „Alle anzeigen“-Seiten.
- `tidal://`- und `https://tidal.com/…`-Links öffnen sich in Swell.

**Ihre Sammlung**
- Tausende favorisierte Titel, virtualisiert, mit sofortigem Filtern und
  Sortieren; favorisierte Alben, Künstler, Playlists und Mixe; zuletzt
  gespielt.
- Herzen bei allem (Titel, Alben, Künstler, Playlists, Mixe), mit einer
  einzigen Anfrage für die gesamte Bibliothek.
- Playlists erstellen, umbenennen, beschreiben, veröffentlichen und löschen;
  Titel über ein Menü oder per Ziehen auf eine Playlist in der Seitenleiste
  hinzufügen; Titel entfernen und umsortieren, mit Rückgängig.

**Wiedergabe**
- Hi-Res Lossless (FLAC bis 24 Bit/192 kHz), [bit-perfect](#bit-perfect-wiedergabe).
- Lückenlose Wiedergabe, optionales ReplayGain (Titel oder Album), Auswahl
  des Ausgabegeräts.
- Eine echte Warteschlange: als Nächstes abspielen, zur Warteschlange
  hinzufügen, springen, entfernen, umsortieren, mit Rückgängig leeren;
  Zufallswiedergabe, alle/einen wiederholen, Verlauf.
- Übersteht Netzwerkabbrüche und abgelaufene Stream-URLs und setzt an
  derselben Position fort.
- Synchronisierte Songtexte, die dem Lied folgen und beim Klicken dorthin
  springen.
- Stellt beim Start Ihre Warteschlange und Position wieder her;
  Schlaf-Timer; Desktop-Benachrichtigungen; hält das System während der
  Wiedergabe wach.
- MPRIS unter Linux: Medientasten, Desktop-Widgets und `playerctl`
  funktionieren.

**Oberfläche**
- libadwaita-Design, das dem hellen/dunklen Stil und der Akzentfarbe des
  Systems folgt, mit drei Layouts: breit mit angedocktem Now-Playing-Panel,
  normal und schmal mit Bottom Sheet und Mini-Player.
- Vollständige Tastaturbedienung (Leertaste, Strg+←/→, Strg+F, Strg+L, F6 …;
  Strg+? listet alle Tastenkürzel auf) und zugängliche Namen für jedes
  Bedienelement.
- Verfügbar in elf Sprachen (siehe [Sprachen](#sprachen)).

## Screenshots

| Startseite | Suche |
|---|---|
| ![Startseiten-Feed mit Schnellzugriffen, Mixen und zuletzt Gehörtem](docs/screenshots/home.png) | ![Suchergebnisse für „yoasobi“ mit bestem Treffer, Titeln und Künstlern](docs/screenshots/search.png) |
| **Synchronisierte Songtexte** | **Heller Stil** |
| ![Now-Playing-Panel mit der Warteschlange](docs/screenshots/now-playing.png) | ![Die Albumseite im hellen Stil](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="Schmales Layout mit Mini-Player und unterer Navigationsleiste">
</p>

## Installation

Pakete für jede Version finden Sie auf der
[Releases-Seite](https://github.com/oxakromax/swell-player/releases).
Swell benötigt GTK ≥ 4.18, libadwaita ≥ 1.7 und GStreamer mit den Base- und
Good-Plugins; die Pakete ziehen diese automatisch nach.

### Arch Linux (AUR)

Das AUR-Paket ist in Arbeit. Bis dahin können Sie dieselbe PKGBUILD aus
diesem Repository bauen:

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

Sobald es im AUR ist: `yay -S swell-player` (oder `swell-player-git`).

### Flatpak

Laden Sie das `.flatpak`-Bundle von der Releases-Seite herunter und
installieren Sie es (es verwendet die GNOME-Runtime von Flathub). Ein
Flathub-Eintrag ist geplant.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian und Ubuntu

Erfordert Debian 13 (trixie), Ubuntu 25.04 oder neuer (GTK ≥ 4.18 und
libadwaita ≥ 1.7). Laden Sie die `.deb`-Datei von der Releases-Seite
herunter:

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Laden Sie die `.rpm`-Datei von der Releases-Seite herunter:

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Laden Sie das Installationsprogramm (oder die portable `.zip`-Datei) von der
Releases-Seite herunter. Die Builds sind noch nicht code-signiert, daher
warnt SmartScreen möglicherweise vor einer nicht erkannten App: Wählen Sie
**Weitere Informationen → Trotzdem ausführen**.

### macOS

Laden Sie die `.dmg`-Datei von der Releases-Seite herunter und ziehen Sie
Swell in den Ordner „Programme“. Die App ist noch nicht notarisiert;
klicken Sie sie daher beim ersten Mal mit der rechten Maustaste (oder bei
gedrückter Control-Taste) an und wählen Sie **Öffnen**; unter macOS 15 und
neuer öffnen Sie sie einmal und erlauben sie dann unter **Systemeinstellungen →
Datenschutz & Sicherheit → Trotzdem öffnen**. Da jede Version ad hoc signiert
wird, fragt macOS nach einem Update möglicherweise erneut nach dem Zugriff
auf den Schlüsselbund-Eintrag von Swell: Wählen Sie **Immer erlauben**.
`tidal://`-Links öffnen Swell unter macOS noch nicht, und das
Intel-Image (x86_64) wird nur nach Möglichkeit bereitgestellt, daher kann es
bei einer Version fehlen.

Die Builds für Windows und macOS sind neuer als der für Linux: MPRIS und die
Bit-perfect-Hinweise unten gelten nur für Linux.

### Aus dem Quellcode

Sie benötigen Rust ≥ 1.96 ([rustup](https://rustup.rs), falls die Version
Ihrer Distribution älter ist), GTK ≥ 4.20 und libadwaita ≥ 1.8 (oder GTK 4.18
und libadwaita 1.7 mit `--no-default-features`), GStreamer ≥ 1.24, SQLite und
`glib-compile-resources` von GLib.

<details>
<summary>Build-Abhängigkeiten je Distribution</summary>

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

`gst-plugins-good` stellt den DASH-Demuxer (`dashdemux2`) bereit, der für
Hi-Res-Streams verwendet wird.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` legt eine Installation zum Paketieren
bereit, und `sudo make uninstall` entfernt sie. Die Binärdatei läuft auch
direkt aus `target/release/swell-player`, da Icons und Übersetzungen
eingebettet sind.

### Aktualisierungen

Swell sucht kurz nach dem Start und danach einmal täglich auf GitHub nach
einer neuen Version; unter **Einstellungen → Allgemein → Aktualisierungen**
lässt sich das abschalten oder sofort prüfen. Was mit einer neuen Version
geschieht, hängt davon ab, wie Swell installiert wurde:

| Installiert mit | Neue Version |
|---|---|
| Windows-Installationsprogramm, nur für Sie (Standard) | Wird heruntergeladen, anhand der `SHA256SUMS` der Version geprüft und beim Beenden von Swell installiert, oder sofort mit **Neu starten** |
| macOS-`.dmg` | Genauso, wenn Swell in den Ordner schreiben darf, in dem die App liegt; andernfalls lädt Swell das neue Image herunter und bietet an, es zu öffnen |
| Linux-Pakete, Flatpak, Tarball, portable Windows-`.zip`-Datei, Windows-Installationsprogramm für alle Benutzer | Eine Benachrichtigung mit Link zur Version: Aktualisieren Sie auf dem Weg, auf dem Sie installiert haben |

Distributionen, die Aktualisierungen selbst ausliefern, können Swell ohne
die Prüfungen bauen (siehe
[packaging/README.md](packaging/README.md#update-checks)); Builds, die aus
dem Quellbaum laufen, prüfen nicht.

## Anmeldung und Audioqualität

TIDAL erlaubt Drittanbieter-Apps nicht, eigene OAuth-Clients zu
registrieren. Deshalb meldet sich Swell wie jeder Open-Source-TIDAL-Client
als eine der eigenen Apps von TIDAL an. Der gewählte Anmeldeweg bestimmt,
als welche, und das begrenzt die Qualität:

| Anmeldung | So funktioniert es | Beste Qualität |
|---|---|---|
| **Browser** (empfohlen) | Swell öffnet die Anmeldeseite von TIDAL in Ihrem Browser. Nach der Anmeldung landet TIDAL auf einer Seite mit „Oops“; kopieren Sie die Adresse dieser Seite zurück in Swell. | **Hi-Res Lossless**: FLAC bis 24 Bit/192 kHz |
| **Gerätecode** | Swell zeigt einen kurzen Code an; bestätigen Sie ihn unter `link.tidal.com` auf einem beliebigen Gerät. | **Lossless**: FLAC 16 Bit/44,1 kHz |

Wählen Sie die Streaming-Qualität unter **Einstellungen → Wiedergabe**. Das
Format-Badge in der Player-Leiste zeigt, was tatsächlich geliefert wird.

## Bit-perfect-Wiedergabe

Bei ausgeschalteter Normalisierung und einer Lautstärke von 100 % behält
Swell das Format des Streams bis zu PipeWire bei: Ein Titel mit
24 Bit/96 kHz kommt dort als 24 Bit bei 96 kHz an. Das Format-Badge in der
Player-Leiste öffnet ein Popover, das anzeigt, ob die Ausgabe bit-perfect
ist und, falls nicht, warum.

Danach entscheidet PipeWire, was die Soundkarte erhält. Standardmäßig läuft
es mit einer einzigen Rate (oft 48 kHz) und resampelt alles andere. Damit es
der Rate jedes Titels folgt, erlauben Sie die gängigen Raten, zum Beispiel in
`~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

Starten Sie PipeWire dann neu (`systemctl --user restart pipewire`). `pw-top`
zeigt, was das Gerät erreicht.

## Sprachen

Swell spricht **Englisch**, **Spanisch**, **Französisch**, **Deutsch**,
**Italienisch**, **brasilianisches Portugiesisch**, **Russisch**,
**Japanisch**, **Koreanisch** sowie **vereinfachtes** und **traditionelles
Chinesisch**. Es folgt automatisch der Systemsprache (`LANGUAGE`, `LC_ALL`,
`LC_MESSAGES`, `LANG`, wie gettext), und Sie können dies in den
**Einstellungen** überschreiben. Die eigenen Inhalte von TIDAL (redaktionelle
Texte, Seitentitel) werden in derselben Sprache angefordert, sodass ein
Bildschirm nie zwei Sprachen mischt.

Sie möchten Swell in Ihrer Sprache? Siehe [docs/translating.md](docs/translating.md).

## Datenschutz

- **Tokens** liegen im Schlüsselbund Ihres Systems (unter Linux im Secret
  Service: GNOME Keyring, KWallet, KeePassXC … oder im Secret-Portal bei
  Flatpak), nie im Klartext auf der Festplatte. Sie werden kurz vor ihrem
  Ablauf erneuert, und Netzwerkprobleme melden Sie nie ab.
- **Auf Ihrem Computer zwischengespeichert:** die Antworten von TIDAL und
  Ihre Bibliothek (`~/.cache/swell-player/library.sqlite3`), Cover, die
  ersten Sekunden der gehörten oder als Nächstes anstehenden Titel
  (`~/.cache/swell-player/audio/`, nur Audio, höchstens 512 MB), Ihre
  Einstellungen, die gespeicherte Warteschlange und letzte Suchanfragen. In
  den Einstellungen lässt sich der Cache leeren; beim Abmelden werden die
  Tokens, der Cache, die gespeicherte Warteschlange und der Suchverlauf
  gelöscht.
- **Keine Telemetrie.** Swell verbindet sich nur mit den Servern von TIDAL,
  für das, was Sie anzeigen oder abspielen lassen, und einmal täglich mit
  GitHub, um nach einer neuen Version zu suchen; dabei wird nur Swells
  Versionsnummer gesendet (siehe [Aktualisierungen](#aktualisierungen)). Es
  enthält keine Analyse-, Absturzberichts- oder sonstigen
  Drittanbieterdienste.

## Messungen

Swell im Vergleich zu [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject) auf demselben Rechner, beide in einer headless
KWin-Sitzung (`kwin_wayland --virtual`, 1280×860) mit gespeicherter Sitzung.
Mediane aus drei Läufen; Speicher aus `/proc` gelesen (RssAnon ist der
eigene Heap der App; RSS zählt auch gemeinsam genutzte Bibliotheken und etwa
250 MB GPU-Treiber-Mappings, die jede GTK-4-App hier hat). Die Werte im
Leerlauf sind über 10 s gemittelt, ohne Wiedergabe.

| | Swell | High Tide |
|---|---|---|
| Startseite sichtbar: RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 1.295 favorisierte Titel sichtbar, aus dem Cache | **0,42 s** nach dem Start (synchron mit TIDAL bei 0,50 s) | — ¹ |
| … ohne den Cache von Swell | die ersten 100 nach **0,59 s**, alle nach 1,2–1,9 s | — ¹ |
| Favoritenansicht: RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| CPU im Leerlauf / Wakeups des Hauptthreads | ~0 % / 1,1 pro s | ~0 % / 0,1 pro s |
| Threads | 15 | 19 |
| Hi-Res-Titel gewählt → Audio | **0,67 s** (24 Bit/96 kHz, erreicht PipeWire als S24_32 bei 96 kHz) | — |

¹ High Tide speichert Favoriten nicht im Cache und hat keine Möglichkeit,
seine Favoritenliste direkt zu öffnen; mit einem Testaufbau maß der
Proof of Concept von Swell 13,3 s, bis High Tide dieselbe Liste anzeigte,
gegenüber 0,3 s bei Swell.

Rechner: Arch Linux, KDE Plasma 6 (Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, Release-Build.

## Entwicklung

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) behandelt das Bauen, Testen, die
Umgebungsvariablen zum Debuggen (eine Seite öffnen, einen Titel abspielen
oder das Fenster ohne Display in ein PNG rendern) und wie man Änderungen
einreicht. [docs/architecture.md](docs/architecture.md) erklärt, wie der
Code aufgebaut ist; [docs/features.md](docs/features.md) beschreibt den
Funktionsumfang und [docs/design.md](docs/design.md) die
Oberflächenspezifikation (auf Spanisch). Die Versionshinweise stehen in
[CHANGELOG.md](CHANGELOG.md).

Sicherheitsproblem gefunden? Bitte melden Sie es privat: siehe
[SECURITY.md](SECURITY.md).

## Lizenz

Swell ist freie Software: Sie können es unter den Bedingungen der GNU
General Public License, wie von der Free Software Foundation veröffentlicht,
weitergeben und/oder ändern, entweder gemäß Version 3 der Lizenz oder (nach
Ihrer Wahl) jeder späteren Version. Siehe [COPYING](COPYING).

TIDAL ist eine Marke ihres Inhabers und wird hier nur verwendet, um zu
sagen, womit Swell funktioniert.
