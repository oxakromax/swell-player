# Swell: especificación de diseño de la interfaz

Documento de diseño para implementar directamente en gtk4-rs 0.11 (API GTK
4.20) y libadwaita 0.9 (API hasta 1.8). Cubre todo `docs/features.md`. Los
textos de interfaz van en inglés (idioma fuente de la app, traducible con
gettext); la prosa del documento, en español.

> **Dependencias.** `Cargo.toml` activa hoy `adw/v1_7` y `gtk/v4_18`. Este
> diseño usa `AdwShortcutsDialog` (1.8): hay que subir a `adw/v1_8` y
> `gtk/v4_20`. No se usa nada de libadwaita 1.9 (por ejemplo `AdwSidebar`);
> cuando se suba el mínimo, la barra lateral puede migrar a ella sin cambiar el
> diseño.

---

## 0. Qué hacemos mejor que High Tide

| High Tide | Swell | Por qué |
|---|---|---|
| Sin barra lateral; navegación con 3 toggles abajo | Barra lateral con secciones y las playlists del usuario; `AdwViewSwitcherBar` en estrecho | Reconocer antes que recordar: el destino está a la vista |
| `GtkListBox` para listas de pistas (no virtualizadas) | `gtk::ListView`/`GridView` con filas de altura fija | Rendimiento con 5 000 favoritos |
| Menú contextual solo en pistas | Menú en todo objeto: pista, álbum, playlist, artista, mix, fila de cola | Consistencia y estándares |
| Cola sin saltar/quitar/reordenar | Clic para saltar, ✕ por fila, arrastrar + Alt+↑/↓ | Control y libertad del usuario |
| Búsqueda solo con Enter | Búsqueda en vivo con filtros por tipo | Visibilidad del estado; menos pasos |
| `style-dark.css` con colores fijos (`#888`, `#444`) | Solo variables CSS de libadwaita | Funciona en claro/oscuro y con el acento del sistema |
| Volumen en pasos del 10 %, sin nombres accesibles | Pasos del 5 %, todos los botones de icono con nombre accesible | WCAG 4.1.2 |

---

## 1. Principios y lenguaje visual

### 1.1 Principios

1. **El contenido es la interfaz.** Portadas y títulos llevan la jerarquía;
   el cromo (bordes, sombras, colores) se mantiene al mínimo (diseño estético
   y minimalista).
2. **Un solo reproductor, siempre visible.** La reproducción es global y nunca
   se tapa (barra inferior o mini reproductor).
3. **Todo objeto es accionable de la misma forma.** Clic o Enter abre o
   reproduce; clic derecho, pulsación larga, Shift+F10 o tecla Menú abren el
   mismo menú contextual (consistencia y estándares).
4. **Rápido antes que bonito.** Ningún efecto que requiera dibujo propio o
   widgets pesados por fila. Animaciones solo de opacidad y `transform`.
5. **Nativo en GNOME y Plasma.** Tema claro/oscuro y acento del sistema por el
   portal de ajustes, botones de ventana según `gtk-decoration-layout`, fuente
   del sistema (nunca se fija `font-family`).

### 1.2 Escala de espaciado (múltiplos de 6)

| Token | px | Uso |
|---|---|---|
| `xs` | 6 | Dentro de una molécula (icono↔etiqueta, título↔badge, líneas de texto) |
| `s` | 12 | Entre moléculas (portada↔texto de fila, botones de acción, huecos de carrusel/rejilla) |
| `m` | 18 | Margen interno de cabeceras de página en estrecho |
| `l` | 24 | Margen lateral de página (ancho), separación cabecera de página → contenido |
| `xl` | 36 | Separación vertical entre secciones (carruseles) |
| `xxl` | 48 | Margen superior de páginas de estado y del login |

Margen lateral de página: 24 px (ancho) y 12 px (estrecho), aplicado por CSS
con la clase `narrow` en la ventana (§1.8).

### 1.3 Escala tipográfica (clases de libadwaita)

| Rol | Clase | Notas |
|---|---|---|
| Título de cabecera de página | `title-1` (ancho) / `title-2` (estrecho) | Máximo 2 líneas, `wrap`, `ellipsize end`, `lines 2` |
| Sobretítulo de tipo ("ALBUM", "PLAYLIST") | `caption-heading` + `dimmed` | En mayúsculas en el texto |
| Título de sección / carrusel | `title-3` | |
| Título de tarjeta | `heading` | 1 línea, tooltip con el texto completo |
| Subtítulo de tarjeta | `caption` + `dimmed` | 1 línea |
| Título de fila de pista | (cuerpo, sin clase) | Regular; acento si suena |
| Línea secundaria de fila | `dimmed` | Tamaño de cuerpo: `caption`+`dimmed` sería demasiado poco contraste |
| Duraciones, tiempos, números de pista, contadores | `numeric` (+ `dimmed`) | Dígitos tabulares: las columnas no bailan |
| Título en "Now playing" | `title-2` | |
| Líneas de letra | `title-3` | |
| Metadatos de cabecera | `dimmed` | |

`dimmed` es el nombre de libadwaita 1.7+; `dim-label` sigue funcionando pero
no se usa en código nuevo.

### 1.4 Radios de esquina

| Radio | Dónde |
|---|---|
| 4 px | Badges de calidad/explícito |
| 6 px | Portadas ≤ 56 px (filas, cola, mini reproductor, barra del reproductor, barra lateral), barras de skeleton |
| 12 px | Tarjetas y sus portadas, portada de cabecera, portada de "Now playing", líneas de letra al pasar el ratón |
| círculo | Artistas siempre (`AdwAvatar`) |

Las portadas con radio necesitan `set_overflow(gtk::Overflow::Hidden)` en el
widget, además de la clase CSS.

### 1.5 Tamaños de portada por contexto

Tamaño lógico mostrado → tamaño de descarga. Regla: `px = ceil(lógico ×
escala)` (escala fraccional de la superficie en Wayland); se pide el tamaño
del servidor de imágenes de TIDAL más pequeño que sea ≥ `px` entre
80/160/320/640 (1280 solo para álbumes, con escala ≥ 1,5 y lógico > 320). Las
tablas por tipo de objeto (álbum, artista, playlist, mix tienen tamaños
distintos en TIDAL) viven en `covers.rs`.

| Contexto | Lógico (ancho / estrecho) | Descarga a 1× / 2× |
|---|---|---|
| Fila de barra lateral (playlist) | 32 | 80 / 80 |
| Fila de pista con portada | 48 | 80 / 160 |
| Fila de cola, mini reproductor | 42 | 80 / 160 |
| Barra del reproductor | 56 | 80 / 160 |
| Atajo de Home (tile horizontal) | 56 | 80 / 160 |
| Top hit de búsqueda | 120 | 160 / 320 |
| Tarjeta (carrusel, rejilla) | 160 / 144 | 160 / 320 |
| Cabecera de página (álbum, playlist, mix) | 240 / 180 | 320 / 640 |
| Cabecera de artista (`AdwAvatar`) | 200 / 144 | 320 / 640 |
| Now playing | fluido, máx. 480 | 640 / 1280 |

Placeholder mientras carga: icono simbólico (`audio-x-generic-symbolic` para
pistas/álbumes, `view-list-symbolic` para playlists) centrado sobre el fondo
`.skeleton` estático; artistas: iniciales de `AdwAvatar`.

### 1.6 Uso del color

- **Acento** (`--accent-color` para texto/iconos, `--accent-bg-color` para
  fondos): solo para (a) la acción primaria de cada vista (Play,
  `suggested-action`), (b) el estado "esto está sonando" (título de la fila
  actual, línea de letra actual no: esa usa opacidad), (c) toggles activados
  (shuffle, repeat, corazón lleno), (d) el badge Hi-Res. Nunca decorativo.
  Usar el acento del sistema (y no el dorado de TIDAL) integra la app en GNOME
  y Plasma; el texto de los badges lleva el significado.
- **Corazón:** acento, no rojo. El rojo (`--error-color`) ya significa error
  en libadwaita; mezclar ambos confunde. El estado se comunica por forma
  (contorno ↔ relleno), no solo por color.
- **Atenuado** (`dimmed`, opacidad `--dim-opacity`): información secundaria.
  Contraste medido ≈ 4,8:1 (claro) y ≈ 5,4:1 (oscuro) sobre el fondo de
  ventana: cumple AA a tamaño de cuerpo. No combinar `dimmed` con texto
  menor que `caption`.
- **Destructivo** (`destructive-action`): solo en diálogos de confirmación
  (borrar playlist, cerrar sesión).
- **Badges** (nunca solo color; siempre texto):

| Badge | Texto | Aspecto | Dónde |
|---|---|---|---|
| Hi-Res | `HI-RES` | Texto y contorno 1 px en `--accent-color`, sin relleno | Filas de pista, tarjetas no, cabeceras, reproductor |
| Lossless | `LOSSLESS` | Texto en color normal, contorno 1 px al 40 % | Solo cabeceras de álbum/playlist y reproductor (en filas sería ruido: casi todo TIDAL es lossless) |
| Explícito | `E` | Cuadrado 16×16, relleno `currentColor` al 16 %, texto normal | Filas, cabeceras, now playing; nombre accesible "Explicit" |
| Formato entregado | `FLAC 24/96` | Como Lossless, `numeric` | Barra del reproductor y now playing; es un botón que abre detalles |

Dolby Atmos no se muestra: Swell no lo reproduce y anunciarlo engaña.

### 1.7 Movimiento

| Qué | Duración | Curva | Implementación |
|---|---|---|---|
| Aparición del overlay de play y del botón ⋯ al pasar el ratón | 150 ms | ease-out | CSS `transition: opacity` |
| Skeleton → contenido | 200 ms | — | `gtk::Stack` crossfade |
| Pulso del skeleton | 1 200 ms, alterno, infinito | ease-in-out | CSS `@keyframes` (opacidad) |
| Corazón al marcar | 250 ms | ease-out | CSS `@keyframes` `transform: scale(1.25)` en el 50 % |
| Desplazamiento de carrusel con ‹ › | 300 ms | ease-out-cubic | `adw::TimedAnimation` sobre la `hadjustment` |
| Auto-scroll de letras | 400 ms | ease-in-out-cubic | `adw::TimedAnimation` sobre la `vadjustment` |
| Push/pop de páginas, panel lateral, bottom sheet, toasts | por defecto de libadwaita | — | No sobrescribir |

Las `AdwAnimation` siguen `gtk-enable-animations` por defecto y GTK desactiva
las transiciones/animaciones CSS con ese ajuste: el modo "reducir movimiento"
del sistema se respeta sin código extra. No se anima la carga de portadas en
listas (provoca parpadeo en scroll rápido).

### 1.8 CSS propio (`data/style.css`, cargado en `startup`)

Solo variables de libadwaita y `currentColor`. Única excepción: el velo sobre
imágenes (`alpha(black, …)`, igual que el `.osd` de libadwaita), porque va
encima de una portada, no del tema.

```css
/* Portadas */
.cover          { border-radius: 6px; }
.cover.large    { border-radius: 12px; }
.cover-overlay  {                        /* icono play/pausa sobre la portada */
  border-radius: 6px;
  background-color: alpha(black, 0.5);
  color: white;
  opacity: 0;
  transition: opacity 150ms ease-out;
}
listview > row:hover .cover-overlay,
.track-row.playing .cover-overlay { opacity: 1; }
.track-row.playing .track-title    { color: var(--accent-color); }

/* Número de pista ↔ icono play en filas de álbum */
.track-number, .row-play-icon { transition: opacity 150ms ease-out; }
.row-play-icon { opacity: 0; }
listview > row:hover .track-number   { opacity: 0; }
listview > row:hover .row-play-icon  { opacity: 1; }

/* Botón ⋯ de fila: siempre presente, realzado al pasar o con foco */
.row-more { opacity: 0.55; transition: opacity 150ms ease-out; }
listview > row:hover .row-more, .row-more:focus-visible { opacity: 1; }

/* Badges */
.badge {
  min-height: 16px; padding: 0 5px; border-radius: 4px;
  font-size: smaller; font-weight: 800; letter-spacing: 0.04em;
  box-shadow: inset 0 0 0 1px color-mix(in srgb, currentColor 40%, transparent);
}
.badge.hires    { color: var(--accent-color);
                  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-color) 70%, transparent); }
.badge.explicit { min-width: 16px; padding: 0; box-shadow: none;
                  background-color: color-mix(in srgb, currentColor 16%, transparent); }

/* Corazón */
.heart:checked { color: var(--accent-color); animation: heart-pop 250ms ease-out; }
@keyframes heart-pop { 50% { transform: scale(1.25); } }

/* Skeleton */
.skeleton {
  background-color: color-mix(in srgb, currentColor 10%, transparent);
  border-radius: 6px;
  animation: skeleton-pulse 1200ms ease-in-out infinite alternate;
}
@keyframes skeleton-pulse { to { opacity: 0.45; } }

/* Letras */
.lyrics-line { padding: 6px 12px; border-radius: 12px;
               opacity: var(--dim-opacity); transition: opacity 250ms ease-out; }
.lyrics-line.current { opacity: 1; }
.lyrics-line.synced:hover { background-color: color-mix(in srgb, currentColor 7%, transparent); }

/* Indicadores de soltar al reordenar */
.track-row.drop-above { box-shadow: inset 0  2px var(--accent-bg-color); }
.track-row.drop-below { box-shadow: inset 0 -2px var(--accent-bg-color); }

/* Mini reproductor */
.mini-progress trough, .mini-progress progress { min-height: 3px; border-radius: 0; }
.mini-player button { min-width: 44px; min-height: 44px; }

/* Rejillas */
gridview.media-grid > child { padding: 6px; border-radius: 12px; }

/* Responsive: la ventana recibe la clase `narrow` desde el breakpoint C */
.page-content                { margin-left: 24px; margin-right: 24px; }
window.narrow .page-content  { margin-left: 12px; margin-right: 12px; }
```

Las clases responsive se ponen con `AdwBreakpoint::connect_apply/unapply`
(`window.add_css_class("narrow")`), porque los setters de breakpoint no
pueden añadir clases y muchas páginas se crean después.

### 1.9 Iconos

Nombres estándar freedesktop/Adwaita cuando existen (Breeze los tiene
también): `media-playback-start/pause-symbolic`,
`media-skip-forward/backward-symbolic`, `media-playlist-shuffle-symbolic`,
`media-playlist-repeat-symbolic`, `media-playlist-repeat-song-symbolic`,
`audio-volume-{muted,low,medium,high}-symbolic`, `view-more-symbolic`,
`system-search-symbolic`, `user-home-symbolic`, `list-add-symbolic`,
`edit-symbolic`, `edit-copy-symbolic`, `user-trash-symbolic`,
`view-list-ordered-symbolic` (cola), `sidebar-show-symbolic`,
`network-offline-symbolic`, `list-drag-handle-symbolic`,
`window-close-symbolic`, `open-menu-symbolic`.
Se empaquetan en el gresource (prefijo `swell-`, del Icon Development Kit)
los que no son estándar: `swell-heart-symbolic`,
`swell-heart-filled-symbolic`, `swell-explore-symbolic`,
`swell-library-symbolic`, `swell-lyrics-symbolic`, `swell-radio-symbolic`,
`swell-mix-symbolic`, `swell-timer-symbolic`.

---

## 2. Estructura de la aplicación (shell)

### 2.1 Árbol de widgets

```
AdwApplicationWindow  (width-request 360, height-request 294, por defecto 1180×780)
├─ AdwBreakpoint A  "max-width: 1180sp"
├─ AdwBreakpoint B  "max-width: 900sp"
├─ AdwBreakpoint C  "max-width: 720sp"
└─ content: GtkStack root (crossfade 200 ms)
   ├─ "loading": AdwSpinner 32 px (visible solo tras 300 ms)
   ├─ "login":   AdwToolbarView → AdwHeaderBar + LoginView (§3.1)
   └─ "main":    AdwMultiLayoutView layouts
        ├─ Layout "wide"
        │  AdwToolbarView  (bottom-bar-style RAISED_BORDER)
        │  ├─ content: AdwOverlaySplitView sidebar_split
        │  │    (sidebar-position START, min 200, max 280, fraction 0.2)
        │  │    ├─ sidebar: AdwToolbarView
        │  │    │    ├─ [top] AdwHeaderBar (title "Swell"; end: main menu)
        │  │    │    └─ GtkScrolledWindow → GtkListView.navigation-sidebar  (§2.3)
        │  │    └─ content: AdwOverlaySplitView panel_split
        │  │         (sidebar-position END, min 320, max 420, fraction 0.3,
        │  │          show-sidebar = panel abierto)
        │  │         ├─ content: AdwLayoutSlot "content"
        │  │         └─ sidebar: AdwLayoutSlot "panel"
        │  └─ [bottom] AdwLayoutSlot "player"
        └─ Layout "narrow"
           AdwToolbarView
           ├─ content: AdwBottomSheet np_sheet
           │    (show-drag-handle true, full-width true, modal true)
           │    ├─ content:    AdwLayoutSlot "content"
           │    ├─ bottom-bar: MiniPlayer (§4.10)
           │    └─ sheet:      AdwLayoutSlot "panel"
           └─ [bottom] AdwViewSwitcherBar (stack = sections, reveal = !np_sheet.open)

   Hijos de las ranuras (un solo ejemplar, se re-parentan al cambiar de layout):
   [content] Box vertical
               └─ AdwToastOverlay → AdwViewStack sections
                    ├─ "home"       AdwNavigationView (raíz: HomePage)
                    ├─ "explore"    AdwNavigationView (raíz: ExplorePage)
                    ├─ "search"     AdwNavigationView (raíz: SearchPage)
                    └─ "collection" AdwNavigationView (raíz: depende del layout, §2.4)
   [panel]   NowPlayingPanel (§3.13)
   [player]  PlayerBar (§4.9)
```

Decisiones clave:

- **Una `AdwNavigationView` por sección**: cada sección conserva su
  historial (ir a Home y volver a Collection te devuelve donde estabas). Es el
  patrón que el usuario espera de pestañas en móvil y reduce memoria de
  trabajo en escritorio.
- **`AdwToastOverlay` solo alrededor del contenido**: los toasts aparecen
  encima de la barra del reproductor, nunca tapándola.
- **`AdwMultiLayoutView`** porque el panel y el reproductor cambian de padre
  entre layouts; los breakpoints no pueden mover widgets.
- **Barra del reproductor a todo el ancho**, debajo de barra lateral y
  contenido: la reproducción es global y no pertenece a ninguna página
  (Gestalt, región común).
- `AdwHeaderBar` dentro de los split views gestiona sola qué botones de
  ventana muestra. Dentro del bottom sheet se fuerzan
  `show-start/end-title-buttons = false` con setters del breakpoint C.

### 2.2 Breakpoints

| Breakpoint | Condición | Setters / efectos |
|---|---|---|
| (base) | > 1180 sp | Barra lateral fija; panel derecho (now playing/letras/cola) **acoplado**: estrecha el contenido |
| A | `max-width: 1180sp` | `panel_split.collapsed = true`: el panel se superpone al contenido; Esc o clic fuera lo cierra |
| B | `max-width: 900sp` | Barra del reproductor compacta: `volume_scale.visible = false`, `volume_menu.visible = true` (popover con escala vertical); badge de formato muestra solo `HI-RES`/`LOSSLESS` (el detalle sigue en su popover); `sidebar_split.min-sidebar-width = 180` |
| C | `max-width: 720sp` | `multi_layout.layout-name = "narrow"`; clase `narrow` en la ventana; tarjetas de 144 px; cabeceras de página apiladas y centradas; `np_transport.visible = true` (controles dentro del panel); cabecera del panel sin botones de ventana; botón de menú principal visible en la cabecera raíz de cada sección |

Además, cada página con lista de pistas va envuelta en un `AdwBreakpointBin`
(uno por página, nunca por fila) con `max-width: 640px` que pone la propiedad
`compact` de la lista; las filas la leen en `bind` (§4.1). Así la fila se
adapta al ancho real del contenido, que depende de si la barra lateral y el
panel están acoplados. `AdwBreakpointBin` exige `width-request`/
`height-request`: 360×200.

### 2.3 Barra lateral (ancho)

Un único `gtk::ListView` con clase `navigation-sidebar`, `SingleSelection`,
directamente dentro del `ScrolledWindow` (virtualizado: el usuario puede
tener cientos de playlists). Modelo: `FlattenListModel` de tres
`ListStore` con cabeceras de sección por `header-factory` (GTK 4.12).

```
┌ Swell                        ☰ ┐   AdwHeaderBar; ☰ = menú principal
│ ⌂  Home                        │   filas de navegación: 36 px, icono 16
│ ◎  Explore                     │
│ ⌕  Search                      │
│ COLLECTION                     │   cabecera: caption-heading + dimmed
│ ♥  Tracks               1 234  │   contador numeric dimmed
│ ◫  Albums                      │
│ ☺  Artists                     │
│ ≡  Playlists                   │
│ ∿  Mixes & Radios              │
│ ⟲  Recently played             │
│ PLAYLISTS                   +  │   + = "New Playlist" (botón flat 24 px)
│ ▣  Chill evenings          🔊  │   filas de 44 px, portada 32; 🔊 = está sonando
│ ▣  Running                     │
│ ▣  Discover Weekly (fav.)      │
└────────────────────────────────┘
```

- La sección PLAYLISTS lista las propias y las favoritas, ordenadas por
  última modificación; tooltip con "by <owner>" en las ajenas.
- Selección = ubicación actual: al cambiar la página visible de la sección
  activa, se selecciona la fila de la playlist abierta si está en la lista;
  si no, la fila de la sección (visibilidad del estado del sistema).
- La fila de la fuente que está sonando muestra `audio-volume-high-symbolic`
  al final (con nombre accesible "Playing").
- Cada fila de playlist propia es `GtkDropTarget` de pistas (§4.12): soltar
  añade a la playlist con toast.
- Clic derecho en una playlist: menú de playlist (§4.11).
- **F9** oculta/muestra la barra lateral en ancho; con ella oculta aparece
  `sidebar-show-symbolic` al inicio de la cabecera del contenido.

Menú principal (☰, `GtkMenuButton` con `gio::Menu`):
`Preferences` (Ctrl+,) · `Keyboard Shortcuts` (Ctrl+?) · `About Swell` ─
`Sign Out…` ─ `Quit` (Ctrl+Q). "Quit" se incluye porque cerrar la ventana
puede dejar la app en segundo plano.

### 2.4 Navegación por secciones y anchos

- **Ancho:** seleccionar una fila de la barra lateral activa su sección en
  `sections` y, para las subvistas de Collection y las playlists, hace
  `collection_nav.replace(&[subvista])`. Las playlists de la barra lateral se
  abren como `replace(&[playlist])` en Collection.
- **Estrecho:** no hay barra lateral; `AdwViewSwitcherBar` con 4 destinos
  (Home, Explore, Search, Collection). La raíz de Collection es
  `CollectionIndexPage` (§3.6), que reutiliza el mismo modelo y factoría de la
  barra lateral. Las subvistas se apilan con `push`.
- Al pasar de ancho a estrecho, si la raíz de Collection es una subvista, se
  reconstruye la pila como `[índice, subvista, …resto]`; al revés, se quita el
  índice. Se hace en `apply`/`unapply` del breakpoint C.
- Enlaces `tidal://…` y `https://tidal.com/browse/…`: `present()` de la
  ventana y `push` en la sección visible.

### 2.5 Now playing, letras y cola

| | Ancho (> 1180 sp) | Regular (721–1180 sp) | Estrecho (≤ 720 sp) |
|---|---|---|---|
| Contenedor | Panel derecho acoplado | Panel derecho superpuesto | `AdwBottomSheet` |
| Se abre con | Clic en la portada de la barra; botones Letras/Cola de la barra; Ctrl+Shift+N, Ctrl+Shift+L, Ctrl+L | igual | Toque en el mini reproductor (abre en Playing); mismos atajos |
| Se cierra con | Pulsar de nuevo el botón de la pestaña activa | Igual + Esc + clic fuera | Arrastrar abajo, Esc, botón `go-down-symbolic` |
| Controles de transporte | En la barra | En la barra | Dentro de la pestaña Playing |

Los botones Letras y Cola de la barra son `GtkToggleButton` que reflejan
"panel abierto en esta pestaña": pulsar el de la pestaña visible cierra el
panel; pulsar otro cambia de pestaña. El estado del panel (abierto, pestaña)
se guarda en GSettings.

### 2.6 Wireframes del shell

Ancho, panel acoplado:

```
┌──────────────┬──────────────────────────────────────┬────────────────────┐
│ Swell      ☰ │ ‹  Album                             │ Playing Lyrics Queue│
│ ⌂ Home       │ ┌────────┐ ALBUM                     │ ┌────────────────┐ │
│ ◎ Explore    │ │ cover  │ Random Access Memories    │ │                │ │
│ ⌕ Search     │ │  240   │ Daft Punk · 2013          │ │   cover 312    │ │
│ COLLECTION   │ │        │ 13 tracks, 74 min         │ │                │ │
│ ♥ Tracks     │ └────────┘ [HI-RES] [E]              │ └────────────────┘ │
│ ◫ Albums     │ (▶ Play) (⤮ Shuffle)  ♥  ⧉  ⋯        │ Get Lucky       E  │
│ …            │ ──────────────────────────────────── │ Daft Punk · RAM    │
│ PLAYLISTS  + │  1  Give Life Back to Music     4:34 │ [FLAC 24/88.2]     │
│ ▣ Chill      │  2  The Game of Love            5:21 │  ♥   ⊚ radio   ⋯   │
│ ▣ Running    │ ▶3  Giorgio by Moroder          9:04 │ Up next: Beyond  › │
├──────────────┴──────────────────────────────────────┴────────────────────┤
│ ▣ Get Lucky  ♥   │   ⤮   ⏮  (▶)  ⏭   ⟳    │ [HI-RES 24/88] ✎ ≡ 🔊━━━ ⋯ │
│   Daft Punk      │ 2:01 ━━━━━━●━━━━━━ 6:09 │                            │
└──────────────────────────────────────────────────────────────────────────┘
```

Estrecho:

```
┌─────────────────────────────┐        ┌─────────────────────────────┐
│ ‹      Album              ⋯ │        │            ───              │ drag handle
│        ┌──────────┐         │        │ Playing  Lyrics  Queue    ⌄ │
│        │ cover 180│         │        │      ┌───────────────┐      │
│        └──────────┘         │        │      │  cover fluido │      │
│   Random Access Memories    │        │      └───────────────┘      │
│   Daft Punk · 2013          │        │ Get Lucky                 E │
│ (▶ Play) (⤮)  ♥  ⋯          │        │ Daft Punk · RAM             │
│  1 Give Life Back…    ⋯     │        │ 2:01 ━━━━━━●━━━━━━━━━ 6:09  │
│  2 The Game of Love   ⋯     │        │  ⤮    ⏮    (▶)    ⏭    ⟳   │
│ ─────────────────────────── │        │ [FLAC 24/88.2]  ♥  ⊚  🔊  ⋯ │
│ ▣ Get Lucky        ▶  ⏭    │ mini   │                             │
│ ━━━━━━━━━━━━━━━━━━━━━━━━━━━ │        │                             │
│  ⌂     ◎     ⌕     ♥        │ tabs   │                             │
└─────────────────────────────┘        └─────────────────────────────┘
```

| Decisión | Principio | Reducción de carga cognitiva |
|---|---|---|
| Barra lateral con subvistas y playlists visibles | Nielsen 6 (reconocer > recordar), Ley de Hick | Destinos frecuentes a un clic, sin menús intermedios |
| Pila de navegación por sección | Nielsen 3 (control), modelo mental de pestañas | El usuario no pierde su sitio al cambiar de sección |
| Reproductor siempre visible, a todo el ancho | Nielsen 1 (estado del sistema), Gestalt región común | Estado de reproducción sin buscarlo; separado de la navegación |
| Panel lateral en lugar de página para la cola/letras | Nielsen 7 (flexibilidad) | Se puede explorar mientras se ve la cola: sin cambio de contexto |
| Bottom sheet + mini reproductor en estrecho | Ley de Fitts, convención móvil (Nielsen 4) | Objetivos grandes en la zona del pulgar |

---

## 3. Pantallas

Patrón común de página: `AdwNavigationPage` → `AdwToolbarView` con
`[top] AdwHeaderBar` y `[top] AdwBanner offline` (enlazado a
`AppState::offline`), contenido en un `gtk::Stack` con hijos `skeleton`,
`content`, `empty`, `error`. El título de la cabecera (`AdwWindowTitle`)
aparece solo cuando la cabecera de página sale de la vista (se escucha la
`vadjustment`): evita títulos duplicados.

### Estados comunes (aplican a todas las pantallas salvo indicación)

| Estado | Presentación |
|---|---|
| Cargando | Skeleton con la forma de la página tras 150 ms (si la respuesta llega antes, no parpadea). `AdwSpinner` solo donde la forma es desconocida (búsqueda, paginación al final de listas) |
| Vacío | `AdwStatusPage` (icono, título, descripción, a veces un botón `pill`) |
| Error | `AdwStatusPage` icono `network-error-symbolic`, título "Couldn't Load This Page", descripción humana del error, botón `pill suggested-action` "Retry" (F5). Fallo al paginar: fila final con texto y botón "Retry", sin perder lo cargado |
| Sin conexión | `AdwBanner` "You're offline. Showing saved music." con botón "Retry". Se muestra lo cacheado; acciones que necesitan red quedan insensibles con tooltip "Not available offline" |
| Sesión caducada | Vuelta a la vista de login con el mensaje "Your TIDAL session has expired. Sign in again; your queue will be restored." La cola y la posición se guardan antes |
| Sin GStreamer | `AdwBanner` permanente "Playback is unavailable: install GStreamer's playbin3 plugins." Navegación disponible, botones de reproducir insensibles |

### 3.1 Login

**Objetivo:** iniciar sesión entendiendo la diferencia de calidad entre los
dos métodos.

**Jerarquía:** icono de app → título → elección de método (primaria:
navegador) → instrucciones del paso actual.

`AdwNavigationView` con tres páginas (Atrás cancela el flujo en curso):

```
┌──────────────────────────────────────────┐
│                 (icono 128)              │
│            Welcome to Swell              │  title-1
│   Sign in with your TIDAL account.       │  dimmed
│                                          │
│   ( Sign In with Browser )               │  pill suggested-action, 360 px
│   Hi-Res Lossless, up to 24-bit/192 kHz  │  caption dimmed
│                                          │
│   ( Use a Device Code )                  │  pill
│   Lossless, up to 16-bit/44.1 kHz        │  caption dimmed
└──────────────────────────────────────────┘
```

| Nivel | Pieza | Widget |
|---|---|---|
| Átomos | Icono de app; título; pistas de calidad | `GtkImage` 128; `GtkLabel.title-1`; `GtkLabel.caption.dimmed` |
| Moléculas | Botón de método + pista | `GtkBox` v (6): `GtkButton.pill` + `GtkLabel` |
| Organismo | Elección | `AdwStatusPage` con hijo `GtkBox` (12) dentro de `AdwClamp` 360 |
| Página "Browser" | Pasos numerados; botón "Open Browser Again"; `AdwEntryRow` "Paste the address of the “Oops” page" en `boxed-list`; botón "Continue" `suggested-action` (insensible hasta que la URL contenga `code=`) | `AdwStatusPage` + `GtkListBox.boxed-list` |
| Página "Device code" | Código `title-1` `monospace` seleccionable + botón copiar; `GtkLinkButton` link.tidal.com; fila "Waiting for approval…" con `AdwSpinner` | `AdwStatusPage` |

Estados: error en línea (`GtkLabel.error`, wrap) bajo la acción; el código de
dispositivo caducado muestra "This code expired" + "Get a New Code". Llavero
inaccesible: mensaje con causa y solución.

| Decisión | Principio | Efecto |
|---|---|---|
| La pista de calidad bajo cada botón | Nielsen 2 (lenguaje del usuario), Gestalt proximidad | El usuario elige con información, no a ciegas |
| Pasos en páginas separadas con Atrás | Nielsen 3 y 5 | Una tarea por pantalla; cancelar es obvio |
| Validación del URL antes de habilitar Continue | Nielsen 5 (prevención de errores) | Evita el error más común del flujo PKCE |

### 3.2 Home

**Objetivo:** volver a lo que se escucha y descubrir, con el mínimo de
decisiones.

**Jerarquía:** atajos (lo reciente, arriba) → carruseles en el orden del feed
→ listas de pistas.

`GtkScrolledWindow` → `AdwClamp` (máx. 1600) → `GtkBox` v `.page-content`
(spacing 36), módulos renderizados por un `ModuleRenderer` común (también lo
usan Explore y sus subpáginas):

| Módulo del feed | Organismo | Widget |
|---|---|---|
| Shortcuts | Rejilla de tiles horizontales (portada 56 + título `heading` de 2 líneas + overlay play al pasar) | `GtkFlowBox` homogéneo, 2–4 columnas, `selection-mode none`, máx. 8 |
| Carrusel (mixes, álbumes, artistas, playlists) | Sección de carrusel (§4.3) | `CarouselSection` |
| Lista de pistas | Cabecera de sección + hasta 5 filas + "View All" | `GtkListBox.boxed-list` con `TrackRow` (no virtualizado: 5 filas) |
| Más módulos por cursor | Spinner al final; se pide al estar a < 1 pantalla del final | `AdwSpinner` |

Skeleton: fila de 4 tiles + 2 carruseles de 6 tarjetas fantasma. Vacío
(feed sin módulos): `AdwStatusPage` "Nothing Here Yet" + "Explore" pill.
F5 recarga.

| Decisión | Principio | Efecto |
|---|---|---|
| Atajos arriba con portada | Nielsen 6, efecto de posición serial | Lo reciente se reconoce de un vistazo |
| Una sola gramática de módulos para Home/Explore/View all | Nielsen 4 (consistencia) | Se aprende una vez |
| Botón play sobre la portada al pasar | Ley de Fitts, Nielsen 7 (atajo) | Reproducir sin entrar en la página |

### 3.3 Explore

**Objetivo:** navegar por géneros, estados de ánimo y destacados.

- Nube de enlaces de página (géneros/moods): `AdwWrapBox` (1.7, spacing 6,
  line-spacing 6) de `GtkButton.pill` con el título; `push` de una
  `ModulePage` (mismo renderer).
- Debajo, módulos destacados como en Home.
- Sin colores arbitrarios por género: el texto es la información y los
  colores inventados romperían el tema.

| Decisión | Principio | Efecto |
|---|---|---|
| Géneros como píldoras que fluyen | Gestalt similitud, Ley de Hick (lista escaneable) | Escanear texto es más rápido que decodificar colores |
| Subpáginas con el mismo renderer | Nielsen 4 | Sin patrones nuevos |

### 3.4 Search

**Objetivo:** encontrar mientras se escribe.

```
┌ ‹      [ ⌕ Search TIDAL                    ✕ ]          (spinner) ┐
│  ( All | Tracks | Albums | Artists | Playlists )                   │
│  TOP HIT                     TRACKS                    View All ›  │
│  ┌─────┐ Daft Punk           ▣ Get Lucky  Daft Punk      6:09 ⋯   │
│  │ 120 │ Artist              ▣ One More Time …           5:20 ⋯   │
│  └─────┘ (▶)                 ▣ …                                   │
│  ALBUMS                                             ‹ ›  View All ›│
│  [card][card][card][card][card]…                                   │
└────────────────────────────────────────────────────────────────────┘
```

| Nivel | Pieza | Widget |
|---|---|---|
| Átomo | Campo de búsqueda | `GtkSearchEntry` en `AdwClamp` 520 como `title-widget` de la cabecera; `search-delay` 250 ms |
| Átomo | Indicador de petición | `AdwSpinner` 16 al final de la cabecera, solo si tarda > 300 ms |
| Molécula | Filtros | `AdwToggleGroup` (1.7) dentro de `GtkScrolledWindow` horizontal (en 360 px puede desbordar) |
| Organismo | Top hit | `GtkButton` grande (portada 120 o `AdwAvatar` 120, nombre `title-2`, tipo `dimmed`) + botón play `circular suggested-action` superpuesto |
| Organismo | Pistas (All) | Hasta 5 `TrackRow` en `GtkListBox.boxed-list` + "View All" (cambia el filtro a Tracks) |
| Organismo | Carruseles Albums/Artists/Playlists | `CarouselSection` |
| Plantilla por filtro | Tracks: `ListView` virtualizado con paginación por offset; resto: `GridView.media-grid` paginado | §4.1, §4.2 |

Vacío (consulta vacía): "Recent Searches" (hasta 10, locales, con "Clear" y ✕
por fila) en `boxed-list`; si no hay, `AdwStatusPage` "Search TIDAL" con
descripción "Tracks, albums, artists and playlists". Sin resultados:
`AdwStatusPage.compact` "No Results for “…”". Cada tecla cancela la petición
en curso; los resultados previos permanecen hasta que llegan los nuevos (no
parpadea a vacío). Escape vacía el campo; Ctrl+F lo enfoca desde cualquier
sitio (cambia a la sección Search).

| Decisión | Principio | Efecto |
|---|---|---|
| Búsqueda en vivo con debounce | Nielsen 1, Doherty (< 400 ms) | Feedback continuo sin pulsar Enter |
| Filtros como toggle group de una sola elección | Gestalt similitud/proximidad, Nielsen 4 | Estado del filtro visible siempre |
| Pistas como lista real (no tarjetas) | Nielsen 4 | Mismo comportamiento que cualquier lista de pistas |
| Resultados previos se mantienen mientras carga | Nielsen 1 | Evita destellos y pérdida de contexto |

### 3.5 Collection: subvistas

Todas comparten plantilla: cabecera con `AdwWindowTitle` (título + subtítulo
con recuento, por ejemplo "1 234 tracks · 3 d 4 h") y una barra de
herramientas bajo la cabecera (`GtkBox` 12 px de margen, spacing 6, como
`[top]` del `AdwToolbarView`):

```
(▶ Play) (⤮ Shuffle)          [ ⌕ Filter tracks          ] [ Date added ▾ ]
```

- Filtro: `GtkSearchEntry` (máx. 360, en estrecho `hexpand`), con
  `key-capture-widget` = la lista: escribir con el foco en la lista filtra.
  Filtrado local sobre la caché (`gtk::FilterListModel` con
  `StringFilter` sobre título/artista/álbum).
- Orden: `GtkMenuButton` con etiqueta del orden actual y menú de radio items
  + sección "Ascending/Descending".
- En estrecho, Play/Shuffle pasan a icono `circular` con tooltip.

| Subvista | Contenido | Ordenes | Vacío |
|---|---|---|---|
| Tracks | `ListView` virtualizado de `TrackRow` (variante Cover) | Date added, Title, Artist, Album, Duration | "No Favourite Tracks" / "Tap the heart on any track to save it here." + "Explore" |
| Albums | `GridView.media-grid` de `MediaCard` | Date added, Title, Artist, Release date | "No Favourite Albums" |
| Artists | `GridView` de tarjetas redondas | Date added, Name | "No Followed Artists" |
| Playlists | `GridView`; `AdwToggleGroup` All / Created by Me / Favourited; botón `list-add-symbolic` "New Playlist" en la cabecera | Last updated, Title | "No Playlists" + "New Playlist" |
| Mixes & Radios | `GridView` | Date added, Title | "No Saved Mixes" |

Filtro sin coincidencias: `AdwStatusPage.compact` "No Matches for “…”".
Quitar de favoritos desde aquí: la fila/tarjeta desaparece y aparece toast con
"Undo" (§4.13).

| Decisión | Principio | Efecto |
|---|---|---|
| Filtro siempre visible + escribir para filtrar | Nielsen 7 (aceleradores), 6 | Encontrar una pista entre miles sin recordar atajos |
| Orden visible en la etiqueta del botón | Nielsen 1 | El usuario sabe por qué la lista está en ese orden |
| Misma plantilla en las 5 subvistas | Nielsen 4, Gestalt similitud | Una sola curva de aprendizaje |

### 3.6 Collection: índice (solo estrecho)

`GtkScrolledWindow` → `GtkListView.navigation-sidebar` con el mismo modelo y
factoría de la barra lateral, sin las filas Home/Explore/Search (ya están en
el view switcher). Filas de 48 px en estrecho (objetivo táctil). Cabecera:
título "Collection", menú principal al final.

### 3.7 Album

**Objetivo:** reproducir el álbum o una pista, y entender qué es (calidad,
año, duración).

```
Ancho                                         Estrecho
┌────────┐ ALBUM                              ┌──────────┐
│ cover  │ Random Access Memories   title-1   │ cover 180│
│  240   │ Daft Punk (enlace)                 └──────────┘
│        │ 2013 · 13 tracks, 74 min  dimmed   Random Access Memories
└────────┘ [HI-RES] [LOSSLESS] [E]            Daft Punk · 2013 · 74 min
(▶ Play) (⤮ Shuffle)  ♥  ⧉  ⋯                 [HI-RES] [E]
                                              (▶ Play) (⤮)  ♥  ⋯
DISC 1
 1  Give Life Back to Music   ♥  4:34  ⋯
 2  The Game of Love          ♥  5:21  ⋯
```

| Nivel | Pieza | Widget |
|---|---|---|
| Átomos | Portada; sobretítulo; título; enlaces de artista; metadatos; badges; botones | `GtkPicture.cover.large` (`content-fit cover`) en `GtkAspectFrame` 1:1; `GtkLabel`s; `GtkLabel` con `<a href="swell:artist/ID">` y `activate-link`; `QualityBadge`; `GtkButton.pill` |
| Moléculas | Fila de acciones | `GtkBox` (12): Play `pill suggested-action`, Shuffle `pill`, `HeartButton`, Copy link `circular flat` (`edit-copy-symbolic`), ⋯ `GtkMenuButton circular flat` |
| Organismo | Cabecera | `PageHeader` variante Album (§4.4) |
| Organismo | Lista | `ListView` virtualizado: ítem 0 = cabecera, "Disc N" como ítems separadores (36 px, `heading dimmed`, solo si hay > 1 disco), `TrackRow` variante Numbered, ítem final = pie |
| Organismo | Pie | Fecha de lanzamiento completa y copyright (`caption dimmed`); botón "Credits" → `AdwDialog` con `AdwPreferencesGroup` por rol; reseña: tarjeta `card` con 6 líneas + "Read More" → `AdwDialog`; carrusel "More by <artist>" |

Doble clic/Enter en una pista: reproduce el álbum desde esa pista. Skeleton:
cuadro 240 + 3 barras + 8 filas fantasma. Pistas no disponibles: fila
`dimmed`, tooltip "Not available in your region", se saltan al reproducir.

| Decisión | Principio | Efecto |
|---|---|---|
| Play como única acción en acento | Von Restorff, Nielsen 8 | La acción principal salta a la vista |
| Badges de calidad con texto en la cabecera | Nielsen 1, WCAG 1.4.1 | Calidad comprensible sin color |
| Cabecera dentro de la lista virtualizada | Rendimiento percibido | Una sola superficie de scroll, sin saltos |
| Número de pista → icono play al pasar | Gestalt figura-fondo, Fitts | El número sigue informando y la acción aparece donde se mira |

### 3.8 Artist

**Objetivo:** reproducir lo mejor del artista y explorar su discografía.

- Cabecera: `AdwAvatar` 200/144 (iniciales de respaldo), sobretítulo
  "ARTIST", nombre `title-1`. Acciones: Play `pill suggested-action` (todas
  las top tracks, cargando el resto en segundo plano), Shuffle `pill`,
  Follow `GtkToggleButton.pill` ("Follow" ↔ "Following" + icono check: no
  solo color), Radio `circular flat` (`swell-radio-symbolic`), ⋯ (Copy link,
  Block artist).
- Secciones (`GtkBox` v, spacing 36, no virtualizado: todo acotado):
  Top Tracks (5 `TrackRow` en `boxed-list` + "View All" → página virtualizada
  con todas), Albums, EPs & Singles, Appears On, Artist Mixes (carruseles),
  Similar Artists (carrusel redondo), About (tarjeta con 4 líneas de bio +
  "Read More" → `AdwDialog` con texto seleccionable).
- Secciones vacías no se muestran (nunca un carrusel vacío).

| Decisión | Principio | Efecto |
|---|---|---|
| Play reproduce todas las top tracks | Nielsen 2 (expectativa real) | Corrige el "5 pistas" de High Tide |
| Follow con texto que cambia | Nielsen 1, WCAG 1.4.1 | Estado sin ambigüedad |
| Ocultar secciones vacías | Nielsen 8 | Menos ruido |

### 3.9 Playlist (propia y ajena)

Cabecera variante Playlist: portada, sobretítulo "PLAYLIST", título, línea
"by <owner> · 120 tracks, 7 h 12 min · Updated 3 days ago", descripción
`dimmed` de 3 líneas (tooltip con el texto completo; "Read More" si se corta).

| | Ajena | Propia |
|---|---|---|
| Acciones | Play, Shuffle, ♥, Copy link, ⋯ | Play, Shuffle, Edit (`edit-symbolic`, abre diálogo), ⋯ (Make Public/Private, Copy link, Delete Playlist…) |
| Visibilidad | — | Etiqueta con icono + texto "Public"/"Private" en la línea de metadatos |
| Filas | `TrackRow` Cover | `TrackRow` Cover + asa de arrastre (solo con orden "Custom order") |
| Quitar pista | — | Menú "Remove from This Playlist", tecla Supr; toast con Undo |
| Reordenar | — | Arrastrar por el asa; Alt+↑/↓; menú "Move Up/Move Down" (alternativa sin arrastre, WCAG 2.5.7) |
| Orden | Custom order, Title, Artist, Album, Duration, Date added | Igual; con un orden distinto de Custom se ocultan las asas |

Diálogo de edición/creación: `AdwDialog` (ancho 420) → `AdwToolbarView` con
`AdwHeaderBar` (Cancel a la izquierda, Save `suggested-action` a la derecha,
insensible con nombre vacío) → `AdwPreferencesGroup`: `AdwEntryRow` "Name",
`GtkTextView` en marco `card` para "Description" (máx. 500 caracteres, con
contador), `AdwSwitchRow` "Public". Borrar: `AdwAlertDialog` "Delete
“Chill”?" / "This can't be undone." / Cancel + Delete `destructive`.

Vacío (propia): `AdwStatusPage` "This Playlist Is Empty" / "Add tracks from
their menu or drag them here." + "Search" pill.

| Decisión | Principio | Efecto |
|---|---|---|
| Asas solo en orden personalizado | Nielsen 5 (prevenir errores), modelo mental | No se puede "reordenar" una vista ordenada |
| Alt+↑/↓ y menú además de arrastrar | WCAG 2.5.7, Nielsen 7 | Reordenar sin precisión motora |
| Confirmación solo para lo irreversible; Undo para lo demás | Nielsen 3 y 5 | Menos diálogos, sin pérdidas |

### 3.10 Mix / Radio

Cabecera variante Mix: portada del mix, sobretítulo "MIX", "TRACK RADIO" o
"ARTIST RADIO", título, subtítulo del mix (`dimmed`). Acciones: Play,
Shuffle, ♥ (funciona: favorito de mix), Copy link, ⋯. Lista `TrackRow`
Cover sin columna de álbum en compacto. Las radios de pista/artista se abren
desde menús y desde el botón de radio de Now playing.

### 3.11 "View all" / More

Página genérica: título del módulo; `GridView.media-grid` (tarjetas) o
`ListView` (pistas). Paginación por limit/offset: se pide la siguiente página
cuando el último ítem visible está a < 2 pantallas del final. Último ítem:
spinner mientras carga; fila de error con "Retry" si falla.

### 3.12 Recently played

`ListView` con cabeceras de sección (GtkSectionModel): "Today", "Yesterday",
"This Week", "Earlier". Fila de 60 px: portada 48 (redonda si es artista),
título, "Album · Daft Punk" `dimmed`, hora relativa `numeric dimmed`, ⋯.
Clic abre; el botón play del overlay reproduce. Vacío: "Nothing Played Yet".

### 3.13 Now playing (panel: Playing · Lyrics · Queue)

`NowPlayingPanel`: `AdwToolbarView` → `[top] AdwHeaderBar` con
`title-widget` = `AdwInlineViewSwitcher` (1.7) sobre un `AdwViewStack` de 3
páginas; en estrecho, además, botón `go-down-symbolic` "Close".

**Pestaña Playing**

```
┌──────────────────────────┐
│ ┌──────────────────────┐ │  GtkAspectFrame 1:1 → GtkPicture.cover.large.card
│ │      cover fluido    │ │  (máx. 480, en AdwClamp)
│ └──────────────────────┘ │
│ Get Lucky             E  │  title-2 (wrap 2 líneas)
│ Daft Punk                │  title-4, enlaces
│ Random Access Memories   │  dimmed, enlace
│ [HI-RES] [FLAC 24/88.2]  │  QualityBadge + botón de formato
│ 2:01 ━━━━━━●━━━━━━ 6:09  │  ┐ solo en estrecho (np_transport)
│  ⤮   ⏮   (▶)   ⏭   ⟳     │  ┘
│  ♥     ⊚ Radio    ⋯      │  acciones secundarias, circular flat 36
│ UP NEXT                  │
│ ▣ Beyond · Daft Punk   › │  1 fila: abre la pestaña Queue
└──────────────────────────┘
```

Nada sonando: `AdwStatusPage` "Nothing Playing" / "Pick something from your
collection." En estrecho, el botón play es 56 px `circular
suggested-action` y los demás 44 px.

**Pestaña Lyrics**

- Sincronizadas: `GtkScrolledWindow` → `GtkListBox` (≤ 200 líneas, no
  necesita virtualización) de `GtkLabel.title-3.lyrics-line.synced` (wrap,
  centradas en estrecho, alineadas a la izquierda en ancho). La línea actual
  recibe `.current` (opacidad 1; resto `--dim-opacity`). Clic o Enter en una
  línea → seek a su tiempo.
- Auto-scroll: la línea actual se coloca a 1/3 de la altura (animación de
  400 ms). Si el usuario desplaza, se pausa; aparece un botón flotante `pill
  osd` "Back to Current Line" (abajo, centrado, `GtkOverlay`) que reanuda.
- Sin sincronizar: `GtkLabel` wrap, `selectable`, cuerpo de texto.
- Pie: proveedor de la letra en `caption dimmed`.
- Sin letra: `AdwStatusPage.compact` "No Lyrics for This Track".

**Pestaña Queue**

- Cabecera de la pestaña: "Playing from <fuente>" (`caption dimmed`, enlace a
  la fuente) + botón "Clear" `flat` (toast con Undo).
- `ListView` virtualizado, `TrackRow` variante Queue (54 px), cabeceras de
  sección: "Now Playing", "Next in Queue" (lo añadido por el usuario, que
  suena antes), "Next From: <fuente>".
- Clic en fila → salta a ella. ✕ por fila → quitar. Arrastrar por el asa,
  Alt+↑/↓, Supr, menú (§4.11). Al abrir, `scroll_to` la fila actual.
- Vacía: `AdwStatusPage.compact` "Queue Is Empty" / "Use “Add to Queue” on
  any track, album or playlist."

| Decisión | Principio | Efecto |
|---|---|---|
| Tres pestañas en un panel | Gestalt región común, Nielsen 8 | Todo lo de "lo que suena" en un sitio |
| "Up next" en Playing | Nielsen 1 | Se ve qué viene sin abrir la cola |
| Pausa del auto-scroll al leer | Nielsen 3 | El sistema no pelea con el usuario |
| ✕ y salto directo en la cola | Nielsen 3, Fitts | Corrige la mayor queja de High Tide |
| Línea actual por opacidad, no por tamaño | Estabilidad del layout | Sin reflujo: la vista no salta |

### 3.14 Preferences

`AdwPreferencesDialog` (búsqueda activada), páginas:

| Página | Grupos y filas |
|---|---|
| Playback | **Quality:** `AdwComboRow` "Streaming Quality" (Low, High, Lossless, Hi-Res Lossless; las que el login no permite aparecen con subtítulo "Requires browser sign-in" e insensibles). **Processing:** `AdwComboRow` "Volume Normalisation" (Off / Track / Album), subtítulo "Off keeps the output bit-perfect"; `AdwSwitchRow` "Gapless Playback". **Output:** `AdwComboRow` "Output Device" (Automatic + sinks de PipeWire/ALSA). |
| General | `AdwSwitchRow` "Run in Background" (subtítulo "Keep playing when the window is closed"; pide el portal de background); `AdwSwitchRow` "Track Change Notifications"; `AdwSwitchRow` "Restore Queue on Start" |
| Storage | `AdwActionRow` "Cover Cache" (subtítulo tamaño "128 MB") + botón "Clear" `flat`; idem "Library Cache"; confirmación por toast |
| Blocked | Dos `AdwPreferencesGroup` (Artists, Tracks); filas `AdwActionRow` con avatar/portada y botón "Unblock" `flat`; vacío: `AdwStatusPage.compact` "Nothing Blocked" |
| Account | `AdwAvatar` 64 + nombre + plan; `AdwButtonRow` "Sign Out…" `destructive-action` → `AdwAlertDialog` |

### 3.15 Shortcuts dialog

`AdwShortcutsDialog` (1.8) con secciones General, Navigation, Playback, Lists;
contenido exacto en §5.2. Cada `AdwShortcutsItem` usa `action-name` cuando la
acción existe, para que el acelerador mostrado nunca se desincronice.

### 3.16 About

`AdwAboutDialog`: icono, nombre, versión, desarrollador, web, issue tracker,
licencia GPL-3.0-or-later, `debug-info` (versiones de GTK, libadwaita,
GStreamer, sink de salida, rutas de caché), créditos, y en `comments`: "Not
affiliated with TIDAL."

---

## 4. Catálogo de componentes reutilizables

### 4.1 TrackRow

Una sola clase `SwellTrackRow` (subclase de `GtkBox`), variante fijada al
crear la fábrica; propiedades `compact` y `reorderable` leídas en `bind`.

```
Cover (60 px)   [⠿] [cover 48▶] Title  [E][HI-RES]       Album         ♥  4:34  ⋯
                                Artist, Artist
Numbered (54)       [ 3 / ▶ ]   Title  [E]                              ♥  4:34  ⋯
                                feat. Artist (solo si difiere del álbum)
Queue (54)      [⠿] [cover 42]  Title  [E]                                 4:34  ✕
                                Artist
Compact (<640)      [cover 48]  Title  [E][HI-RES]                               ⋯
                                Artist · Album
```

| Pieza | Widget | Detalle |
|---|---|---|
| Asa | `GtkImage` `list-drag-handle-symbolic` 16 en caja de 24, `GtkDragSource` | Solo `reorderable`; cursor `grab` |
| Portada + estado | `GtkOverlay`: `GtkImage.cover` (pixel-size 48/42) + `GtkImage.cover-overlay` (`can-target false`) | Overlay: play al pasar; pausa/sonando en la fila actual |
| Número | `GtkOverlay`: `GtkLabel.numeric.dimmed.track-number` (36 px, `xalign 1`) + `GtkImage.row-play-icon` | Variante Numbered |
| Título | `GtkLabel.track-title` | `ellipsize end`, `single-line-mode` |
| Badges | `GtkLabel.badge.explicit` "E", `GtkLabel.badge.hires` "HI-RES" | Inline tras el título (proximidad), visibles según la pista |
| Secundaria | `GtkLabel.dimmed` | Artistas; en compacto "Artist · Album" |
| Álbum | `GtkLabel.dimmed` | Columna homogénea con el texto (como hoy); oculta en compacto |
| Corazón | `HeartButton` 36 | Oculto en compacto y en Queue (está en el menú) |
| Duración | `GtkLabel.numeric.dimmed`, `width-chars 5`, `xalign 1` | Oculta en compacto |
| Más / quitar | `GtkButton.circular.flat.row-more` 36 (`view-more-symbolic`) o ✕ (`window-close-symbolic`, "Remove from Queue") | No es `GtkMenuButton`: abre el popover compartido de la lista |

Reglas: márgenes 6/12, spacing 12; altura fija por variante (la lista entera
usa una variante); ningún menú, popover ni `AdwBreakpointBin` por fila. Los
artistas/álbum no son enlaces en las filas (mantiene el Tab razonable); se
llega a ellos por el menú. Fila sonando: clase `.playing` (título en acento +
icono). Nombre accesible del ítem con `ListItem::set_accessible_label`:
"Get Lucky, by Daft Punk, from Random Access Memories, 6 minutes 9 seconds,
Hi-Res, Explicit". Activación: doble clic / Enter (`ListView::activate`),
clic simple en táctil (`single-click-activate` en estrecho).

### 4.2 MediaCard

`GtkOverlay` → hijo `GtkButton.flat` (el área de la tarjeta) con `GtkBox` v
(6): portada (`GtkPicture.cover.large` 160/144 o `AdwAvatar` para
artistas), título `heading`, subtítulo `caption dimmed`; overlay: botón play
`circular suggested-action` 40 px abajo a la derecha de la portada (margen
6), visible al pasar o con foco dentro de la tarjeta (CSS opacity), siempre
visible en táctil/estrecho. Dos botones hermanos, no anidados (foco y
lector de pantalla correctos).

| Tipo | Portada | Subtítulo | Clic | Play |
|---|---|---|---|---|
| Álbum | cuadrada | Artista · año | abre álbum | reproduce álbum |
| Playlist | cuadrada | "by owner" / "N tracks" | abre playlist | reproduce |
| Mix | cuadrada | subtítulo del mix | abre mix | reproduce |
| Artista | `AdwAvatar` redondo, texto centrado | "Artist" | abre artista | top tracks |

Nombre accesible: "Album: Random Access Memories, by Daft Punk". Clic
derecho / pulsación larga / Shift+F10: menú del tipo (§4.11).

### 4.3 CarouselSection

```
Albums                                         (‹) (›)  View All
[card] [card] [card] [card] [card] [card] [ca…
```

`GtkBox` v (12): cabecera `GtkBox` [título `GtkLabel.title-3` hexpand,
`GtkButton.circular.flat` ‹ y › (insensibles en los extremos; nombres "Scroll
Back"/"Scroll Forward"), `GtkButton.flat` "View All" solo si hay más
ítems]; cuerpo `GtkScrolledWindow` (`hscrollbar-policy EXTERNAL`,
`vscrollbar NEVER`) → `GtkBox` h (12) con **≤ 20 tarjetas creadas de una vez**.
‹ › desplazan una "página" (ancho visible redondeado a tarjetas enteras) con
animación. La última tarjeta asoma parcialmente (en estrecho 144 px →
~2,2 tarjetas en 360 px): señal de que hay más (Gestalt continuidad). En
estrecho se ocultan ‹ › (se desliza con el dedo).

### 4.4 PageHeader

`GtkBox` (ancho: horizontal, spacing 24; estrecho: vertical centrado,
spacing 12, cambiado en `apply` del breakpoint C) con portada + columna de
texto + fila de acciones (12). Variantes:

| Variante | Portada | Sobretítulo | Metadatos | Acciones |
|---|---|---|---|---|
| Album | 240/180 cuadrada | ALBUM / EP / SINGLE | artistas (enlaces) · año · pistas, duración · badges | Play, Shuffle, ♥, Copy link, ⋯ |
| Playlist | 240/180 | PLAYLIST | by owner · pistas, duración · actualizada · Public/Private (propia) · descripción | Play, Shuffle, ♥ / Edit, ⋯ |
| Artist | `AdwAvatar` 200/144 | ARTIST | — | Play, Shuffle, Follow, Radio, ⋯ |
| Mix | 240/180 | MIX / TRACK RADIO / ARTIST RADIO | subtítulo | Play, Shuffle, ♥, Copy link, ⋯ |

### 4.5 QualityBadge

`GtkLabel` con `.badge` + modificador (`.hires`, `.explicit`). Fábrica única
`QualityBadge::new(kind)` que fija texto, tooltip y nombre accesible ("Hi-Res
Lossless", "Lossless", "Explicit"). Badge de formato: `GtkMenuButton.flat`
con etiqueta `numeric` "FLAC 24/96" que abre un `GtkPopover` con una
`GtkGrid` de pares etiqueta/valor: Quality, Codec, Delivered (bits · kHz),
Output device, Normalisation (Off/Track/Album). Si el stream entregado es
inferior al pedido, una línea `caption` lo explica ("Lossless delivered:
this track has no Hi-Res master").

### 4.6 HeartButton

`GtkToggleButton.circular.flat.heart`, icono `swell-heart-symbolic` ↔
`swell-heart-filled-symbolic`, tooltip "Add to Favourites" / "Remove from
Favourites", nombre accesible fijo "Favourite" + estado pressed (lo da el
toggle). Se enlaza en `bind` a `FavoritesStore` (conjunto cargado de
`favorites/ids`) por tipo + id; un cambio en el store notifica solo a las
filas visibles afectadas. Optimista: cambia al instante, revierte con toast
de error si la API falla. Funciona para pista, álbum, artista (Follow),
playlist y mix.

### 4.7 Skeletons

`SkeletonRow` (60 px: cuadro 48 + barras de 40 % y 25 % de ancho),
`SkeletonCard`, `SkeletonHeader`. Solo se crean los que caben en la vista
(~12 filas). Clase `.skeleton`; `accessible-role` `presentation` y el
contenedor anuncia "Loading".

### 4.8 Componentes de estado

`AdwStatusPage` (página completa) o `AdwStatusPage.compact` (dentro de
paneles y filtros); `AdwBanner` para estados globales (offline, sin
GStreamer). Textos en §3 "Estados comunes".

### 4.9 PlayerBar (ancho y regular)

`GtkCenterBox` (min-height 78, márgenes 6/12) dentro del `[bottom]` del
`AdwToolbarView`; oculta (`reveal-bottom-bars false`) hasta que hay algo que
reproducir o restaurar.

```
start (≥ 240, ≤ 30 %)          center (máx. 640)                end
[▣56] Get Lucky        ♥      ⤮   ⏮   (▶)   ⏭   ⟳      [FLAC 24/88] ⏲23m ✎ ≡ 🔇━━━━ ⋯
      Daft Punk              2:01 ━━━━━━━●━━━━━━━ 6:09
```

| Zona | Pieza | Widget |
|---|---|---|
| Inicio | Portada 56 (abre Now playing, "Show Now Playing") | `GtkButton.flat` con `GtkPicture.cover` |
| | Título (enlace al álbum) + E; artistas (enlaces) | `GtkLabel.heading` / `GtkLabel.dimmed`, markup con `<a>` |
| | Corazón | `HeartButton` |
| Centro | Shuffle (toggle), Previous, Play/Pause, Next, Repeat | `GtkToggleButton.circular.flat` 32; `GtkButton.circular.flat` 36; `GtkButton.circular.suggested-action` 40; repeat: botón que cicla Off → All → One, icono `repeat` / `repeat` pressed / `repeat-song`, tooltip "Repeat: All" |
| | Posición, barra, duración | `GtkLabel.numeric.caption.dimmed` (5 ch), `GtkScale` (`draw-value false`, hexpand), igual |
| Fin | Formato | `QualityBadge` de formato (§4.5) |
| | Temporizador (solo activo) | `GtkLabel.numeric` con icono `swell-timer-symbolic` "23 min", tooltip "Sleep timer: stops in 23 minutes" |
| | Letras, Cola | `GtkToggleButton.flat` (`swell-lyrics-symbolic`, `view-list-ordered-symbolic`) |
| | Volumen | `GtkButton.flat` silenciar (icono según nivel) + `GtkScale` 100 px; en B: `GtkMenuButton` con popover y escala vertical |
| | Más | `GtkMenuButton.flat` `view-more-symbolic`: Sleep Timer ▸ (Off, 15/30/45/60 min, End of Track), Go to Album, Go to Artist, Track Radio, Add to Playlist ▸, Copy Link |

Buffering > 300 ms: `AdwSpinner` 16 superpuesto en el botón play, nombre
accesible "Loading". Previous: reinicia la pista si van > 3 s (tooltip
coherente: "Restart Track" / "Previous Track"). Barra de progreso:
←/→ ±5 s, RePág/AvPág ±30 s, nombre "Seek", texto de valor "2 minutes 1
second of 6 minutes 9 seconds". Errores de stream: toast con "Retry"; pista
no disponible: se salta con toast.

### 4.10 MiniPlayer (estrecho)

`bottom-bar` del `AdwBottomSheet`: `GtkBox` v → `GtkProgressBar.mini-progress`
(3 px, sin texto, decorativo para el lector: la posición está en el sheet) +
`GtkBox` h (12, márgenes 6/12, alto 60): portada 42 `.cover`, `GtkBox` v
(título `heading`, artistas `caption dimmed`, ambos ellipsize), Play/Pause y
Next `circular flat` de 44 px. Tocar fuera de los botones abre el sheet
(comportamiento nativo del `AdwBottomSheet`). Nombre accesible del área:
"Now Playing: Get Lucky by Daft Punk. Open player".

### 4.11 Menús contextuales

Un **único `GtkPopoverMenu` por lista/rejilla** (padre = la vista,
`set_pointing_to` el rectángulo del clic o de la fila), modelos `gio::Menu`
estáticos por tipo, acciones de un `gio::SimpleActionGroup` "item" que actúan
sobre el objeto guardado al abrir. Disparadores: clic derecho
(`GestureClick` botón 3), pulsación larga (`GestureLongPress`, solo táctil),
Shift+F10 / tecla Menú sobre el ítem con foco, botón ⋯ de la fila.

| Tipo | Ítems (las rayas son secciones) |
|---|---|
| Pista | Play Next · Add to Queue ─ Add to Playlist ▸ (New Playlist…, 8 playlists recientes, More…) · Add to/Remove from Favourites ─ Go to Album · Go to Artist (▸ si hay varios) · Track Radio ─ Copy Link ─ *(contextual)* Remove from This Playlist · Move Up · Move Down ─ Block ▸ (Block Track, Block Artist) |
| Álbum | Play · Shuffle · Play Next · Add to Queue ─ Add to/Remove from Favourites · Add to Playlist ▸ ─ Go to Artist · Artist Radio ─ Copy Link |
| Playlist ajena | Play · Shuffle · Play Next · Add to Queue ─ Add to/Remove from Favourites ─ Copy Link |
| Playlist propia | Play · Shuffle · Play Next · Add to Queue ─ Edit Details… · Make Public / Make Private ─ Copy Link ─ Delete Playlist… |
| Artista | Play Top Tracks · Shuffle · Artist Radio ─ Follow / Unfollow ─ Copy Link ─ Block Artist |
| Mix / Radio | Play · Shuffle · Play Next · Add to Queue ─ Add to/Remove from Favourites ─ Copy Link |
| Fila de cola | Play Now ─ Move to Top · Move Up · Move Down · Remove from Queue ─ Add to Favourites · Add to Playlist ▸ ─ Go to Album · Go to Artist |

"More…" abre `AdwDialog` "Add to Playlist" con `GtkSearchEntry` + `ListView`
de playlists propias + fila "New Playlist…". Los ítems que no aplican se
ocultan (no se deshabilitan) salvo los que dependen de la red estando
offline, que se deshabilitan para que el usuario sepa que existen.

### 4.12 Arrastrar y soltar

- Fuente: cualquier `TrackRow` y `MediaCard` (`GtkDragSource`, contenido
  `text/uri-list` con `https://tidal.com/browse/track/ID`, también útil fuera
  de la app). Icono de arrastre: la portada.
- Destinos: filas de playlists propias en la barra lateral (añadir), la
  pestaña Queue (añadir al final o en la posición soltada), filas de la
  playlist propia en orden Custom y de la cola (reordenar, con
  `.drop-above`/`.drop-below`).
- Siempre con alternativa sin arrastre (menú y Alt+↑/↓).

### 4.13 Toasts y deshacer

`AdwToast` en el `AdwToastOverlay` del contenido; con acción → timeout 8 s;
sin acción → 3 s. Ctrl+Z deshace la última acción reversible aunque el toast
haya desaparecido.

| Acción | Toast | Botón |
|---|---|---|
| Quitar de favoritos (en Collection) | "Removed “Get Lucky” from Favourites" | Undo |
| Quitar pista(s) de playlist propia | "Removed from “Chill”" | Undo (re-inserta en el mismo índice) |
| Vaciar cola | "Queue cleared" | Undo |
| Quitar de la cola | (sin toast; es inmediato y visible) | Ctrl+Z |
| Añadir a la cola / play next | "Added 12 tracks to the queue" | Show (abre Queue) |
| Añadir a playlist | "Added to “Running”" | Open |
| Copiar enlace | "Link copied" | — |
| Pista saltada | "Skipped “X”: not available" | — |
| Error de red en una acción | "Couldn't update favourites" | Retry |

Borrar playlist y cerrar sesión no usan Undo: confirmación con
`AdwAlertDialog`, porque son irreversibles en el servidor o borran caché.

---

## 5. Accesibilidad y teclado

### 5.1 Lista de comprobación (WCAG 2.2 AA)

- [ ] Todo botón con solo icono tiene tooltip **y** nombre accesible
  (`update_property(&[Property::Label(..)])`); los toggles exponen pressed.
- [ ] Ítems de `ListView`/`GridView` con `set_accessible_label` completo
  (título, artista, álbum, duración, calidad, explícito, "playing").
- [ ] Ningún estado solo por color: corazón (forma), repeat (icono +
  pressed + tooltip), Follow (texto), fila actual (icono + acento), badges
  (texto), offline (banner con texto).
- [ ] Contraste ≥ 4,5:1 en texto normal y ≥ 3:1 en `title-3` (letras
  atenuadas a `--dim-opacity`, texto grande). `--accent-color` (no
  `--accent-bg-color`) para texto en acento.
- [ ] Objetivos ≥ 24×24 en todas partes (filas: 36), 44×44 en estrecho y
  mini reproductor; play del sheet 56.
- [ ] Orden de foco: barra lateral → cabecera del contenido → contenido →
  panel → barra del reproductor. F6 / Shift+F6 saltan entre esas regiones.
- [ ] Foco visible (`:focus-visible` de libadwaita, sin quitar outlines);
  el botón ⋯ de fila es plenamente opaco con foco.
- [ ] Reordenar sin arrastre (WCAG 2.5.7): menú Move Up/Down y Alt+↑/↓.
- [ ] Atajos de una tecla (Espacio, Supr) solo actúan con foco en
  listas/fondo; nunca en campos de texto (WCAG 2.1.4).
- [ ] Toasts y errores anunciados con `gtk::Accessible::announce` (4.14,
  prioridad media) si libadwaita no lo hace ya; el cambio de pista solo se
  anuncia si lo pidió el usuario (siguiente/anterior/doble clic).
- [ ] La escala de seek y la de volumen exponen valor en texto.
- [ ] Respeto de "reducir animaciones" (§1.7) y de alto contraste (las
  variables de libadwaita ya cambian).
- [ ] Textos de interfaz traducibles y sin unir cadenas (plurales con
  `ngettext`).
- [ ] Ventana usable a 360×294 sin scroll horizontal.

### 5.2 Mapa de teclado

Los atajos de reproducción con flechas y Espacio van en un
`GtkEventControllerKey` de la ventana en **fase de captura**, que deja pasar
el evento si el foco está en un `gtk::Editable` (y, para Espacio, también en
botones, interruptores y escalas, donde Espacio tiene su significado nativo).
El resto se registra con `set_accels_for_action`.

| Grupo | Atajo | Acción |
|---|---|---|
| General | Ctrl+F | Ir a Search y enfocar el campo |
| | Alt+1 / 2 / 3 / 4 | Home / Explore / Search / Collection |
| | F9 | Mostrar/ocultar barra lateral (ancho) |
| | F6 / Shift+F6 | Siguiente / anterior región de foco |
| | F5 | Recargar la página |
| | Alt+← (y botón "atrás" del ratón) | Volver |
| | Esc | Cerrar panel superpuesto / sheet / popover; vaciar búsqueda |
| | Ctrl+Z | Deshacer la última acción reversible |
| | Ctrl+, | Preferences |
| | Ctrl+? | Keyboard Shortcuts |
| | Ctrl+W | Cerrar ventana (queda en segundo plano si está activado) |
| | Ctrl+Q | Salir |
| Panel | Ctrl+Shift+N | Now Playing (abrir/cerrar) |
| | Ctrl+Shift+L | Lyrics |
| | Ctrl+L | Queue |
| Reproducción | Espacio | Play/Pause (no en texto/botones) |
| | Ctrl+Espacio | Play/Pause en cualquier contexto |
| | Ctrl+→ / Ctrl+← | Siguiente / anterior (reinicia si > 3 s) |
| | Shift+→ / Shift+← | Avanzar / retroceder 10 s |
| | Ctrl+↑ / Ctrl+↓ | Volumen ±5 % |
| | Ctrl+M | Silenciar |
| | Ctrl+S | Shuffle |
| | Ctrl+R | Repeat (Off → All → One) |
| | Ctrl+D | Favorito de la pista actual |
| | Teclas multimedia | Vía MPRIS (play, pause, stop, next, previous) |
| Barra de seek con foco | ← / → · RePág / AvPág · Inicio / Fin | ±5 s · ±30 s · principio / final |
| Listas | ↑ ↓ · Inicio / Fin | Moverse |
| | Enter | Reproducir pista / abrir ítem |
| | Shift+F10 / Menú | Menú contextual |
| | Supr | Quitar (cola, playlist propia; con Undo) |
| | Alt+↑ / Alt+↓ | Mover fila (cola, playlist propia en orden Custom) |
| | Escribir | Filtrar (subvistas de Collection) |

---

## 6. Restricciones de rendimiento que el diseño respeta

1. **Virtualización obligatoria** para todo lo que pueda superar ~50 ítems:
   favoritos, playlists, cola, resultados por tipo, rejillas de Collection,
   View all, Recently played, barra lateral. `ListView`/`GridView` como
   **hijo directo** del `ScrolledWindow` (o de `AdwClampScrollable`, que
   conserva la virtualización y limita listas a 1200 px de ancho). Nunca
   dentro de un `GtkBox` con otros hermanos en el mismo scroll: las
   cabeceras de página, discos y pies van como ítems del modelo.
2. **Alturas fijas por variante** (60/54/54/44/36 px): el scroll es exacto y
   la posición del centro de la vista se traduce directamente en índice para
   el `CoverLoader` (como hace hoy `library.rs`).
3. **Filas ligeras:** como mucho ~12 widgets por fila, sin `GMenuModel`,
   `GtkPopover`, `AdwBreakpointBin`, `AdwAvatar` con imagen grande ni
   sombras (`card`) por fila. El menú y el popover son uno por lista; la
   compacidad la decide un `AdwBreakpointBin` por página.
4. **Portadas perezosas:** solo se piden al hacer `bind`; el ticket se suelta
   en `unbind` (cancela la descarga si ya no se ve); prioridad por cercanía al
   centro de la vista; tamaño según §1.5; caché en memoria LRU (≈ 64 MB de
   texturas) + disco. En carruseles y Home se piden solo las tarjetas dentro
   de la vista (± 1 pantalla vertical, ± 1 página horizontal).
5. **Carruseles eager acotados:** ≤ 20 tarjetas creadas a la vez; el resto
   está en View all. Home crea los módulos según llegan y pide más por cursor
   al acercarse al final.
6. **Listas pequeñas no virtualizadas permitidas:** top tracks del artista (5),
   módulos de pistas de Home (5), letras (≤ 200), atajos (≤ 8), Preferences.
7. **Sin trabajo en el hilo de UI por fila:** formateo de duraciones y nombres
   accesibles calculados al cargar los datos, no en cada `bind` (o
   cacheados). Filtrado y ordenación con `FilterListModel`/`SortListModel`
   (incrementales).
8. **Cambios de favoritos sin reconstruir listas:** el `FavoritesStore`
   notifica por id; solo las filas visibles enlazadas se actualizan.
9. **Animaciones solo de opacidad/transform;** nada que provoque relayout
   por frame (la línea de letra actual no cambia de tamaño).
10. **Skeleton con retardo de 150 ms** y limitado a lo que cabe en pantalla;
    la caché SQLite se pinta antes de validar con la red (lo que ya hace hoy
    la lista de favoritos).
11. **Búsqueda:** debounce de 250 ms, cancelación de la petición anterior,
    sin vaciar resultados mientras llega la nueva respuesta.

---

## 7. Mapa a código (sugerido)

| Módulo | Contenido |
|---|---|
| `ui/window.rs` | Shell (§2), breakpoints, acciones de ventana, atajos |
| `ui/sidebar.rs` | Modelo y fábrica compartidos por barra lateral e índice de Collection |
| `ui/pages/` | `home`, `explore`, `module_page`, `search`, `collection/*`, `album`, `artist`, `playlist`, `mix`, `view_all`, `history`, `login` |
| `ui/widgets/` | `track_row` (evolución del actual), `track_list` (ListView + popover + DnD), `media_card`, `carousel`, `page_header`, `quality_badge`, `heart_button`, `skeleton`, `player_bar`, `mini_player`, `now_playing`, `lyrics_view`, `queue_view` |
| `ui/menus.rs` | Modelos `gio::Menu` por tipo y grupo de acciones "item" |
| `ui/dialogs/` | Editar playlist, añadir a playlist, créditos, bio/reseña |
| `data/style.css` | §1.8 |
| `data/icons/` | Iconos `swell-*` (§1.9) |

Cambios respecto al código actual: `track_row.rs` deja la clase `card` en la
portada (sombra por fila) por `.cover`, mueve el Hi-Res al lado del título
como badge con texto, añade overlay de estado, corazón, ⋯ y nombre
accesible; `player_bar.rs` pasa a `GtkCenterBox` con transporte completo,
volumen y botones de panel; `window.rs` sustituye la pila plana por el shell
de §2.1.
