//! Developer aids, inert unless their environment variable is set:
//!
//! - `SWELL_ROUTE=<route>` opens a page once signed in: `home`, `explore`,
//!   `search:<query>`, `tracks`, `albums`, `artists`, `playlists`, `mixes`,
//!   `album:<id>`, `artist:<id>`, `playlist:<uuid>`, `mix:<id>`,
//!   `radio:<track id>`, `page:<api path>`.
//! - `SWELL_SCREENSHOT=<file.png>` renders the window to a PNG after
//!   `SWELL_SCREENSHOT_DELAY_MS` (default 5000) and quits. Several
//!   comma-separated `SWELL_SCREENSHOT_SIZES=WxH,...` take one shot per
//!   window size (`file-WxH.png`).
//! - `SWELL_PANEL=playing|lyrics|queue` opens the now-playing panel.

use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::context::PanelTab;
use super::nav::{CollectionView, Route};

pub fn route() -> Option<Route> {
    let value = std::env::var("SWELL_ROUTE").ok()?;
    let (kind, arg) = value.split_once(':').unwrap_or((value.as_str(), ""));
    Some(match kind {
        "home" => Route::Home,
        "explore" => Route::Explore,
        "search" => Route::Search((!arg.is_empty()).then(|| arg.to_owned())),
        "tracks" => Route::Collection(CollectionView::Tracks),
        "albums" => Route::Collection(CollectionView::Albums),
        "artists" => Route::Collection(CollectionView::Artists),
        "playlists" => Route::Collection(CollectionView::Playlists),
        "mixes" => Route::Collection(CollectionView::Mixes),
        "album" => Route::Album(arg.parse().ok()?),
        "artist" => Route::Artist(arg.parse().ok()?),
        "playlist" => Route::Playlist(arg.to_owned()),
        "mix" => Route::Mix(arg.to_owned()),
        "radio" => Route::TrackRadio(arg.parse().ok()?),
        "page" => Route::Page {
            path: arg.to_owned(),
            title: String::new(),
        },
        _ => return None,
    })
}

pub fn panel() -> Option<PanelTab> {
    match std::env::var("SWELL_PANEL").ok()?.as_str() {
        "playing" => Some(PanelTab::Playing),
        "lyrics" => Some(PanelTab::Lyrics),
        "queue" => Some(PanelTab::Queue),
        _ => None,
    }
}

/// Schedules the screenshot(s) if `SWELL_SCREENSHOT` is set.
pub fn schedule_screenshot(window: &gtk::Window) {
    let Ok(path) = std::env::var("SWELL_SCREENSHOT") else {
        return;
    };
    let delay = std::env::var("SWELL_SCREENSHOT_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    let sizes: Vec<(i32, i32)> = std::env::var("SWELL_SCREENSHOT_SIZES")
        .ok()
        .map(|v| {
            v.split(',')
                .filter_map(|size| {
                    let (w, h) = size.split_once('x')?;
                    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let window = window.clone();
    glib::timeout_add_local_once(Duration::from_millis(delay), move || {
        if sizes.is_empty() {
            save(&window, &path);
            quit(&window);
            return;
        }
        shoot_sizes(window, path, sizes, 0);
    });
}

fn shoot_sizes(window: gtk::Window, path: String, sizes: Vec<(i32, i32)>, index: usize) {
    let Some(&(width, height)) = sizes.get(index) else {
        return quit(&window);
    };
    window.unmaximize();
    window.set_default_size(width, height);
    // Give the layout (and breakpoints) time to settle at the new size.
    glib::timeout_add_local_once(Duration::from_millis(1500), move || {
        let file = match path.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}-{width}x{height}.{ext}"),
            None => format!("{path}-{width}x{height}"),
        };
        save(&window, &file);
        shoot_sizes(window, path, sizes, index + 1);
    });
}

fn save(window: &gtk::Window, path: &str) {
    // A paintable of the toplevel itself renders nothing; its child does.
    let Some(content) = window.child() else {
        return log::warn!("screenshot: empty window");
    };
    let (width, height) = (content.width(), content.height());
    let paintable = gtk::WidgetPaintable::new(Some(&content));
    let snapshot = gtk::Snapshot::new();
    // The window background is drawn by the toplevel, not the content.
    let background = if adw::StyleManager::default().is_dark() {
        gdk::RGBA::new(0.133, 0.133, 0.149, 1.0)
    } else {
        gdk::RGBA::new(0.98, 0.98, 0.984, 1.0)
    };
    snapshot.append_color(
        &background,
        &gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
    );
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let Some(node) = snapshot.to_node() else {
        return log::warn!("screenshot: nothing to render ({width}×{height}, mapped {})", content.is_mapped());
    };
    let Some(renderer) = window.native().map(|n| n.renderer()).flatten() else {
        return log::warn!("screenshot: no renderer");
    };
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture: gdk::Texture = renderer.render_texture(node, Some(&bounds));
    match texture.save_to_png(path) {
        Ok(()) => log::warn!("screenshot saved to {path} ({width}×{height})"),
        Err(err) => log::warn!("screenshot failed: {err}"),
    }
}

fn quit(window: &gtk::Window) {
    if let Some(app) = window.application() {
        app.quit();
    }
}
