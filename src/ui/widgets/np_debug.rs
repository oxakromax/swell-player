//! Developer aid for the player widgets, inert unless
//! `SWELL_DEBUG_PANEL=playing|lyrics|queue` is set:
//!
//! - opens a separate window with a [`NowPlayingPanel`] on that tab
//!   (`SWELL_DEBUG_PANEL_SIZE=WxH`, default 420x820; `SWELL_DEBUG_NARROW=1`
//!   for the narrow layout, `SWELL_DEBUG_COMPACT=1` for the compact bar),
//!   with a [`MiniPlayer`] under it;
//! - plays album 529955456 (YOASOBI, Hi-Res, synced lyrics) from track
//!   529955457 with two tracks queued by hand, jumps to 0:45, plays 3 s
//!   and pauses;
//! - with `SWELL_DEBUG_SHOT=<file.png>`, after `SWELL_DEBUG_SHOT_MS`
//!   (default 8000) renders the panel on each tab to
//!   `<file>-<tab>.png`, then stops playback.
//!
//! It also loads a TEMPORARY copy of the docs/design.md §1.8 rules these
//! widgets use, until the application stylesheet lands.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, glib};

use super::super::context::{Ctx, PanelTab};
use super::mini_player::MiniPlayer;
use super::now_playing::NowPlayingPanel;
use super::player_bar::PlayerBar;
use crate::model::TrackInfo;
use crate::playback::{PlayState, Source};
use crate::runtime;
use crate::tidal::pages::Target;

const ALBUM: u64 = 529_955_456;
const TRACK: u64 = 529_955_457;
/// Where to jump once playing (a line with lyrics), and for how long to
/// play before pausing (keep test audio short).
const SEEK_TO: f64 = 45.0;
const PLAY_FOR: Duration = Duration::from_secs(3);

/// TEMPORARY: the §1.8 rules for these widgets (the app stylesheet will
/// replace this), plus links in the player inheriting the text colour.
const CSS: &str = "
.cover { border-radius: 6px; }
.cover.large { border-radius: 12px; }
.track-row.playing .track-title { color: var(--accent-color); }
.badge { min-height: 16px; padding: 0 5px; border-radius: 4px; font-size: smaller; font-weight: 800;
  letter-spacing: 0.04em; box-shadow: inset 0 0 0 1px color-mix(in srgb, currentColor 40%, transparent); }
.badge.hires { color: var(--accent-color);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-color) 70%, transparent); }
.badge.explicit { min-width: 16px; padding: 0; box-shadow: none;
  background-color: color-mix(in srgb, currentColor 16%, transparent); }
.heart:checked { color: var(--accent-color); }
.lyrics-line { padding: 6px 12px; border-radius: 12px; opacity: var(--dim-opacity);
  transition: opacity 250ms ease-out; }
.lyrics-line.current { opacity: 1; }
.track-row.drop-above { box-shadow: inset 0 2px var(--accent-bg-color); }
.track-row.drop-below { box-shadow: inset 0 -2px var(--accent-bg-color); }
.mini-progress trough, .mini-progress progress { min-height: 3px; border-radius: 0; }
.np-link link { color: inherit; text-decoration: none; }
.np-link link:hover { text-decoration: underline; }
";

thread_local! {
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn start(ctx: &Rc<Ctx>, bar: &Rc<PlayerBar>) {
    let Ok(tab) = std::env::var("SWELL_DEBUG_PANEL") else {
        return;
    };
    if STARTED.replace(true) {
        return;
    }
    let tab = match tab.as_str() {
        "lyrics" => PanelTab::Lyrics,
        "queue" => PanelTab::Queue,
        _ => PanelTab::Playing,
    };
    let flag = |name: &str| std::env::var(name).is_ok_and(|v| v == "1");
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    bar.set_compact(flag("SWELL_DEBUG_COMPACT"));

    let (width, height) = std::env::var("SWELL_DEBUG_PANEL_SIZE")
        .ok()
        .and_then(|size| {
            let (w, h) = size.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((420, 820));
    let panel = NowPlayingPanel::new(ctx);
    panel.set_narrow(flag("SWELL_DEBUG_NARROW"));
    panel.show_tab(tab);
    // The mini player sits under the panel, as under the narrow layout's
    // content.
    let mini = MiniPlayer::new(ctx);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.widget.set_vexpand(true);
    content.append(&panel.widget);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&mini.widget);
    let window = adw::Window::builder()
        .title("Now Playing (debug)")
        .default_width(width)
        .default_height(height)
        .content(&content)
        .build();
    let keep = RefCell::new(Some((panel.clone(), mini)));
    window.connect_destroy(move |_| drop(keep.take()));
    window.present();

    let ctx_weak = Rc::downgrade(ctx);
    let api = ctx.api.clone();
    glib::spawn_future_local(async move {
        let tracks = runtime::spawn(async move { api.album_tracks(ALBUM, 0, 50).await }).await;
        let Some(ctx) = ctx_weak.upgrade() else {
            return;
        };
        let tracks: Vec<TrackInfo> = match tracks {
            Ok(page) => page.items.into_iter().map(TrackInfo::from).collect(),
            Err(err) => return log::warn!("debug panel: {err:#}"),
        };
        let start = tracks.iter().position(|t| t.id == TRACK).unwrap_or(0);
        let queued: Vec<TrackInfo> = tracks.iter().rev().take(2).cloned().collect();
        let title = tracks.first().map(|t| t.album.clone()).unwrap_or_default();
        ctx.playback.play_tracks(
            tracks,
            start,
            Source {
                title,
                target: Some(Target::Album(ALBUM)),
            },
        );
        ctx.playback.add_to_queue(queued);
        // Once it plays (the engine may still say "buffering"): jump into
        // the song, let it run briefly, pause.
        let playback = Rc::downgrade(&ctx.playback);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(playback) = playback.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let started = playback.state() == PlayState::Playing
                || playback.position().is_some_and(|p| p > 0.3);
            if !started {
                return glib::ControlFlow::Continue;
            }
            playback.seek(SEEK_TO);
            let playback = Rc::downgrade(&playback);
            glib::timeout_add_local_once(PLAY_FOR, move || {
                if let Some(playback) = playback.upgrade() {
                    playback.pause();
                }
            });
            glib::ControlFlow::Break
        });
    });

    let Ok(path) = std::env::var("SWELL_DEBUG_SHOT") else {
        return;
    };
    let delay = std::env::var("SWELL_DEBUG_SHOT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8000);
    let ctx = Rc::downgrade(ctx);
    glib::timeout_add_local_once(Duration::from_millis(delay), move || {
        shoot(window, panel, path, ctx, 0);
    });
}

/// Renders the panel on each tab in turn, starting with the requested one.
fn shoot(
    window: adw::Window,
    panel: Rc<NowPlayingPanel>,
    path: String,
    ctx: std::rc::Weak<Ctx>,
    step: usize,
) {
    let tabs = [
        (PanelTab::Playing, "playing"),
        (PanelTab::Lyrics, "lyrics"),
        (PanelTab::Queue, "queue"),
    ];
    if step > 0 {
        let (_, name) = tabs[step - 1];
        let file = match path.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}-{name}.{ext}"),
            None => format!("{path}-{name}"),
        };
        save(&window, &file);
    }
    let Some(&(tab, _)) = tabs.get(step) else {
        if let Some(ctx) = ctx.upgrade() {
            ctx.playback.stop();
        }
        return;
    };
    panel.show_tab(tab);
    glib::timeout_add_local_once(Duration::from_millis(2500), move || {
        shoot(window, panel, path, ctx, step + 1);
    });
}

fn save(window: &adw::Window, path: &str) {
    let Some(content) = window.content() else {
        return;
    };
    let (width, height) = (content.width(), content.height());
    let paintable = gtk::WidgetPaintable::new(Some(&content));
    let snapshot = gtk::Snapshot::new();
    let background = if adw::StyleManager::default().is_dark() {
        gdk::RGBA::new(0.133, 0.133, 0.149, 1.0)
    } else {
        gdk::RGBA::new(0.98, 0.98, 0.984, 1.0)
    };
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    snapshot.append_color(&background, &bounds);
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) else {
        return log::warn!("debug panel: nothing to render");
    };
    let texture = renderer.render_texture(node, Some(&bounds));
    match texture.save_to_png(path) {
        Ok(()) => log::warn!("debug panel: saved {path}"),
        Err(err) => log::warn!("debug panel: {err}"),
    }
}
