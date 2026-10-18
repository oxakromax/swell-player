<div align="center">

[English](README.md) · **Español** · [Français](README.fr.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Português (Brasil)](README.pt_BR.md) · [Русский](README.ru.md) · [日本語](README.ja.md) · [한국어](README.ko.md) · [简体中文](README.zh_CN.md) · [繁體中文](README.zh_TW.md)

<img src="data/icons/scalable/apps/io.github.oxakromax.SwellPlayer.svg" width="112" alt="">

# Swell

**Un cliente de TIDAL rápido y nativo, con reproducción Hi-Res bit-perfect.**

[![CI](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml/badge.svg)](https://github.com/oxakromax/swell-player/actions/workflows/ci.yml)
[![Última versión](https://img.shields.io/github/v/release/oxakromax/swell-player)](https://github.com/oxakromax/swell-player/releases/latest)
[![Licencia: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](COPYING)

![Swell mostrando un álbum, con el panel de reproducción acoplado a la derecha](docs/screenshots/album.png)

</div>

Swell está escrito en Rust con [gtk4-rs](https://gtk-rs.org),
[libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/) y
[GStreamer](https://gstreamer.freedesktop.org/). Tu biblioteca se abre al
instante desde una caché local, el FLAC Hi-Res llega intacto al servidor de
sonido, y la interfaz se adapta desde ventanas del tamaño de un teléfono
hasta escritorios anchos, con una fracción de la memoria de otros clientes
([mediciones](#mediciones)).

> [!NOTE]
> Swell no está afiliado a TIDAL ni cuenta con su respaldo o soporte. Usa
> las API no oficiales de TIDAL, que pueden cambiar sin aviso, y necesita una
> suscripción de pago a TIDAL.

## Contenido

- [Funciones](#funciones)
- [Capturas de pantalla](#capturas-de-pantalla)
- [Instalación](#instalación)
- [Inicio de sesión y calidad de audio](#inicio-de-sesión-y-calidad-de-audio)
- [Reproducción bit-perfect](#reproducción-bit-perfect)
- [Idiomas](#idiomas)
- [Privacidad](#privacidad)
- [Mediciones](#mediciones)
- [Desarrollo](#desarrollo)
- [Licencia](#licencia)

## Funciones

**Explorar**
- Inicio: tu portada personalizada de TIDAL (accesos directos, mixes,
  recomendaciones, escuchado recientemente), que carga más a medida que te
  desplazas. Explorar: páginas editoriales, géneros, estados de ánimo y
  décadas.
- Búsqueda mientras escribes, con filtros (canciones, álbumes, artistas,
  playlists), un resultado destacado, búsquedas recientes y paginación;
  pegar un enlace de TIDAL lo abre.
- Páginas de álbumes, artistas, playlists, mixes y radios, con créditos,
  reseñas, biografías, artistas similares y páginas «Ver todo».
- Los enlaces `tidal://` y `https://tidal.com/…` se abren en Swell.

**Tu colección**
- Miles de canciones favoritas, virtualizadas, con filtro y orden
  instantáneos; álbumes, artistas, playlists y mixes favoritos; escuchado
  recientemente.
- Corazones en todo (canciones, álbumes, artistas, playlists, mixes), con
  una sola petición para toda la biblioteca.
- Crea, renombra, describe, publica y elimina playlists; añade canciones
  desde un menú o arrastrándolas a una playlist de la barra lateral;
  quítalas y reordénalas, con deshacer.

**Reproducción**
- Hi-Res Lossless (FLAC hasta 24 bits/192 kHz), [bit-perfect](#reproducción-bit-perfect).
- Reproducción sin pausas, ReplayGain opcional (por canción o por álbum),
  elección del dispositivo de salida.
- Una cola de verdad: reproducir a continuación, añadir a la cola, saltar,
  quitar, reordenar, vaciar con deshacer; aleatorio, repetir todo/una,
  historial.
- Resiste cortes de red y URL de streaming caducadas, y retoma en la misma
  posición.
- Letras sincronizadas que siguen la canción y saltan a ese punto al hacer
  clic.
- Restaura la cola y la posición al iniciar; temporizador; notificaciones
  del escritorio; mantiene el sistema despierto mientras reproduce.
- MPRIS en Linux: las teclas multimedia, los widgets del escritorio y
  `playerctl` funcionan.

**Interfaz**
- Diseño libadwaita que sigue el estilo claro u oscuro y el color de acento
  del sistema, con tres diseños: ancho con el panel de reproducción
  acoplado, normal, y estrecho con una hoja inferior y un minirreproductor.
- Control total por teclado (Espacio, Ctrl+←/→, Ctrl+F, Ctrl+L, F6…; Ctrl+?
  lista todos los atajos) y nombres accesibles en todos los controles.
- En inglés, español, francés, alemán, italiano, portugués de Brasil, ruso,
  japonés, coreano y chino (simplificado y tradicional).

## Capturas de pantalla

| Inicio | Búsqueda |
|---|---|
| ![Portada de Inicio con accesos directos, mixes y escuchado recientemente](docs/screenshots/home.png) | ![Resultados de búsqueda de "yoasobi" con un resultado destacado, canciones y artistas](docs/screenshots/search.png) |
| **Letras sincronizadas** | **Estilo claro** |
| ![Panel de reproducción mostrando la cola](docs/screenshots/now-playing.png) | ![La página de un álbum en el estilo claro](docs/screenshots/light.png) |

<p align="center">
  <img src="docs/screenshots/narrow.png" width="300" alt="Diseño estrecho con el minirreproductor y la barra de navegación inferior">
</p>

## Instalación

Los paquetes de cada versión están en la
[página de Releases](https://github.com/oxakromax/swell-player/releases).
Swell necesita GTK ≥ 4.18, libadwaita ≥ 1.7 y GStreamer con los plugins
base y good; los paquetes los instalan como dependencias.

### Arch Linux (AUR)

El paquete de AUR está en camino. Mientras tanto, compila el mismo PKGBUILD
desde este repositorio:

```bash
git clone https://github.com/oxakromax/swell-player.git
cd swell-player/packaging/aur/swell-player   # or swell-player-git
makepkg -si
```

Cuando esté en AUR: `yay -S swell-player` (o `swell-player-git`).

### Flatpak

Descarga el paquete `.flatpak` de la página de Releases e instálalo (usa el
runtime de GNOME de Flathub). Está previsto publicarlo en Flathub.

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./swell-player-*.flatpak
flatpak run io.github.oxakromax.SwellPlayer
```

### Debian y Ubuntu

Requiere Debian 13 (trixie), Ubuntu 25.04 o posterior (GTK ≥ 4.18 y
libadwaita ≥ 1.7). Descarga el `.deb` de la página de Releases:

```bash
sudo apt install ./swell-player_*.deb
```

### Fedora

Descarga el `.rpm` de la página de Releases:

```bash
sudo dnf install ./swell-player-*.rpm
```

### Windows

Descarga el instalador (o el `.zip` portable) de la página de Releases.
Las compilaciones aún no están firmadas, así que SmartScreen puede advertir
de una aplicación no reconocida: elige **Más información → Ejecutar de
todas formas**.

### macOS

Descarga el `.dmg` de la página de Releases y arrastra Swell a
Aplicaciones. La aplicación aún no está notarizada, así que la primera vez
haz clic derecho (o Control-clic) sobre ella y elige **Abrir**; en macOS 15
y posteriores, ábrela una vez y luego permítela en **Ajustes del Sistema →
Privacidad y seguridad → Abrir igualmente**. Como cada versión se firma ad
hoc, macOS puede volver a pedir acceso al elemento de Swell en el Llavero
tras una actualización: elige **Permitir siempre**. Los enlaces `tidal://`
aún no abren Swell en macOS, y la imagen de disco para Intel (x86_64) se
ofrece sin garantías, así que puede faltar en alguna versión.

Las compilaciones para Windows y macOS son más recientes que la de Linux:
MPRIS y las notas sobre bit-perfect de más abajo son específicas de Linux.

### Desde el código fuente

Necesitas Rust ≥ 1.96 ([rustup](https://rustup.rs) si el de tu
distribución es más antiguo), GTK ≥ 4.20 y libadwaita ≥ 1.8 (o GTK 4.18 y
libadwaita 1.7 con `--no-default-features`), GStreamer ≥ 1.24, SQLite y
`glib-compile-resources` de GLib.

<details>
<summary>Dependencias de compilación por distribución</summary>

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

`gst-plugins-good` aporta el demultiplexor DASH (`dashdemux2`) que se usa
para los streams Hi-Res.

</details>

```bash
make                  # cargo build --release --locked
sudo make install     # binary, desktop entry, icon, metainfo under /usr/local
```

`make install PREFIX=/usr DESTDIR=…` prepara una instalación para
empaquetar, y `sudo make uninstall` la elimina. El binario también funciona
directamente desde `target/release/swell-player`, ya que los iconos y las
traducciones van incluidos.

## Inicio de sesión y calidad de audio

TIDAL no permite que las aplicaciones de terceros registren sus propios
clientes OAuth, así que Swell, como todo cliente de TIDAL de código
abierto, inicia sesión como una de las aplicaciones de TIDAL. El método de
inicio de sesión que elijas decide cuál, y eso limita la calidad:

| Inicio de sesión | Cómo funciona | Mejor calidad |
|---|---|---|
| **Navegador** (recomendado) | Swell abre la página de inicio de sesión de TIDAL en tu navegador. Tras iniciar sesión, TIDAL te lleva a una página que dice "Oops"; copia la dirección de esa página de vuelta en Swell. | **Hi-Res Lossless**: FLAC hasta 24 bits/192 kHz |
| **Código de dispositivo** | Swell muestra un código corto; apruébalo en `link.tidal.com` desde cualquier dispositivo. | **Lossless**: FLAC 16 bits/44,1 kHz |

Elige la calidad de streaming en **Preferencias → Reproducción**. La
etiqueta de formato de la barra del reproductor muestra lo que realmente
se está recibiendo.

## Reproducción bit-perfect

Con la normalización desactivada y el volumen al 100 %, Swell mantiene el
formato del stream hasta PipeWire: una canción de 24 bits/96 kHz le llega
como 24 bits a 96 kHz. La etiqueta de formato de la barra del reproductor
abre un popover que indica si la salida es bit-perfect y, si no, por qué.

Luego PipeWire decide qué recibe la tarjeta de sonido. Por defecto funciona
a una sola frecuencia (a menudo 48 kHz) y remuestrea todo lo demás. Para
que siga la frecuencia de cada canción, permite las frecuencias habituales,
por ejemplo en `~/.config/pipewire/pipewire.conf.d/10-rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

y reinicia PipeWire (`systemctl --user restart pipewire`). `pw-top` muestra
lo que llega al dispositivo.

## Idiomas

Swell habla **inglés**, **español**, **francés**, **alemán**,
**italiano**, **portugués de Brasil**, **ruso**, **japonés**, **coreano** y
**chino** (simplificado y tradicional). Sigue automáticamente el idioma del
sistema (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, como gettext), y
puedes cambiarlo en **Preferencias**. El contenido propio de TIDAL (textos
editoriales, títulos de páginas) se pide en el mismo idioma, para que una
pantalla nunca mezcle dos.

¿Quieres Swell en tu idioma? Consulta [docs/translating.md](docs/translating.md).

## Privacidad

- **Los tokens** se guardan en el llavero del sistema (en Linux, el Secret
  Service: GNOME Keyring, KWallet, KeePassXC…, o el portal de secretos en
  Flatpak), nunca en el disco como texto plano. Se renuevan poco antes de
  caducar, y los problemas de red nunca cierran tu sesión.
- **Guardado en tu equipo:** las respuestas de TIDAL y tu biblioteca
  (`~/.cache/swell-player/library.sqlite3`), las portadas, los primeros
  segundos de las canciones que escuchaste o ibas a escuchar
  (`~/.cache/swell-player/audio/`, solo audio, 512 MB como máximo), tus
  ajustes, la cola guardada y las búsquedas recientes. Preferencias permite
  vaciar la caché; cerrar sesión borra los tokens, la caché, la cola
  guardada y el historial de búsqueda.
- **Sin telemetría.** Swell solo se conecta a los servidores de TIDAL, para
  lo que le pides mostrar o reproducir. No tiene analíticas, informes de
  fallos ni otros servicios de terceros.

## Mediciones

Swell frente a [High Tide](https://github.com/Nokse22/high-tide) 1.6
(Python + PyGObject) en la misma máquina, ambos en una sesión de KWin sin
pantalla (`kwin_wayland --virtual`, 1280×860) con una sesión iniciada.
Medianas de tres ejecuciones; memoria leída de `/proc` (RssAnon es el heap
propio de la aplicación; RSS también cuenta las bibliotecas compartidas y
unos 250 MB de mapeos del controlador de la GPU que tiene aquí cualquier
aplicación GTK 4). Las cifras en reposo son promedios de 10 s sin
reproducir nada.

| | Swell | High Tide |
|---|---|---|
| Inicio en pantalla: RssAnon / RSS | **61 MB / 360 MB** | 131 MB / 504 MB |
| 1295 canciones favoritas en pantalla, desde la caché | **0,42 s** tras el arranque (sincronizadas con TIDAL a los 0,50 s) | — ¹ |
| … sin la caché de Swell | las primeras 100 a los **0,59 s**, todas entre 1,2 y 1,9 s | — ¹ |
| Vista de favoritos: RssAnon / RSS | 85 MB / 371 MB | — ¹ |
| CPU en reposo / activaciones del hilo principal | ~0 % / 1,1 por s | ~0 % / 0,1 por s |
| Hilos | 15 | 19 |
| Canción Hi-Res elegida → audio | **0,67 s** (24 bits/96 kHz, llega a PipeWire como S24_32 a 96 kHz) | — |

¹ High Tide no guarda los favoritos en caché ni tiene forma de abrir
directamente su lista de favoritos; con un arnés de pruebas, la prueba de
concepto de Swell midió 13,3 s para que High Tide mostrara la misma lista,
frente a 0,3 s de Swell.

Máquina: Arch Linux, KDE Plasma 6 (Wayland), GTK 4.24, libadwaita 1.10,
GStreamer 1.28, Rust 1.99, compilación release.

## Desarrollo

```bash
cargo run --release
cargo test --release
cargo clippy --release --all-targets
```

[CONTRIBUTING.md](CONTRIBUTING.md) explica cómo compilar y probar, las
variables de entorno de depuración (abrir una página, reproducir una
canción o guardar la ventana como PNG sin pantalla) y cómo enviar cambios.
[docs/architecture.md](docs/architecture.md) explica cómo está organizado
el código; [docs/features.md](docs/features.md) es el alcance de las
funciones y [docs/design.md](docs/design.md) la especificación de la
interfaz (en español). Las notas de versión están en
[CHANGELOG.md](CHANGELOG.md).

¿Encontraste un problema de seguridad? Infórmalo de forma privada: consulta
[SECURITY.md](SECURITY.md).

## Licencia

Swell es software libre: puedes redistribuirlo y/o modificarlo según los
términos de la Licencia Pública General de GNU publicada por la Free
Software Foundation, ya sea la versión 3 de la Licencia o (a tu elección)
cualquier versión posterior. Consulta [COPYING](COPYING).

TIDAL es una marca registrada de su propietario, usada aquí solo para decir
con qué funciona Swell.
