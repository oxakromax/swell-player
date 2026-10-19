# Swell: interface design specification

Design document for implementing directly in gtk4-rs 0.11 (GTK 4.20 API) and
libadwaita 0.9 (API up to 1.8). It covers all of `docs/features.md`. UI text
is in English (the app's source language, translatable with gettext), and so
is the prose of this document.

> **Dependencies.** `Cargo.toml` currently enables `adw/v1_7` and `gtk/v4_18`.
> This design uses `AdwShortcutsDialog` (1.8), so it requires bumping to
> `adw/v1_8` and `gtk/v4_20`. Nothing from libadwaita 1.9 is used (for example
> `AdwSidebar`); once the minimum version is raised, the sidebar can migrate to
> it without changing the design.

---

## 0. What we do better than High Tide

| High Tide | Swell | Why |
|---|---|---|
| No sidebar; navigation with 3 toggles at the bottom | Sidebar with sections and the user's playlists; `AdwViewSwitcherBar` when narrow | Recognition over recall: the destination is in plain sight |
| `GtkListBox` for track lists (not virtualized) | `gtk::ListView`/`GridView` with fixed-height rows | Performance with 5,000 favourites |
| Context menu on tracks only | Menu on every object: track, album, playlist, artist, mix, queue row | Consistency and standards |
| Queue with no skip/remove/reorder | Click to skip, ✕ per row, drag + Alt+↑/↓ | User control and freedom |
| Search only on Enter | Live search with filters by type | Visibility of system status; fewer steps |
| `style-dark.css` with fixed colours (`#888`, `#444`) | Only libadwaita CSS variables | Works in light/dark and with the system accent |
| Volume in 10 % steps, no accessible names | 5 % steps, every icon button has an accessible name | WCAG 4.1.2 |

---

## 1. Principles and visual language

### 1.1 Principles

1. **Content is the interface.** Covers and titles carry the hierarchy;
   chrome (borders, shadows, colours) is kept to a minimum (aesthetic and
   minimalist design).
2. **A single player, always visible.** Playback is global and is never
   covered (bottom bar or mini player).
3. **Every object is actionable in the same way.** Click or Enter opens or
   plays; right click, long press, Shift+F10 or the Menu key open the same
   context menu (consistency and standards).
4. **Fast before pretty.** No effect that requires custom drawing or heavy
   per-row widgets. Animations are limited to opacity and `transform`.
5. **Native on GNOME and Plasma.** Light/dark theme and system accent via the
   settings portal, window buttons according to `gtk-decoration-layout`,
   system font (`font-family` is never set).

### 1.2 Spacing scale (multiples of 6)

| Token | px | Use |
|---|---|---|
| `xs` | 6 | Inside a molecule (icon↔label, title↔badge, text lines) |
| `s` | 12 | Between molecules (cover↔row text, action buttons, carousel/grid gaps) |
| `m` | 18 | Inner margin of page headers when narrow |
| `l` | 24 | Page side margin (wide), spacing from page header → content |
| `xl` | 36 | Vertical spacing between sections (carousels) |
| `xxl` | 48 | Top margin of status pages and the login |

Page side margin: 24 px (wide) and 12 px (narrow), applied through CSS with the
`narrow` class on the window (§1.8).

### 1.3 Type scale (libadwaita classes)

| Role | Class | Notes |
|---|---|---|
| Page header title | `title-1` (wide) / `title-2` (narrow) | At most 2 lines, `wrap`, `ellipsize end`, `lines 2` |
| Type overline ("ALBUM", "PLAYLIST") | `caption-heading` + `dimmed` | Uppercase in the text itself |
| Section / carousel title | `title-3` | |
| Card title | `heading` | 1 line, tooltip with the full text |
| Card subtitle | `caption` + `dimmed` | 1 line |
| Track row title | (body, no class) | Regular; accent while playing |
| Secondary row line | `dimmed` | Body size: `caption`+`dimmed` would have too little contrast |
| Durations, times, track numbers, counters | `numeric` (+ `dimmed`) | Tabular digits: columns don't jitter |
| Title in "Now playing" | `title-2` | |
| Lyrics lines | `title-3` | |
| Header metadata | `dimmed` | |

`dimmed` is the libadwaita 1.7+ name; `dim-label` still works but is not used
in new code.

### 1.4 Corner radii

| Radius | Where |
|---|---|
| 4 px | Quality/explicit badges |
| 6 px | Covers ≤ 56 px (rows, queue, mini player, player bar, sidebar), skeleton bars |
| 12 px | Cards and their covers, header cover, "Now playing" cover, lyrics lines on hover |
| circle | Artists always (`AdwAvatar`) |

Covers with a radius need `set_overflow(gtk::Overflow::Hidden)` on the widget,
in addition to the CSS class.

### 1.5 Cover sizes by context

Displayed logical size → download size. Rule: `px = ceil(logical × scale)`
(fractional scale of the surface on Wayland); the smallest TIDAL image-server
size that is ≥ `px` among 80/160/320/640 is requested (1280 only for albums,
with scale ≥ 1.5 and logical > 320). The per-object-type tables (album, artist,
playlist and mix have different sizes on TIDAL) live in `covers.rs`.

| Context | Logical (wide / narrow) | Download at 1× / 2× |
|---|---|---|
| Sidebar row (playlist) | 32 | 80 / 80 |
| Track row with cover | 48 | 80 / 160 |
| Queue row, mini player | 42 | 80 / 160 |
| Player bar | 56 | 80 / 160 |
| Home shortcut (horizontal tile) | 56 | 80 / 160 |
| Search top hit | 120 | 160 / 320 |
| Card (carousel, grid) | 160 / 144 | 160 / 320 |
| Page header (album, playlist, mix) | 240 / 180 | 320 / 640 |
| Artist header (`AdwAvatar`) | 200 / 144 | 320 / 640 |
| Now playing | fluid, max 480 | 640 / 1280 |

Placeholder while loading: a symbolic icon (`audio-x-generic-symbolic` for
tracks/albums, `view-list-symbolic` for playlists) centred on the static
`.skeleton` background; artists: `AdwAvatar` initials.

### 1.6 Use of colour

- **Accent** (`--accent-color` for text/icons, `--accent-bg-color` for
  backgrounds): only for (a) the primary action of each view (Play,
  `suggested-action`), (b) the "this is playing" state (title of the current
  row; the current lyrics line does not use it, it uses opacity), (c) toggles
  that are switched on (shuffle, repeat, filled heart), (d) the Hi-Res badge.
  Never decorative. Using the system accent (rather than TIDAL's gold)
  integrates the app into GNOME and Plasma; the badges' text carries the
  meaning.
- **Heart:** accent, not red. Red (`--error-color`) already means error in
  libadwaita; mixing both is confusing. The state is communicated by shape
  (outline ↔ filled), not by colour alone.
- **Dimmed** (`dimmed`, opacity `--dim-opacity`): secondary information.
  Measured contrast ≈ 4.8:1 (light) and ≈ 5.4:1 (dark) against the window
  background: it meets AA at body size. Don't combine `dimmed` with text
  smaller than `caption`.
- **Destructive** (`destructive-action`): only in confirmation dialogs
  (delete playlist, sign out).
- **Badges** (never colour alone; always text):

| Badge | Text | Appearance | Where |
|---|---|---|---|
| Hi-Res | `HI-RES` | Text and 1 px outline in `--accent-color`, no fill | Track rows, headers, player; not on cards |
| Lossless | `LOSSLESS` | Text in the normal colour, 1 px outline at 40 % | Only album/playlist headers and the player (in rows it would be noise: almost everything on TIDAL is lossless) |
| Explicit | `E` | 16×16 square, `currentColor` fill at 16 %, normal text | Rows, headers, now playing; accessible name "Explicit" |
| Delivered format | `FLAC 24/96` | Like Lossless, `numeric` | Player bar and now playing; it is a button that opens details |

Dolby Atmos is not shown: Swell does not play it and advertising it would be
misleading.

### 1.7 Motion

| What | Duration | Curve | Implementation |
|---|---|---|---|
| Fade-in of the play overlay and the ⋯ button on hover | 150 ms | ease-out | CSS `transition: opacity` |
| Skeleton → content | 200 ms | — | `gtk::Stack` crossfade |
| Skeleton pulse | 1,200 ms, alternating, infinite | ease-in-out | CSS `@keyframes` (opacity) |
| Heart when toggled on | 250 ms | ease-out | CSS `@keyframes` `transform: scale(1.25)` at 50 % |
| Carousel scroll with ‹ › | 300 ms | ease-out-cubic | `adw::TimedAnimation` on the `hadjustment` |
| Lyrics auto-scroll | 400 ms | ease-in-out-cubic | `adw::TimedAnimation` on the `vadjustment` |
| Page push/pop, side panel, bottom sheet, toasts | libadwaita defaults | — | Do not override |

`AdwAnimation`s follow `gtk-enable-animations` by default, and GTK disables CSS
transitions/animations with that setting: the system's "reduce motion" mode is
respected with no extra code. Cover loading is not animated in lists (it
causes flicker on fast scrolling).

### 1.8 Custom CSS (`data/style.css`, loaded on `startup`)

Only libadwaita variables and `currentColor`. The one exception: the scrim over
images (`alpha(black, …)`, like libadwaita's `.osd`), because it sits on top of
a cover, not the theme.

```css
/* Covers */
.cover          { border-radius: 6px; }
.cover.large    { border-radius: 12px; }
.cover-overlay  {                        /* play/pause icon over the cover */
  border-radius: 6px;
  background-color: alpha(black, 0.5);
  color: white;
  opacity: 0;
  transition: opacity 150ms ease-out;
}
listview > row:hover .cover-overlay,
.track-row.playing .cover-overlay { opacity: 1; }
.track-row.playing .track-title    { color: var(--accent-color); }

/* Track number ↔ play icon in album rows */
.track-number, .row-play-icon { transition: opacity 150ms ease-out; }
.row-play-icon { opacity: 0; }
listview > row:hover .track-number   { opacity: 0; }
listview > row:hover .row-play-icon  { opacity: 1; }

/* Row ⋯ button: always present, emphasised on hover or focus */
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

/* Heart */
.heart:checked { color: var(--accent-color); animation: heart-pop 250ms ease-out; }
@keyframes heart-pop { 50% { transform: scale(1.25); } }

/* Skeleton */
.skeleton {
  background-color: color-mix(in srgb, currentColor 10%, transparent);
  border-radius: 6px;
  animation: skeleton-pulse 1200ms ease-in-out infinite alternate;
}
@keyframes skeleton-pulse { to { opacity: 0.45; } }

/* Lyrics */
.lyrics-line { padding: 6px 12px; border-radius: 12px;
               opacity: var(--dim-opacity); transition: opacity 250ms ease-out; }
.lyrics-line.current { opacity: 1; }
.lyrics-line.synced:hover { background-color: color-mix(in srgb, currentColor 7%, transparent); }

/* Drop indicators when reordering */
.track-row.drop-above { box-shadow: inset 0  2px var(--accent-bg-color); }
.track-row.drop-below { box-shadow: inset 0 -2px var(--accent-bg-color); }

/* Mini player */
.mini-progress trough, .mini-progress progress { min-height: 3px; border-radius: 0; }
.mini-player button { min-width: 44px; min-height: 44px; }

/* Grids */
gridview.media-grid > child { padding: 6px; border-radius: 12px; }

/* Responsive: the window gets the `narrow` class from breakpoint C */
.page-content                { margin-left: 24px; margin-right: 24px; }
window.narrow .page-content  { margin-left: 12px; margin-right: 12px; }
```

The responsive classes are set with `AdwBreakpoint::connect_apply/unapply`
(`window.add_css_class("narrow")`), because breakpoint setters cannot add
classes and many pages are created later.

### 1.9 Icons

Standard freedesktop/Adwaita names where they exist (Breeze has them too):
`media-playback-start/pause-symbolic`,
`media-skip-forward/backward-symbolic`, `media-playlist-shuffle-symbolic`,
`media-playlist-repeat-symbolic`, `media-playlist-repeat-song-symbolic`,
`audio-volume-{muted,low,medium,high}-symbolic`, `view-more-symbolic`,
`system-search-symbolic`, `user-home-symbolic`, `list-add-symbolic`,
`edit-symbolic`, `edit-copy-symbolic`, `user-trash-symbolic`,
`view-list-ordered-symbolic` (queue), `sidebar-show-symbolic`,
`network-offline-symbolic`, `list-drag-handle-symbolic`,
`window-close-symbolic`, `open-menu-symbolic`.
The non-standard ones are bundled in the gresource (prefix `swell-`, from the
Icon Development Kit): `swell-heart-symbolic`,
`swell-heart-filled-symbolic`, `swell-explore-symbolic`,
`swell-library-symbolic`, `swell-lyrics-symbolic`, `swell-radio-symbolic`,
`swell-mix-symbolic`, `swell-timer-symbolic`.

---

## 2. Application structure (shell)

### 2.1 Widget tree

```
AdwApplicationWindow  (width-request 360, height-request 294, default 1180×780)
├─ AdwBreakpoint A  "max-width: 1180sp"
├─ AdwBreakpoint B  "max-width: 900sp"
├─ AdwBreakpoint C  "max-width: 720sp"
└─ content: GtkStack root (crossfade 200 ms)
   ├─ "loading": AdwSpinner 32 px (visible only after 300 ms)
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
        │  │          show-sidebar = panel open)
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

   Children of the slots (a single instance, re-parented when the layout changes):
   [content] vertical Box
               └─ AdwToastOverlay → AdwViewStack sections
                    ├─ "home"       AdwNavigationView (root: HomePage)
                    ├─ "explore"    AdwNavigationView (root: ExplorePage)
                    ├─ "search"     AdwNavigationView (root: SearchPage)
                    └─ "collection" AdwNavigationView (root: depends on the layout, §2.4)
   [panel]   NowPlayingPanel (§3.13)
   [player]  PlayerBar (§4.9)
```

Key decisions:

- **One `AdwNavigationView` per section**: each section keeps its own history
  (going to Home and back to Collection returns you to where you were). It is
  the pattern users expect from tabs on mobile, and it reduces working memory
  on the desktop.
- **`AdwToastOverlay` only around the content**: toasts appear above the
  player bar, never covering it.
- **`AdwMultiLayoutView`** because the panel and the player change parent
  between layouts; breakpoints cannot move widgets.
- **Full-width player bar**, below the sidebar and the content: playback is
  global and does not belong to any page (Gestalt, common region).
- `AdwHeaderBar` inside the split views decides by itself which window buttons
  to show. Inside the bottom sheet, `show-start/end-title-buttons = false` is
  forced with breakpoint C setters.

### 2.2 Breakpoints

| Breakpoint | Condition | Setters / effects |
|---|---|---|
| (base) | > 1180 sp | Fixed sidebar; right panel (now playing/lyrics/queue) **docked**: it narrows the content |
| A | `max-width: 1180sp` | `panel_split.collapsed = true`: the panel overlays the content; Esc or a click outside closes it |
| B | `max-width: 900sp` | Compact player bar: `volume_scale.visible = false`, `volume_menu.visible = true` (popover with a vertical scale); the format badge shows only `HI-RES`/`LOSSLESS` (the detail stays in its popover); `sidebar_split.min-sidebar-width = 180` |
| C | `max-width: 720sp` | `multi_layout.layout-name = "narrow"`; `narrow` class on the window; 144 px cards; page headers stacked and centred; `np_transport.visible = true` (controls inside the panel); panel header without window buttons; main menu button visible in the root header of each section |

In addition, every page with a track list is wrapped in an `AdwBreakpointBin`
(one per page, never per row) with `max-width: 640px` that sets the list's
`compact` property; rows read it in `bind` (§4.1). This way the row adapts to
the real width of the content, which depends on whether the sidebar and the
panel are docked. `AdwBreakpointBin` requires `width-request`/
`height-request`: 360×200.

### 2.3 Sidebar (wide)

A single `gtk::ListView` with the `navigation-sidebar` class, `SingleSelection`,
directly inside the `ScrolledWindow` (virtualized: the user may have hundreds of
playlists). Model: a `FlattenListModel` of three `ListStore`s with section
headers via `header-factory` (GTK 4.12).

```
┌ Swell                        ☰ ┐   AdwHeaderBar; ☰ = main menu
│ ⌂  Home                        │   navigation rows: 36 px, icon 16
│ ◎  Explore                     │
│ ⌕  Search                      │
│ COLLECTION                     │   header: caption-heading + dimmed
│ ♥  Tracks               1,234  │   numeric dimmed counter
│ ◫  Albums                      │
│ ☺  Artists                     │
│ ≡  Playlists                   │
│ ∿  Mixes & Radios              │
│ ⟲  Recently played             │
│ PLAYLISTS                   +  │   + = "New Playlist" (flat 24 px button)
│ ▣  Chill evenings          🔊  │   44 px rows, cover 32; 🔊 = playing
│ ▣  Running                     │
│ ▣  Discover Weekly (fav.)      │
└────────────────────────────────┘
```

- The PLAYLISTS section lists both the user's own and favourited playlists,
  ordered by last modification; others' playlists get a tooltip with
  "by <owner>".
- Selection = current location: when the visible page of the active section
  changes, the row of the open playlist is selected if it is in the list;
  otherwise, the row of the section (visibility of system status).
- The row of the source that is playing shows `audio-volume-high-symbolic` at
  the end (with the accessible name "Playing").
- Each row of one of the user's own playlists is a `GtkDropTarget` for tracks
  (§4.12): dropping adds to the playlist with a toast.
- Right click on a playlist: playlist menu (§4.11).
- **F9** hides/shows the sidebar when wide; while it is hidden,
  `sidebar-show-symbolic` appears at the start of the content header.

Main menu (☰, `GtkMenuButton` with a `gio::Menu`):
`Preferences` (Ctrl+,) · `Keyboard Shortcuts` (Ctrl+?) · `About Swell` ─
`Sign Out…` ─ `Quit` (Ctrl+Q). "Quit" is included because closing the window
may leave the app running in the background.

### 2.4 Navigation by section and width

- **Wide:** selecting a sidebar row activates its section in `sections` and,
  for the Collection subviews and playlists, calls
  `collection_nav.replace(&[subview])`. Playlists in the sidebar are opened as
  `replace(&[playlist])` in Collection.
- **Narrow:** there is no sidebar; `AdwViewSwitcherBar` with 4 destinations
  (Home, Explore, Search, Collection). The root of Collection is
  `CollectionIndexPage` (§3.6), which reuses the same model and factory as the
  sidebar. Subviews are stacked with `push`.
- When going from wide to narrow, if the root of Collection is a subview, the
  stack is rebuilt as `[index, subview, …rest]`; the other way round, the index
  is removed. This is done in `apply`/`unapply` of breakpoint C.
- `tidal://…` and `https://tidal.com/browse/…` links: `present()` the window
  and `push` onto the visible section.

### 2.5 Now playing, lyrics and queue

| | Wide (> 1180 sp) | Regular (721–1180 sp) | Narrow (≤ 720 sp) |
|---|---|---|---|
| Container | Docked right panel | Overlaid right panel | `AdwBottomSheet` |
| Opens with | Click on the cover in the bar; the bar's Lyrics/Queue buttons; Ctrl+Shift+N, Ctrl+Shift+L, Ctrl+L | Same | Tap on the mini player (opens on Playing); same shortcuts |
| Closes with | Pressing the active tab's button again | Same + Esc + click outside | Drag down, Esc, `go-down-symbolic` button |
| Transport controls | In the bar | In the bar | Inside the Playing tab |

The bar's Lyrics and Queue buttons are `GtkToggleButton`s that reflect "panel
open on this tab": pressing the one for the visible tab closes the panel;
pressing another switches tabs. The panel state (open, tab) is stored in
GSettings.

### 2.6 Shell wireframes

Wide, docked panel:

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

Narrow:

```
┌─────────────────────────────┐        ┌─────────────────────────────┐
│ ‹      Album              ⋯ │        │            ───              │ drag handle
│        ┌──────────┐         │        │ Playing  Lyrics  Queue    ⌄ │
│        │ cover 180│         │        │      ┌───────────────┐      │
│        └──────────┘         │        │      │  fluid cover  │      │
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

| Decision | Principle | Cognitive load reduction |
|---|---|---|
| Sidebar with visible subviews and playlists | Nielsen 6 (recognition > recall), Hick's law | Frequent destinations one click away, no intermediate menus |
| Navigation stack per section | Nielsen 3 (control), tabs mental model | The user doesn't lose their place when switching sections |
| Player always visible, full width | Nielsen 1 (system status), Gestalt common region | Playback status without searching for it; separate from navigation |
| Side panel instead of a page for queue/lyrics | Nielsen 7 (flexibility) | You can browse while viewing the queue: no context switch |
| Bottom sheet + mini player when narrow | Fitts's law, mobile convention (Nielsen 4) | Large targets in the thumb zone |

---

## 3. Screens

Common page pattern: `AdwNavigationPage` → `AdwToolbarView` with
`[top] AdwHeaderBar` and `[top] AdwBanner offline` (bound to
`AppState::offline`), content in a `gtk::Stack` with children `skeleton`,
`content`, `empty`, `error`. The header title (`AdwWindowTitle`) appears only
when the page header scrolls out of view (the `vadjustment` is watched): this
avoids duplicate titles.

### Common states (apply to all screens unless noted)

| State | Presentation |
|---|---|
| Loading | Skeleton shaped like the page after 150 ms (if the response arrives sooner, it doesn't flicker). `AdwSpinner` only where the shape is unknown (search, pagination at the end of lists) |
| Empty | `AdwStatusPage` (icon, title, description, sometimes a `pill` button) |
| Error | `AdwStatusPage` with icon `network-error-symbolic`, title "Couldn't Load This Page", a human-readable description of the error, `pill suggested-action` button "Retry" (F5). Pagination failure: a final row with text and a "Retry" button, without losing what was loaded |
| Offline | `AdwBanner` "You're offline. Showing saved music." with a "Retry" button. Cached content is shown; actions that need the network become insensitive with the tooltip "Not available offline" |
| Session expired | Back to the login view with the message "Your TIDAL session has expired. Sign in again; your queue will be restored." The queue and position are saved beforehand |
| No GStreamer | Permanent `AdwBanner` "Playback is unavailable: install GStreamer's playbin3 plugins." Navigation stays available, play buttons are insensitive |

### 3.1 Login

**Goal:** sign in while understanding the quality difference between the two
methods.

**Hierarchy:** app icon → title → choice of method (primary: browser) →
instructions for the current step.

`AdwNavigationView` with three pages (Back cancels the flow in progress):

```
┌──────────────────────────────────────────┐
│                 (icon 128)               │
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

| Level | Piece | Widget |
|---|---|---|
| Atoms | App icon; title; quality hints | `GtkImage` 128; `GtkLabel.title-1`; `GtkLabel.caption.dimmed` |
| Molecules | Method button + hint | `GtkBox` v (6): `GtkButton.pill` + `GtkLabel` |
| Organism | Choice | `AdwStatusPage` with a `GtkBox` (12) child inside an `AdwClamp` 360 |
| "Browser" page | Numbered steps; "Open Browser Again" button; `AdwEntryRow` "Paste the address of the “Oops” page" in a `boxed-list`; "Continue" `suggested-action` button (insensitive until the URL contains `code=`) | `AdwStatusPage` + `GtkListBox.boxed-list` |
| "Device code" page | Selectable `title-1` `monospace` code + copy button; `GtkLinkButton` link.tidal.com; "Waiting for approval…" row with an `AdwSpinner` | `AdwStatusPage` |

States: inline error (`GtkLabel.error`, wrap) under the action; an expired
device code shows "This code expired" + "Get a New Code". Inaccessible
keyring: a message with the cause and the fix.

| Decision | Principle | Effect |
|---|---|---|
| The quality hint under each button | Nielsen 2 (user's language), Gestalt proximity | The user chooses with information, not blindly |
| Steps on separate pages with Back | Nielsen 3 and 5 | One task per screen; cancelling is obvious |
| URL validation before enabling Continue | Nielsen 5 (error prevention) | Avoids the most common mistake in the PKCE flow |

### 3.2 Home

**Goal:** get back to what you listen to and discover, with the fewest
decisions.

**Hierarchy:** shortcuts (recent items, at the top) → carousels in feed order →
track lists.

`GtkScrolledWindow` → `AdwClamp` (max 1600) → vertical `GtkBox`
`.page-content` (spacing 36), with modules rendered by a common
`ModuleRenderer` (also used by Explore and its subpages):

| Feed module | Organism | Widget |
|---|---|---|
| Shortcuts | Grid of horizontal tiles (56 cover + 2-line `heading` title + play overlay on hover) | Homogeneous `GtkFlowBox`, 2–4 columns, `selection-mode none`, max 8 |
| Carousel (mixes, albums, artists, playlists) | Carousel section (§4.3) | `CarouselSection` |
| Track list | Section header + up to 5 rows + "View All" | `GtkListBox.boxed-list` with `TrackRow` (not virtualized: 5 rows) |
| More modules by cursor | Spinner at the end; requested when within < 1 screen of the end | `AdwSpinner` |

Skeleton: a row of 4 tiles + 2 carousels of 6 ghost cards. Empty (feed without
modules): `AdwStatusPage` "Nothing Here Yet" + "Explore" pill. F5 reloads.

| Decision | Principle | Effect |
|---|---|---|
| Shortcuts with covers at the top | Nielsen 6, serial position effect | Recent items are recognised at a glance |
| A single module grammar for Home/Explore/View all | Nielsen 4 (consistency) | Learned once |
| Play button over the cover on hover | Fitts's law, Nielsen 7 (shortcut) | Play without opening the page |

### 3.3 Explore

**Goal:** browse by genres, moods and highlights.

- Cloud of page links (genres/moods): an `AdwWrapBox` (1.7, spacing 6,
  line-spacing 6) of `GtkButton.pill` with the title; `push` of a
  `ModulePage` (same renderer).
- Below, featured modules as in Home.
- No arbitrary colours per genre: the text is the information, and invented
  colours would break the theme.

| Decision | Principle | Effect |
|---|---|---|
| Genres as flowing pills | Gestalt similarity, Hick's law (scannable list) | Scanning text is faster than decoding colours |
| Subpages with the same renderer | Nielsen 4 | No new patterns |

### 3.4 Search

**Goal:** find while typing.

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

| Level | Piece | Widget |
|---|---|---|
| Atom | Search field | `GtkSearchEntry` in an `AdwClamp` 520 as the header's `title-widget`; `search-delay` 250 ms |
| Atom | Request indicator | 16 `AdwSpinner` at the end of the header, only if it takes > 300 ms |
| Molecule | Filters | `AdwToggleGroup` (1.7) inside a horizontal `GtkScrolledWindow` (at 360 px it can overflow) |
| Organism | Top hit | Large `GtkButton` (120 cover or 120 `AdwAvatar`, name `title-2`, type `dimmed`) + overlaid `circular suggested-action` play button |
| Organism | Tracks (All) | Up to 5 `TrackRow`s in a `GtkListBox.boxed-list` + "View All" (switches the filter to Tracks) |
| Organism | Albums/Artists/Playlists carousels | `CarouselSection` |
| Template per filter | Tracks: virtualized `ListView` with offset pagination; the rest: paginated `GridView.media-grid` | §4.1, §4.2 |

Empty (empty query): "Recent Searches" (up to 10, local, with "Clear" and a ✕
per row) in a `boxed-list`; if there are none, `AdwStatusPage` "Search TIDAL"
with the description "Tracks, albums, artists and playlists". No results:
`AdwStatusPage.compact` "No Results for “…”". Every keystroke cancels the
request in flight; previous results stay until the new ones arrive (it doesn't
flicker to empty). Escape clears the field; Ctrl+F focuses it from anywhere
(switching to the Search section).

| Decision | Principle | Effect |
|---|---|---|
| Live search with debounce | Nielsen 1, Doherty (< 400 ms) | Continuous feedback without pressing Enter |
| Filters as a single-choice toggle group | Gestalt similarity/proximity, Nielsen 4 | The filter state is always visible |
| Tracks as a real list (not cards) | Nielsen 4 | Same behaviour as any other track list |
| Previous results are kept while loading | Nielsen 1 | Avoids flashes and loss of context |

### 3.5 Collection: subviews

They all share a template: a header with an `AdwWindowTitle` (title + subtitle
with the count, for example "1,234 tracks · 3 d 4 h") and a toolbar under the
header (a `GtkBox` with 12 px margin, spacing 6, as `[top]` of the
`AdwToolbarView`):

```
(▶ Play) (⤮ Shuffle)          [ ⌕ Filter tracks          ] [ Date added ▾ ]
```

- Filter: `GtkSearchEntry` (max 360, `hexpand` when narrow), with
  `key-capture-widget` = the list: typing while the list has focus filters.
  Local filtering over the cache (`gtk::FilterListModel` with a `StringFilter`
  on title/artist/album).
- Sort: a `GtkMenuButton` labelled with the current sort order and a menu of
  radio items + an "Ascending/Descending" section.
- When narrow, Play/Shuffle become `circular` icons with a tooltip.

| Subview | Content | Sort orders | Empty |
|---|---|---|---|
| Tracks | Virtualized `ListView` of `TrackRow` (Cover variant) | Date added, Title, Artist, Album, Duration | "No Favourite Tracks" / "Tap the heart on any track to save it here." + "Explore" |
| Albums | `GridView.media-grid` of `MediaCard` | Date added, Title, Artist, Release date | "No Favourite Albums" |
| Artists | `GridView` of round cards | Date added, Name | "No Followed Artists" |
| Playlists | `GridView`; `AdwToggleGroup` All / Created by Me / Favourited; `list-add-symbolic` "New Playlist" button in the header | Last updated, Title | "No Playlists" + "New Playlist" |
| Mixes & Radios | `GridView` | Date added, Title | "No Saved Mixes" |

Filter without matches: `AdwStatusPage.compact` "No Matches for “…”".
Removing from favourites from here: the row/card disappears and a toast with
"Undo" appears (§4.13).

| Decision | Principle | Effect |
|---|---|---|
| Always-visible filter + type to filter | Nielsen 7 (accelerators), 6 | Find a track among thousands without remembering shortcuts |
| Sort order visible in the button label | Nielsen 1 | The user knows why the list is in that order |
| Same template in all 5 subviews | Nielsen 4, Gestalt similarity | A single learning curve |

### 3.6 Collection: index (narrow only)

`GtkScrolledWindow` → `GtkListView.navigation-sidebar` with the same model and
factory as the sidebar, without the Home/Explore/Search rows (they are already
in the view switcher). 48 px rows when narrow (touch target). Header: title
"Collection", main menu at the end.

### 3.7 Album

**Goal:** play the album or a track, and understand what it is (quality, year,
duration).

```
Wide                                          Narrow
┌────────┐ ALBUM                              ┌──────────┐
│ cover  │ Random Access Memories   title-1   │ cover 180│
│  240   │ Daft Punk (link)                   └──────────┘
│        │ 2013 · 13 tracks, 74 min  dimmed   Random Access Memories
└────────┘ [HI-RES] [LOSSLESS] [E]            Daft Punk · 2013 · 74 min
(▶ Play) (⤮ Shuffle)  ♥  ⧉  ⋯                 [HI-RES] [E]
                                              (▶ Play) (⤮)  ♥  ⋯
DISC 1
 1  Give Life Back to Music   ♥  4:34  ⋯
 2  The Game of Love          ♥  5:21  ⋯
```

| Level | Piece | Widget |
|---|---|---|
| Atoms | Cover; overline; title; artist links; metadata; badges; buttons | `GtkPicture.cover.large` (`content-fit cover`) in a 1:1 `GtkAspectFrame`; `GtkLabel`s; `GtkLabel` with `<a href="swell:artist/ID">` and `activate-link`; `QualityBadge`; `GtkButton.pill` |
| Molecules | Action row | `GtkBox` (12): Play `pill suggested-action`, Shuffle `pill`, `HeartButton`, Copy link `circular flat` (`edit-copy-symbolic`), ⋯ `GtkMenuButton circular flat` |
| Organism | Header | `PageHeader` Album variant (§4.4) |
| Organism | List | Virtualized `ListView`: item 0 = header, "Disc N" as separator items (36 px, `heading dimmed`, only if there is more than 1 disc), `TrackRow` Numbered variant, final item = footer |
| Organism | Footer | Full release date and copyright (`caption dimmed`); "Credits" button → `AdwDialog` with an `AdwPreferencesGroup` per role; review: a `card` with 6 lines + "Read More" → `AdwDialog`; "More by <artist>" carousel |

Double click/Enter on a track: plays the album starting from that track.
Skeleton: 240 square + 3 bars + 8 ghost rows. Unavailable tracks: `dimmed`
row, tooltip "Not available in your region", skipped during playback.

| Decision | Principle | Effect |
|---|---|---|
| Play as the only accent action | Von Restorff, Nielsen 8 | The main action jumps out |
| Quality badges with text in the header | Nielsen 1, WCAG 1.4.1 | Quality is understandable without colour |
| Header inside the virtualized list | Perceived performance | A single scroll surface, no jumps |
| Track number → play icon on hover | Gestalt figure-ground, Fitts | The number still informs and the action appears where you are looking |

### 3.8 Artist

**Goal:** play the artist's best and explore their discography.

- Header: `AdwAvatar` 200/144 (fallback initials), overline "ARTIST", name
  `title-1`. Actions: Play `pill suggested-action` (all top tracks, loading the
  rest in the background), Shuffle `pill`, Follow `GtkToggleButton.pill`
  ("Follow" ↔ "Following" + check icon: not just colour), Radio
  `circular flat` (`swell-radio-symbolic`), ⋯ (Copy link, Block artist).
- Sections (vertical `GtkBox`, spacing 36, not virtualized: everything is
  bounded): Top Tracks (5 `TrackRow`s in a `boxed-list` + "View All" →
  virtualized page with all of them), Albums, EPs & Singles, Appears On, Artist
  Mixes (carousels), Similar Artists (round carousel), About (card with 4 lines
  of bio + "Read More" → `AdwDialog` with selectable text).
- Empty sections are not shown (never an empty carousel).

| Decision | Principle | Effect |
|---|---|---|
| Play plays all top tracks | Nielsen 2 (real expectation) | Fixes High Tide's "5 tracks" |
| Follow with changing text | Nielsen 1, WCAG 1.4.1 | Unambiguous state |
| Hide empty sections | Nielsen 8 | Less noise |

### 3.9 Playlist (own and others')

Playlist header variant: cover, overline "PLAYLIST", title, a line "by <owner>
· 120 tracks, 7 h 12 min · Updated 3 days ago", and a 3-line `dimmed`
description (tooltip with the full text; "Read More" if truncated).

| | Others' | Own |
|---|---|---|
| Actions | Play, Shuffle, ♥, Copy link, ⋯ | Play, Shuffle, Edit (`edit-symbolic`, opens a dialog), ⋯ (Make Public/Private, Copy link, Delete Playlist…) |
| Visibility | — | Label with icon + text "Public"/"Private" in the metadata line |
| Rows | `TrackRow` Cover | `TrackRow` Cover + drag handle (only with the "Custom order" sort) |
| Remove track | — | "Remove from This Playlist" menu item, Delete key; toast with Undo |
| Reorder | — | Drag by the handle; Alt+↑/↓; "Move Up/Move Down" menu items (non-drag alternative, WCAG 2.5.7) |
| Sort order | Custom order, Title, Artist, Album, Duration, Date added | Same; with any sort other than Custom the handles are hidden |

Edit/create dialog: `AdwDialog` (width 420) → `AdwToolbarView` with an
`AdwHeaderBar` (Cancel on the left, Save `suggested-action` on the right,
insensitive with an empty name) → `AdwPreferencesGroup`: `AdwEntryRow` "Name",
`GtkTextView` in a `card` frame for "Description" (max 500 characters, with a
counter), `AdwSwitchRow` "Public". Delete: `AdwAlertDialog` "Delete
“Chill”?" / "This can't be undone." / Cancel + Delete `destructive`.

Empty (own): `AdwStatusPage` "This Playlist Is Empty" / "Add tracks from
their menu or drag them here." + "Search" pill.

| Decision | Principle | Effect |
|---|---|---|
| Handles only in custom order | Nielsen 5 (error prevention), mental model | You can't "reorder" a sorted view |
| Alt+↑/↓ and a menu in addition to dragging | WCAG 2.5.7, Nielsen 7 | Reordering without motor precision |
| Confirmation only for the irreversible; Undo for the rest | Nielsen 3 and 5 | Fewer dialogs, no losses |

### 3.10 Mix / Radio

Mix header variant: the mix's cover, overline "MIX", "TRACK RADIO" or "ARTIST
RADIO", title, the mix's subtitle (`dimmed`). Actions: Play, Shuffle, ♥ (works:
mix favourite), Copy link, ⋯. `TrackRow` Cover list without an album column
when compact. Track/artist radios are opened from menus and from Now playing's
radio button.

### 3.11 "View all" / More

Generic page: the module's title; `GridView.media-grid` (cards) or `ListView`
(tracks). Pagination by limit/offset: the next page is requested when the last
visible item is within < 2 screens of the end. Last item: a spinner while
loading; an error row with "Retry" if it fails.

### 3.12 Recently played

`ListView` with section headers (GtkSectionModel): "Today", "Yesterday",
"This Week", "Earlier". 60 px row: cover 48 (round for artists), title,
"Album · Daft Punk" `dimmed`, relative time `numeric dimmed`, ⋯. Click opens;
the overlay's play button plays. Empty: "Nothing Played Yet".

### 3.13 Now playing (panel: Playing · Lyrics · Queue)

`NowPlayingPanel`: `AdwToolbarView` → `[top] AdwHeaderBar` with
`title-widget` = `AdwInlineViewSwitcher` (1.7) over an `AdwViewStack` with 3
pages; when narrow, additionally a `go-down-symbolic` "Close" button.

**Playing tab**

```
┌──────────────────────────┐
│ ┌──────────────────────┐ │  GtkAspectFrame 1:1 → GtkPicture.cover.large.card
│ │      fluid cover     │ │  (max 480, in AdwClamp)
│ └──────────────────────┘ │
│ Get Lucky             E  │  title-2 (wraps to 2 lines)
│ Daft Punk                │  title-4, links
│ Random Access Memories   │  dimmed, link
│ [HI-RES] [FLAC 24/88.2]  │  QualityBadge + format button
│ 2:01 ━━━━━━●━━━━━━ 6:09  │  ┐ narrow only (np_transport)
│  ⤮   ⏮   (▶)   ⏭   ⟳     │  ┘
│  ♥     ⊚ Radio    ⋯      │  secondary actions, circular flat 36
│ UP NEXT                  │
│ ▣ Beyond · Daft Punk   › │  1 row: opens the Queue tab
└──────────────────────────┘
```

Nothing playing: `AdwStatusPage` "Nothing Playing" / "Pick something from your
collection." When narrow, the play button is a 56 px `circular
suggested-action` and the others are 44 px.

**Lyrics tab**

- Synced: `GtkScrolledWindow` → `GtkListBox` (≤ 200 lines, needs no
  virtualization) of `GtkLabel.title-3.lyrics-line.synced` (wrap, centred when
  narrow, left-aligned when wide). The current line gets `.current` (opacity 1;
  the rest `--dim-opacity`). Click or Enter on a line → seek to its time.
- Auto-scroll: the current line is placed at 1/3 of the height (400 ms
  animation). If the user scrolls, it pauses; a floating `pill osd` button
  "Back to Current Line" appears (at the bottom, centred, `GtkOverlay`) and
  resumes it.
- Unsynced: wrapping, `selectable` `GtkLabel`, body text.
- Footer: the lyrics provider in `caption dimmed`.
- No lyrics: `AdwStatusPage.compact` "No Lyrics for This Track".

**Queue tab**

- Tab header: "Playing from <source>" (`caption dimmed`, a link to the source)
  + a `flat` "Clear" button (toast with Undo).
- Virtualized `ListView`, `TrackRow` Queue variant (54 px), section headers:
  "Now Playing", "Next in Queue" (what the user added, which plays first),
  "Next From: <source>".
- Click on a row → jump to it. ✕ per row → remove. Drag by the handle,
  Alt+↑/↓, Delete, menu (§4.11). On open, `scroll_to` the current row.
- Empty: `AdwStatusPage.compact` "Queue Is Empty" / "Use “Add to Queue” on
  any track, album or playlist."

| Decision | Principle | Effect |
|---|---|---|
| Three tabs in one panel | Gestalt common region, Nielsen 8 | Everything about "what's playing" in one place |
| "Up next" in Playing | Nielsen 1 | You see what's coming without opening the queue |
| Auto-scroll pauses while reading | Nielsen 3 | The system doesn't fight the user |
| ✕ and direct jump in the queue | Nielsen 3, Fitts | Fixes High Tide's biggest complaint |
| Current line by opacity, not size | Layout stability | No reflow: the view doesn't jump |

### 3.14 Preferences

`AdwPreferencesDialog` (search enabled), pages:

| Page | Groups and rows |
|---|---|
| Playback | **Quality:** `AdwComboRow` "Streaming Quality" (Low, High, Lossless, Hi-Res Lossless; the ones the login doesn't allow appear with the subtitle "Requires browser sign-in" and are insensitive). **Processing:** `AdwComboRow` "Volume Normalisation" (Off / Track / Album), subtitle "Off keeps the output bit-perfect"; `AdwSwitchRow` "Gapless Playback". **Output:** `AdwComboRow` "Output Device" (Automatic + PipeWire/ALSA sinks). |
| General | `AdwSwitchRow` "Run in Background" (subtitle "Keep playing when the window is closed"; requests the background portal); `AdwSwitchRow` "Track Change Notifications"; `AdwSwitchRow` "Restore Queue on Start". **Updates** (absent from builds without update checks): `AdwSwitchRow` "Check for Updates" (subtitle: what a new version does on this install); activatable `AdwActionRow` "Check Now" (subtitle: how the checks stand; spinner while checking; the result also as a toast) |
| Storage | `AdwActionRow` "Cover Cache" (subtitle with the size "128 MB") + `flat` "Clear" button; same for "Library Cache" and "Audio Cache"; confirmation via toast |
| Blocked | Two `AdwPreferencesGroup`s (Artists, Tracks); `AdwActionRow` rows with avatar/cover and a `flat` "Unblock" button; empty: `AdwStatusPage.compact` "Nothing Blocked" |
| Account | 64 `AdwAvatar` + name + plan; `AdwButtonRow` "Sign Out…" `destructive-action` → `AdwAlertDialog` |

### 3.15 Shortcuts dialog

`AdwShortcutsDialog` (1.8) with sections General, Navigation, Playback, Lists;
exact content in §5.2. Each `AdwShortcutsItem` uses `action-name` when the
action exists, so the displayed accelerator never gets out of sync.

### 3.16 About

`AdwAboutDialog`: icon, name, version, developer, website, issue tracker,
GPL-3.0-or-later licence, `debug-info` (GTK, libadwaita and GStreamer versions,
output sink, cache paths), credits, and in `comments`: "Not
affiliated with TIDAL."

---

## 4. Reusable component catalogue

### 4.1 TrackRow

A single `SwellTrackRow` class (subclass of `GtkBox`), with the variant fixed
when the factory is created; the `compact` and `reorderable` properties are
read in `bind`.

```
Cover (60 px)   [⠿] [cover 48▶] Title  [E][HI-RES]       Album         ♥  4:34  ⋯
                                Artist, Artist
Numbered (54)       [ 3 / ▶ ]   Title  [E]                              ♥  4:34  ⋯
                                feat. Artist (only if different from the album)
Queue (54)      [⠿] [cover 42]  Title  [E]                                 4:34  ✕
                                Artist
Compact (<640)      [cover 48]  Title  [E][HI-RES]                               ⋯
                                Artist · Album
```

| Piece | Widget | Detail |
|---|---|---|
| Handle | `GtkImage` `list-drag-handle-symbolic` 16 in a 24 box, `GtkDragSource` | Only `reorderable`; `grab` cursor |
| Cover + state | `GtkOverlay`: `GtkImage.cover` (pixel-size 48/42) + `GtkImage.cover-overlay` (`can-target false`) | Overlay: play on hover; pause/playing on the current row |
| Number | `GtkOverlay`: `GtkLabel.numeric.dimmed.track-number` (36 px, `xalign 1`) + `GtkImage.row-play-icon` | Numbered variant |
| Title | `GtkLabel.track-title` | `ellipsize end`, `single-line-mode` |
| Badges | `GtkLabel.badge.explicit` "E", `GtkLabel.badge.hires` "HI-RES" | Inline after the title (proximity), shown depending on the track |
| Secondary | `GtkLabel.dimmed` | Artists; when compact "Artist · Album" |
| Album | `GtkLabel.dimmed` | Column homogeneous with the text (as today); hidden when compact |
| Heart | `HeartButton` 36 | Hidden when compact and in Queue (it is in the menu) |
| Duration | `GtkLabel.numeric.dimmed`, `width-chars 5`, `xalign 1` | Hidden when compact |
| More / remove | `GtkButton.circular.flat.row-more` 36 (`view-more-symbolic`) or ✕ (`window-close-symbolic`, "Remove from Queue") | Not a `GtkMenuButton`: it opens the list's shared popover |

Rules: margins 6/12, spacing 12; fixed height per variant (the whole list uses
one variant); no menu, popover or `AdwBreakpointBin` per row. Artists/album are
not links in the rows (this keeps Tab sensible); they are reached through the
menu. Playing row: `.playing` class (title in accent + icon). The item's
accessible name is set with `ListItem::set_accessible_label`: "Get Lucky, by
Daft Punk, from Random Access Memories, 6 minutes 9 seconds, Hi-Res,
Explicit". Activation: double click / Enter (`ListView::activate`), single
click on touch (`single-click-activate` when narrow).

### 4.2 MediaCard

`GtkOverlay` → child `GtkButton.flat` (the card's area) with a vertical
`GtkBox` (6): cover (`GtkPicture.cover.large` 160/144, or `AdwAvatar` for
artists), `heading` title, `caption dimmed` subtitle; overlay: a 40 px
`circular suggested-action` play button at the bottom right of the cover
(margin 6), visible on hover or with focus inside the card (CSS opacity),
always visible on touch/when narrow. Two sibling buttons, not nested (correct
focus and screen reader behaviour).

| Type | Cover | Subtitle | Click | Play |
|---|---|---|---|---|
| Album | square | Artist · year | opens album | plays album |
| Playlist | square | "by owner" / "N tracks" | opens playlist | plays |
| Mix | square | mix subtitle | opens mix | plays |
| Artist | round `AdwAvatar`, centred text | "Artist" | opens artist | top tracks |

Accessible name: "Album: Random Access Memories, by Daft Punk". Right click
/ long press / Shift+F10: the menu for the type (§4.11).

### 4.3 CarouselSection

```
Albums                                         (‹) (›)  View All
[card] [card] [card] [card] [card] [card] [ca…
```

Vertical `GtkBox` (12): header `GtkBox` [`GtkLabel.title-3` title, hexpand;
`GtkButton.circular.flat` ‹ and › (insensitive at the ends; named "Scroll
Back"/"Scroll Forward"); `GtkButton.flat` "View All" only if there are more
items]; body `GtkScrolledWindow` (`hscrollbar-policy EXTERNAL`,
`vscrollbar NEVER`) → horizontal `GtkBox` (12) with **≤ 20 cards created at
once**. ‹ › scroll one "page" (visible width rounded to whole cards) with an
animation. The last card peeks out partially (when narrow, 144 px → ~2.2 cards
in 360 px): a signal that there is more (Gestalt continuity). When narrow, ‹ ›
are hidden (you swipe with a finger).

### 4.4 PageHeader

`GtkBox` (wide: horizontal, spacing 24; narrow: vertical, centred, spacing 12,
switched in `apply` of breakpoint C) with cover + text column + action row
(12). Variants:

| Variant | Cover | Overline | Metadata | Actions |
|---|---|---|---|---|
| Album | 240/180 square | ALBUM / EP / SINGLE | artists (links) · year · tracks, duration · badges | Play, Shuffle, ♥, Copy link, ⋯ |
| Playlist | 240/180 | PLAYLIST | by owner · tracks, duration · updated · Public/Private (own) · description | Play, Shuffle, ♥ / Edit, ⋯ |
| Artist | `AdwAvatar` 200/144 | ARTIST | — | Play, Shuffle, Follow, Radio, ⋯ |
| Mix | 240/180 | MIX / TRACK RADIO / ARTIST RADIO | subtitle | Play, Shuffle, ♥, Copy link, ⋯ |

### 4.5 QualityBadge

A `GtkLabel` with `.badge` + a modifier (`.hires`, `.explicit`). A single
factory `QualityBadge::new(kind)` sets the text, tooltip and accessible name
("Hi-Res Lossless", "Lossless", "Explicit"). Format badge: a
`GtkMenuButton.flat` with a `numeric` label "FLAC 24/96" that opens a
`GtkPopover` with a `GtkGrid` of label/value pairs: Quality, Codec, Delivered
(bits · kHz), Output device, Normalisation (Off/Track/Album). If the delivered
stream is lower than the requested one, a `caption` line explains why
("Lossless delivered: this track has no Hi-Res master").

### 4.6 HeartButton

`GtkToggleButton.circular.flat.heart`, icon `swell-heart-symbolic` ↔
`swell-heart-filled-symbolic`, tooltip "Add to Favourites" / "Remove from
Favourites", fixed accessible name "Favourite" + pressed state (provided by
the toggle). In `bind` it is linked to the `FavoritesStore` (the loaded set of
`favorites/ids`) by type + id; a change in the store notifies only the
affected visible rows. Optimistic: it changes instantly and reverts with an
error toast if the API fails. Works for track, album, artist (Follow),
playlist and mix.

### 4.7 Skeletons

`SkeletonRow` (60 px: 48 square + bars 40 % and 25 % wide), `SkeletonCard`,
`SkeletonHeader`. Only the ones that fit in the view are created (~12 rows).
`.skeleton` class; `accessible-role` `presentation` and the container
announces "Loading".

### 4.8 State components

`AdwStatusPage` (full page) or `AdwStatusPage.compact` (inside panels and
filters); `AdwBanner` for global states (offline, no GStreamer). Texts in §3
"Common states".

### 4.9 PlayerBar (wide and regular)

`GtkCenterBox` (min-height 78, margins 6/12) inside the `[bottom]` of the
`AdwToolbarView`; hidden (`reveal-bottom-bars false`) until there is something
to play or restore.

```
start (≥ 240, ≤ 30 %)          center (max 640)                 end
[▣56] Get Lucky        ♥      ⤮   ⏮   (▶)   ⏭   ⟳      [FLAC 24/88] ⏲23m ✎ ≡ 🔇━━━━ ⋯
      Daft Punk              2:01 ━━━━━━━●━━━━━━━ 6:09
```

| Zone | Piece | Widget |
|---|---|---|
| Start | 56 cover (opens Now playing, "Show Now Playing") | `GtkButton.flat` with `GtkPicture.cover` |
| | Title (link to the album) + E; artists (links) | `GtkLabel.heading` / `GtkLabel.dimmed`, markup with `<a>` |
| | Heart | `HeartButton` |
| Center | Shuffle (toggle), Previous, Play/Pause, Next, Repeat | `GtkToggleButton.circular.flat` 32; `GtkButton.circular.flat` 36; `GtkButton.circular.suggested-action` 40; repeat: a button that cycles Off → All → One, icon `repeat` / `repeat` pressed / `repeat-song`, tooltip "Repeat: All" |
| | Position, bar, duration | `GtkLabel.numeric.caption.dimmed` (5 ch), `GtkScale` (`draw-value false`, hexpand), same |
| End | Format | Format `QualityBadge` (§4.5) |
| | Timer (only when active) | `GtkLabel.numeric` with icon `swell-timer-symbolic` "23 min", tooltip "Sleep timer: stops in 23 minutes" |
| | Lyrics, Queue | `GtkToggleButton.flat` (`swell-lyrics-symbolic`, `view-list-ordered-symbolic`) |
| | Volume | `GtkButton.flat` mute (icon according to level) + 100 px `GtkScale`; at B: `GtkMenuButton` with a popover and a vertical scale |
| | More | `GtkMenuButton.flat` `view-more-symbolic`: Sleep Timer ▸ (Off, 15/30/45/60 min, End of Track), Go to Album, Go to Artist, Track Radio, Add to Playlist ▸, Copy Link |

Buffering > 300 ms: a 16 `AdwSpinner` overlaid on the play button, accessible
name "Loading". Previous: restarts the track if more than 3 s have elapsed
(consistent tooltip: "Restart Track" / "Previous Track"). Progress bar:
←/→ ±5 s, Page Up/Page Down ±30 s, name "Seek", value text "2 minutes 1
second of 6 minutes 9 seconds". Stream errors: toast with "Retry"; unavailable
track: skipped with a toast.

### 4.10 MiniPlayer (narrow)

`bottom-bar` of the `AdwBottomSheet`: vertical `GtkBox` →
`GtkProgressBar.mini-progress` (3 px, no text, decorative for the screen
reader: the position is in the sheet) + horizontal `GtkBox` (12, margins 6/12,
height 60): 42 `.cover` cover, vertical `GtkBox` (`heading` title,
`caption dimmed` artists, both ellipsized), 44 px `circular flat` Play/Pause
and Next. Tapping outside the buttons opens the sheet (native `AdwBottomSheet`
behaviour). Accessible name of the area: "Now Playing: Get Lucky by Daft
Punk. Open player".

### 4.11 Context menus

A **single `GtkPopoverMenu` per list/grid** (parent = the view,
`set_pointing_to` the rectangle of the click or of the row), static
`gio::Menu` models per type, actions from a `gio::SimpleActionGroup` "item"
that act on the object stored when opened. Triggers: right click
(`GestureClick` button 3), long press (`GestureLongPress`, touch only),
Shift+F10 / Menu key on the focused item, the row's ⋯ button.

| Type | Items (dashes are sections) |
|---|---|
| Track | Play Next · Add to Queue ─ Add to Playlist ▸ (New Playlist…, 8 recent playlists, More…) · Add to/Remove from Favourites ─ Go to Album · Go to Artist (▸ if there are several) · Track Radio ─ Copy Link ─ *(contextual)* Remove from This Playlist · Move Up · Move Down ─ Block ▸ (Block Track, Block Artist) |
| Album | Play · Shuffle · Play Next · Add to Queue ─ Add to/Remove from Favourites · Add to Playlist ▸ ─ Go to Artist · Artist Radio ─ Copy Link |
| Playlist (others') | Play · Shuffle · Play Next · Add to Queue ─ Add to/Remove from Favourites ─ Copy Link |
| Playlist (own) | Play · Shuffle · Play Next · Add to Queue ─ Edit Details… · Make Public / Make Private ─ Copy Link ─ Delete Playlist… |
| Artist | Play Top Tracks · Shuffle · Artist Radio ─ Follow / Unfollow ─ Copy Link ─ Block Artist |
| Mix / Radio | Play · Shuffle · Play Next · Add to Queue ─ Add to/Remove from Favourites ─ Copy Link |
| Queue row | Play Now ─ Move to Top · Move Up · Move Down · Remove from Queue ─ Add to Favourites · Add to Playlist ▸ ─ Go to Album · Go to Artist |

"More…" opens an `AdwDialog` "Add to Playlist" with a `GtkSearchEntry` + a
`ListView` of the user's own playlists + a "New Playlist…" row. Items that
don't apply are hidden (not disabled), except those that depend on the network
while offline, which are disabled so the user knows they exist.

### 4.12 Drag and drop

- Source: any `TrackRow` and `MediaCard` (`GtkDragSource`, `text/uri-list`
  content with `https://tidal.com/browse/track/ID`, also useful outside the
  app). Drag icon: the cover.
- Targets: the rows of the user's own playlists in the sidebar (add), the Queue
  tab (add at the end or at the drop position), rows of the own playlist in
  Custom order and of the queue (reorder, with `.drop-above`/`.drop-below`).
- Always with a non-drag alternative (menu and Alt+↑/↓).

### 4.13 Toasts and undo

`AdwToast` in the content's `AdwToastOverlay`; with an action → 8 s timeout;
without an action → 3 s. Ctrl+Z undoes the last reversible action even after
the toast has disappeared.

| Action | Toast | Button |
|---|---|---|
| Remove from favourites (in Collection) | "Removed “Get Lucky” from Favourites" | Undo |
| Remove track(s) from own playlist | "Removed from “Chill”" | Undo (re-inserts at the same index) |
| Clear queue | "Queue cleared" | Undo |
| Remove from queue | (no toast; it is immediate and visible) | Ctrl+Z |
| Add to queue / play next | "Added 12 tracks to the queue" | Show (opens Queue) |
| Add to playlist | "Added to “Running”" | Open |
| Copy link | "Link copied" | — |
| Track skipped | "Skipped “X”: not available" | — |
| Network error on an action | "Couldn't update favourites" | Retry |

Deleting a playlist and signing out don't use Undo: confirmation with an
`AdwAlertDialog`, because they are irreversible on the server or delete cache.

---

## 5. Accessibility and keyboard

### 5.1 Checklist (WCAG 2.2 AA)

- [ ] Every icon-only button has a tooltip **and** an accessible name
  (`update_property(&[Property::Label(..)])`); toggles expose pressed.
- [ ] `ListView`/`GridView` items with a complete `set_accessible_label`
  (title, artist, album, duration, quality, explicit, "playing").
- [ ] No state by colour alone: heart (shape), repeat (icon + pressed +
  tooltip), Follow (text), current row (icon + accent), badges (text), offline
  (banner with text).
- [ ] Contrast ≥ 4.5:1 for normal text and ≥ 3:1 for `title-3` (lyrics dimmed
  to `--dim-opacity`, large text). `--accent-color` (not `--accent-bg-color`)
  for accent text.
- [ ] Targets ≥ 24×24 everywhere (rows: 36), 44×44 when narrow and in the mini
  player; the sheet's play button 56.
- [ ] Focus order: sidebar → content header → content → panel → player bar.
  F6 / Shift+F6 jump between those regions.
- [ ] Visible focus (libadwaita's `:focus-visible`, without removing outlines);
  the row's ⋯ button is fully opaque when focused.
- [ ] Reordering without dragging (WCAG 2.5.7): Move Up/Down menu and Alt+↑/↓.
- [ ] Single-key shortcuts (Space, Delete) only act when focus is in
  lists/background; never in text fields (WCAG 2.1.4).
- [ ] Toasts and errors announced with `gtk::Accessible::announce` (4.14,
  medium priority) if libadwaita doesn't already do it; a track change is only
  announced if the user requested it (next/previous/double click).
- [ ] The seek and volume scales expose their value as text.
- [ ] Respect for "reduce animations" (§1.7) and high contrast (libadwaita's
  variables already change).
- [ ] UI text is translatable and never concatenated (plurals with
  `ngettext`).
- [ ] Window usable at 360×294 without horizontal scrolling.

### 5.2 Keyboard map

Playback shortcuts with arrows and Space go in a `GtkEventControllerKey` on the
window in the **capture phase**, which lets the event through if focus is in a
`gtk::Editable` (and, for Space, also in buttons, switches and scales, where
Space has its native meaning). The rest are registered with
`set_accels_for_action`.

| Group | Shortcut | Action |
|---|---|---|
| General | Ctrl+F | Go to Search and focus the field |
| | Alt+1 / 2 / 3 / 4 | Home / Explore / Search / Collection |
| | F9 | Show/hide sidebar (wide) |
| | F6 / Shift+F6 | Next / previous focus region |
| | F5 | Reload the page |
| | Alt+← (and the mouse "back" button) | Go back |
| | Esc | Close overlaid panel / sheet / popover; clear search |
| | Ctrl+Z | Undo the last reversible action |
| | Ctrl+, | Preferences |
| | Ctrl+? | Keyboard Shortcuts |
| | Ctrl+W | Close window (stays in the background if enabled) |
| | Ctrl+Q | Quit |
| Panel | Ctrl+Shift+N | Now Playing (open/close) |
| | Ctrl+Shift+L | Lyrics |
| | Ctrl+L | Queue |
| Playback | Space | Play/Pause (not in text/buttons) |
| | Ctrl+Space | Play/Pause in any context |
| | Ctrl+→ / Ctrl+← | Next / previous (restarts if > 3 s) |
| | Shift+→ / Shift+← | Forward / back 10 s |
| | Ctrl+↑ / Ctrl+↓ | Volume ±5 % |
| | Ctrl+M | Mute |
| | Ctrl+S | Shuffle |
| | Ctrl+R | Repeat (Off → All → One) |
| | Ctrl+D | Favourite the current track |
| | Media keys | Via MPRIS (play, pause, stop, next, previous) |
| Seek bar with focus | ← / → · Page Up / Page Down · Home / End | ±5 s · ±30 s · start / end |
| Lists | ↑ ↓ · Home / End | Move |
| | Enter | Play track / open item |
| | Shift+F10 / Menu | Context menu |
| | Delete | Remove (queue, own playlist; with Undo) |
| | Alt+↑ / Alt+↓ | Move row (queue, own playlist in Custom order) |
| | Type | Filter (Collection subviews) |

---

## 6. Performance constraints the design respects

1. **Virtualization is mandatory** for anything that can exceed ~50 items:
   favourites, playlists, queue, results by type, Collection grids, View all,
   Recently played, sidebar. `ListView`/`GridView` as a **direct child** of the
   `ScrolledWindow` (or of an `AdwClampScrollable`, which keeps virtualization
   and limits lists to 1200 px wide). Never inside a `GtkBox` with other
   siblings in the same scroll: page headers, discs and footers go as model
   items.
2. **Fixed heights per variant** (60/54/54/44/36 px): scrolling is exact and the
   position of the centre of the view translates directly into an index for the
   `CoverLoader` (as `library.rs` does today).
3. **Lightweight rows:** at most ~12 widgets per row, no `GMenuModel`,
   `GtkPopover`, `AdwBreakpointBin`, `AdwAvatar` with a large image or shadows
   (`card`) per row. The menu and popover are one per list; compactness is
   decided by one `AdwBreakpointBin` per page.
4. **Lazy covers:** they are only requested on `bind`; the ticket is released
   on `unbind` (cancelling the download if it is no longer visible); priority by
   closeness to the centre of the view; size according to §1.5; in-memory LRU
   cache (≈ 64 MB of textures) + disk. In carousels and Home, only the cards
   within the view are requested (± 1 vertical screen, ± 1 horizontal page).
5. **Bounded eager carousels:** ≤ 20 cards created at once; the rest is in View
   all. Home creates modules as they arrive and requests more by cursor when
   nearing the end.
6. **Small non-virtualized lists allowed:** artist top tracks (5), Home track
   modules (5), lyrics (≤ 200), shortcuts (≤ 8), Preferences.
7. **No per-row work on the UI thread:** formatting of durations and accessible
   names computed when the data loads, not on every `bind` (or cached).
   Filtering and sorting with `FilterListModel`/`SortListModel` (incremental).
8. **Favourite changes without rebuilding lists:** the `FavoritesStore`
   notifies by id; only the bound visible rows are updated.
9. **Animations of opacity/transform only;** nothing that causes a relayout per
   frame (the current lyrics line doesn't change size).
10. **Skeleton with a 150 ms delay** and limited to what fits on screen; the
    SQLite cache is painted before validating against the network (which the
    favourites list already does today).
11. **Search:** 250 ms debounce, cancellation of the previous request, without
    emptying results while the new response arrives.

---

## 7. Code map (suggested)

| Module | Content |
|---|---|
| `ui/window.rs` | Shell (§2), breakpoints, window actions, shortcuts |
| `ui/sidebar.rs` | Model and factory shared by the sidebar and the Collection index |
| `ui/pages/` | `home`, `explore`, `module_page`, `search`, `collection/*`, `album`, `artist`, `playlist`, `mix`, `view_all`, `history`, `login` |
| `ui/widgets/` | `track_row` (evolution of the current one), `track_list` (ListView + popover + DnD), `media_card`, `carousel`, `page_header`, `quality_badge`, `heart_button`, `skeleton`, `player_bar`, `mini_player`, `now_playing`, `lyrics_view`, `queue_view` |
| `ui/menus.rs` | `gio::Menu` models per type and the "item" action group |
| `ui/dialogs/` | Edit playlist, add to playlist, credits, bio/review |
| `data/style.css` | §1.8 |
| `data/icons/` | `swell-*` icons (§1.9) |

Changes relative to the current code: `track_row.rs` swaps the `card` class on
the cover (a shadow per row) for `.cover`, moves Hi-Res next to the title as a
badge with text, and adds a state overlay, heart, ⋯ and an accessible name;
`player_bar.rs` becomes a `GtkCenterBox` with full transport, volume and panel
buttons; `window.rs` replaces the flat stack with the shell from §2.1.
