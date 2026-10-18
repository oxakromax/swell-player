<div align="center">

[English](README.md) · [Español](README.es.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · **Italiano** · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [简体中文](README.zh_CN.md) · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**Un client TIDAL nativo e veloce, con riproduzione Hi-Res bit-perfect.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell che mostra un album, con il pannello di riproduzione ancorato a destra](docs/screenshots/album.png)

</div>

Swell è scritto in Rust con [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) e
[GStreamer](https://gstreamer.freedesktop.org/). La tua libreria si apre
all’istante da una cache locale, il FLAC Hi-Res arriva al server audio
senza essere toccato e l’interfaccia si adatta dalle finestre grandi come
un telefono ai desktop più ampi, usando una frazione della memoria degli
altri client ([misurazioni](#misurazioni)).

> [!NOTE]
> Swell non è affiliato a TIDAL, né approvato o supportato da TIDAL. Usa le
> API non ufficiali di TIDAL, che possono cambiare senza preavviso, e
> richiede un abbonamento a TIDAL a pagamento.

## Indice

- [Funzionalità](#funzionalità)
- [Schermate](#schermate)
- [Installazione](#installazione)
- [Accesso e qualità audio](#accesso-e-qualità-audio)
- [Riproduzione bit-perfect](#riproduzione-bit-perfect)
- [Lingue](#lingue)
- [Privacy](#privacy)
- [Misurazioni](#misurazioni)
- [Sviluppo](#sviluppo)
- [Licenza](#licenza)

## Funzionalità

**Esplorare**
- Home: il tuo feed TIDAL personalizzato (scorciatoie, mix, consigli,
  riprodotti di recente), che ne carica altri mentre scorri. Esplora: pagine
  editoriali, generi, atmosfere e decenni.
- Ricerca mentre scrivi, con filtri (brani, album, artisti, playlist), un
  risultato principale, ricerche recenti e paginazione; incollando un
  collegamento TIDAL lo apri.
- Pagine di album, artisti, playlist, mix e radio, con crediti, recensioni,
  biografie, artisti simili e pagine “Mostra tutto”.
- I collegamenti `tidal://` e `https://tidal.com/…` si aprono in Swell.

**La tua collezione**
- Migliaia di brani preferiti, virtualizzati, con filtro e ordinamento
  istantanei; album, artisti, playlist e mix preferiti; riprodotti di
  recente.
- Cuori su tutto (brani, album, artisti, playlist, mix), con una sola
  richiesta per l’intera libreria.
- Crea, rinomina, descrivi, pubblica ed elimina playlist; aggiungi brani da
  un menu o trascinandoli su una playlist della barra laterale; rimuovili e
  riordinali, con annullamento.

**Riproduzione**
- Hi-Res Lossless (FLAC fino a 24 bit/192 kHz), [bit-perfect](#riproduzione-bit-perfect).
- Riproduzione senza interruzioni, ReplayGain facoltativo (brano o album),
  scelta del dispositivo di uscita.
- Una vera coda: riproduci dopo, aggiungi alla coda, salta, rimuovi,
  riordina, svuota con annullamento; riproduzione casuale, ripeti tutto/uno,
  cronologia.
- Regge alle cadute di rete e agli URL di stream scaduti, riprendendo dalla
  stessa posizione.
- Testi sincronizzati che seguono il brano e permettono di spostarsi con un
  clic.
- Ripristina la coda e la posizione all’avvio; timer di spegnimento;
  notifiche del desktop; impedisce la sospensione del sistema durante la
  riproduzione.
- MPRIS su Linux: funzionano i tasti multimediali, i widget del desktop e
  `playerctl`.

**Interfaccia**
- Design libadwaita che segue lo stile chiaro/scuro e il colore d’accento
  del sistema, con tre disposizioni: ampia con un pannello di riproduzione
  ancorato, normale e stretta con un pannello inferiore e un mini player.
- Pieno controllo da tastiera (Spazio, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…; Ctrl+?
  elenca tutte le scorciatoie) e nomi accessibili su ogni controllo.
- Inglese, spagnolo, francese, tedesco, italiano, portoghese (Brasile),
  russo, giapponese, coreano e cinese (semplificato e tradizionale).

## Schermate

| Home | Ricerca |
|---|---|
| ![Feed della Home con scorciatoie, mix e riprodotti di recente](docs/screenshots/home.png) | ![Risultati della ricerca per “yoasobi” con un risultato principale, brani e artisti](docs/screenshots/search.png) |
| **Testi sincronizzati** | **Stile chiaro** |
| ![Pannello di riproduzione che mostra la coda](docs/screenshots/now-playing.png) | ![La pagina di un album nello stile chiaro](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="Disposizione stretta con il mini player e la barra di navigazione inferiore">
</p>

## Installazione

I pacchetti di ogni versione si trovano nella
[pagina delle release](https://github.com/oxakromax/swell-player/releases).
Swell richiede GTK ≥ 4.18, libadwaita ≥ 1.7 e GStreamer con i plugin base e
good; i pacchetti li installano automaticamente.

### Arch Linux (AUR)

Il pacchetto AUR sta arrivando. Nel frattempo, compila lo stesso PKGBUILD da
questo repository:

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

Quando sarà su AUR: `yay -S swell-player` (o `swell-player-git`).

### Flatpak

Scarica il pacchetto `.flatpak` dalla pagina delle release e installalo
(usa il runtime GNOME di Flathub). È prevista una scheda su Flathub.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian e Ubuntu

Richiede Debian 13 (trixie), Ubuntu 25.04 o più recente (GTK ≥ 4.18 e
libadwaita ≥ 1.7). Scarica il `.deb` dalla pagina delle release:

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Scarica l’`.rpm` dalla pagina delle release:

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Scarica il programma di installazione (o lo `.zip` portabile) dalla pagina
delle release. Le build non sono ancora firmate, quindi SmartScreen può
avvisare di un’app non riconosciuta: scegli **Ulteriori informazioni →
Esegui comunque**.

### macOS

Scarica il `.dmg` dalla pagina delle release e trascina Swell in
Applicazioni. L’app non è ancora notarizzata, quindi la prima volta fai clic
con il tasto destro (o Control-clic) e scegli **Apri**; su macOS 15 e
successivi, aprila una volta, poi consentila in **Impostazioni di Sistema →
Privacy e sicurezza → Apri comunque**. Poiché ogni release è firmata ad hoc,
dopo un aggiornamento macOS può chiedere di nuovo l’accesso all’elemento del
portachiavi di Swell: scegli **Consenti sempre**. I collegamenti `tidal://`
non aprono ancora Swell su macOS, e l’immagine disco Intel (x86_64) viene
fornita al meglio delle possibilità, quindi una release potrebbe non averla.

Le build per Windows e macOS sono più recenti di quella per Linux: MPRIS e
le note sul bit-perfect qui sotto valgono solo per Linux.

### Dai sorgenti

Ti servono Rust ≥ 1.96 ([rustup](https://rustup.rs) se quello della tua
distribuzione è più vecchio), GTK ≥ 4.20 e libadwaita ≥ 1.8 (oppure GTK 4.18
e libadwaita 1.7 con `--no-default-features`), GStreamer ≥ 1.24, SQLite e
`glib-compile-resources` di GLib.

<details>
<summary>Dipendenze di compilazione per distribuzione</summary>

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

`gst-plugins-good` fornisce il demuxer DASH (`dashdemux2`) usato per gli
stream Hi-Res.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` prepara un’installazione per il
packaging, e `sudo make uninstall` la rimuove. Il binario funziona anche
direttamente da `target/release/swell-player`, poiché icone e traduzioni
sono incorporate.

## Accesso e qualità audio

TIDAL non permette alle app di terze parti di registrare i propri client
OAuth, quindi Swell, come ogni client TIDAL open source, accede presentandosi
come una delle app di TIDAL. Il tipo di accesso che scegli determina quale, e
di conseguenza il limite di qualità:

| Accesso | Come funziona | Qualità massima |
|---|---|---|
| **Browser** (consigliato) | Swell apre la pagina di accesso di TIDAL nel browser. Dopo l’accesso, TIDAL arriva su una pagina che dice “Oops”; copia l’indirizzo di quella pagina in Swell. | **Hi-Res Lossless**: FLAC fino a 24 bit/192 kHz |
| **Codice dispositivo** | Swell mostra un breve codice; approvalo su `link.tidal.com` da qualsiasi dispositivo. | **Lossless**: FLAC 16 bit/44,1 kHz |

Scegli la qualità dello streaming in **Preferenze → Riproduzione**. Il badge
del formato nella barra del lettore mostra ciò che viene effettivamente
fornito.

## Riproduzione bit-perfect

Con la normalizzazione disattivata e il volume al 100 %, Swell mantiene il
formato dello stream fino a PipeWire: un brano a 24 bit/96 kHz gli arriva a
24 bit e 96 kHz. Il badge del formato nella barra del lettore apre un
popover che dice se l’uscita è bit-perfect e, se non lo è, perché.

Poi è PipeWire a decidere cosa riceve la scheda audio. Per impostazione
predefinita funziona a un’unica frequenza (spesso 48 kHz) e ricampiona tutto
il resto. Perché segua la frequenza di ogni brano, consenti le frequenze più
comuni, per esempio in
`~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

poi riavvia PipeWire (`systemctl --user restart pipewire`). `pw-top` mostra
cosa arriva al dispositivo.

## Lingue

Swell parla **inglese**, **spagnolo**, **francese**, **tedesco**,
**italiano**, **portoghese (Brasile)**, **russo**, **giapponese**,
**coreano** e **cinese** (semplificato e tradizionale). Segue
automaticamente la lingua del sistema (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`,
`LANG`, come gettext), e puoi cambiarla nelle **Preferenze**. Anche i
contenuti di TIDAL (testi editoriali, titoli delle pagine) vengono richiesti
nella stessa lingua, così una schermata non mescola mai due lingue.

Vuoi Swell nella tua lingua? Vedi [docs/translating.md](docs/translating.md).

## Privacy

- I **token** risiedono nel portachiavi del sistema (su Linux, il Secret
  Service: GNOME Keyring, KWallet, KeePassXC…, o il portale dei segreti in
  Flatpak), mai su disco in chiaro. Vengono rinnovati poco prima della
  scadenza, e i problemi di rete non ti disconnettono mai.
- **In cache sul tuo computer:** le risposte di TIDAL e la tua libreria
  (`~/.cache/swell-player/library.sqlite3`), le copertine, i primi secondi
  dei brani ascoltati o in arrivo (`~/.cache/swell-player/audio/`, solo
  audio, al massimo 512 MB), le impostazioni, la coda salvata e le ricerche
  recenti. Le preferenze permettono di svuotare la cache; la disconnessione
  elimina i token, la cache, la coda salvata e la cronologia delle ricerche.
- **Nessuna telemetria.** Swell si collega solo ai server di TIDAL, per ciò
  che gli chiedi di mostrare o riprodurre. Non ha analisi d’uso, segnalazione
  dei crash né altri servizi di terze parti.

## Misurazioni

Swell a confronto con [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject) sulla stessa macchina, entrambi in una sessione KWin
headless (`kwin_wayland --virtual`, 1280×860) con una sessione salvata.
Mediane di tre esecuzioni; memoria letta da `/proc` (RssAnon è l’heap
dell’app stessa; RSS conta anche le librerie condivise e circa 250 MB di
mappature del driver GPU che qualsiasi app GTK 4 ha qui). I valori a riposo
sono medie su 10 s senza riproduzione.

| | Swell | High Tide |
|---|---|---|
| Home a schermo: RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 1.295 brani preferiti a schermo, dalla cache | **0,42 s** dopo l’avvio (sincronizzato con TIDAL a 0,50 s) | — ¹ |
| … senza la cache di Swell | i primi 100 in **0,59 s**, tutti in 1,2–1,9 s | — ¹ |
| Vista dei preferiti: RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| CPU a riposo / risvegli del thread principale | ~0 % / 1,1 al s | ~0 % / 0,1 al s |
| Thread | 15 | 19 |
| Brano Hi-Res scelto → audio | **0,67 s** (24 bit/96 kHz, arriva a PipeWire come S24_32 a 96 kHz) | — |

¹ High Tide non mette in cache i preferiti e non ha modo di aprire
direttamente la sua lista dei preferiti; con un banco di prova, la prova di
concetto di Swell ha misurato 13,3 s perché High Tide mostrasse la stessa
lista, contro 0,3 s per Swell.

Macchina: Arch Linux, KDE Plasma 6 (Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, build release.

## Sviluppo

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) spiega come compilare, testare, le
variabili d’ambiente di debug (aprire una pagina, riprodurre un brano o
renderizzare la finestra in un PNG senza display) e come inviare modifiche.
[docs/architecture.md](docs/architecture.md) spiega come è organizzato il
codice; [docs/features.md](docs/features.md) descrive l’ambito delle
funzionalità e [docs/design.md](docs/design.md) la specifica
dell’interfaccia (in spagnolo). Le note di rilascio sono in
[CHANGELOG.md](CHANGELOG.md).

Hai trovato un problema di sicurezza? Segnalalo in privato: vedi
[SECURITY.md](SECURITY.md).

## Licenza

Swell è software libero: puoi ridistribuirlo e/o modificarlo secondo i
termini della GNU General Public License come pubblicata dalla Free Software
Foundation, nella versione 3 della licenza o (a tua scelta) in qualsiasi
versione successiva. Vedi [COPYING](COPYING).

TIDAL è un marchio del suo proprietario, usato qui solo per indicare con che
cosa funziona Swell.
