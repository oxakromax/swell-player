//! Reusable widgets (docs/design.md §4).

pub mod carousel;
pub mod heart_button;
pub mod lyrics_view;
pub mod media_card;
pub mod mini_player;
pub mod now_playing;
pub mod page_header;
pub mod player_bar;
pub mod quality_badge;
pub mod queue_view;
pub mod section;
pub mod skeleton;
pub mod status;
pub mod track_list;
pub mod track_row;

use gtk::prelude::*;
use gtk::gdk;

/// `preferred` if the icon theme has it, `fallback` otherwise. The `swell-*`
/// icons (docs/design.md §1.9) come from the app's resources; until they are
/// installed, a standard icon stands in.
pub fn icon_name(preferred: &'static str, fallback: &'static str) -> &'static str {
    let Some(display) = gdk::Display::default() else {
        return fallback;
    };
    if gtk::IconTheme::for_display(&display).has_icon(preferred) {
        preferred
    } else {
        fallback
    }
}

/// Device pixels for `logical` pixels on `widget`'s surface.
pub fn device_pixels(widget: &impl IsA<gtk::Widget>, logical: i32) -> u32 {
    (logical.max(1) * widget.scale_factor().max(1)) as u32
}

/// Sets the accessible name of an icon-only button and its tooltip.
pub fn label_button(button: &impl IsA<gtk::Widget>, label: &str) {
    button.set_tooltip_text(Some(label));
    button
        .upcast_ref::<gtk::Widget>()
        .update_property(&[gtk::accessible::Property::Label(label)]);
}

/// TEMPORARY development stylesheet: the rules of docs/design.md §1.8 that
/// these widgets rely on, plus the few extra classes they introduce
/// (`.media-card*`, `.cover-placeholder`, `.chip`). It is only loaded with
/// `SWELL_DEV_CSS=1` and goes away once `data/style.css` ships them.
pub fn install_dev_css() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|done| done.replace(true)) || std::env::var_os("SWELL_DEV_CSS").is_none() {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(DEV_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    log::debug!("development stylesheet loaded");
}

const DEV_CSS: &str = r#"
.cover { border-radius: 6px; }
.cover.large { border-radius: 12px; }
.cover-overlay { border-radius: 6px; background-color: alpha(black, 0.5); color: white;
  opacity: 0; transition: opacity 150ms ease-out; }
listview > row:hover .cover-overlay, .track-row.playing .cover-overlay { opacity: 1; }
.track-row.playing .track-title { color: var(--accent-color); }
.track-number, .row-play-icon { transition: opacity 150ms ease-out; }
.row-play-icon { opacity: 0; }
listview > row:hover .track-number { opacity: 0; }
listview > row:hover .row-play-icon { opacity: 1; }
.track-row.playing .track-number { opacity: 0; }
.track-row.playing .row-play-icon { opacity: 1; color: var(--accent-color); }
.row-more { opacity: 0.55; transition: opacity 150ms ease-out; }
listview > row:hover .row-more, .row-more:focus-visible { opacity: 1; }
.badge { min-height: 16px; padding: 0 5px; border-radius: 4px; font-size: smaller;
  font-weight: 800; letter-spacing: 0.04em;
  box-shadow: inset 0 0 0 1px color-mix(in srgb, currentColor 40%, transparent); }
.badge.hires { color: var(--accent-color);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent-color) 70%, transparent); }
.badge.explicit { min-width: 16px; padding: 0; box-shadow: none;
  background-color: color-mix(in srgb, currentColor 16%, transparent); }
.heart:checked { color: var(--accent-color); animation: heart-pop 250ms ease-out; }
@keyframes heart-pop { 50% { transform: scale(1.25); } }
.skeleton { background-color: color-mix(in srgb, currentColor 10%, transparent);
  border-radius: 6px; animation: skeleton-pulse 1200ms ease-in-out infinite alternate; }
@keyframes skeleton-pulse { to { opacity: 0.45; } }
.track-row.drop-above { box-shadow: inset 0 2px var(--accent-bg-color); }
.track-row.drop-below { box-shadow: inset 0 -2px var(--accent-bg-color); }
.page-content { margin-left: 24px; margin-right: 24px; }
window.narrow .page-content { margin-left: 12px; margin-right: 12px; }
.cover-placeholder { background-color: color-mix(in srgb, currentColor 10%, transparent); }
.media-card-button { padding: 6px; border-radius: 12px; }
.media-card-play { opacity: 0; transition: opacity 150ms ease-out; }
.media-card:hover .media-card-play, .media-card:focus-within .media-card-play { opacity: 1; }
.chip { border-radius: 9999px; padding: 6px 14px; }
"#;
