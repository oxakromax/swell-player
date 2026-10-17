//! Hooks for the benchmark driver in `bench/`, enabled by environment
//! variables and inert otherwise:
//!
//! - `SWELL_BENCH_AUTOSCROLL=1`: once favourites are on screen, wait
//!   `SWELL_BENCH_AUTOSCROLL_DELAY_MS` (500), then every
//!   `SWELL_BENCH_SCROLL_INTERVAL_MS` (100) scroll down by
//!   `SWELL_BENCH_SCROLL_PAGE_FRACTION` (1.0) of a page. At the bottom, with
//!   every favourite loaded, logs `timing: scrolled to end`.
//! - `SWELL_BENCH_PLAY=1`: then play the first Hi-Res favourite (right after
//!   loading when not autoscrolling). `SWELL_BENCH_PLAY=<track id>` plays
//!   that favourite instead.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::model::TrackInfo;

pub fn autoscroll_enabled() -> bool {
    std::env::var_os("SWELL_BENCH_AUTOSCROLL").is_some()
}

pub fn play_enabled() -> bool {
    std::env::var_os("SWELL_BENCH_PLAY").is_some()
}

/// The favourite `SWELL_BENCH_PLAY` asks for.
pub fn wants_track(track: &TrackInfo) -> bool {
    match std::env::var("SWELL_BENCH_PLAY")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(id) if id > 1 => track.id == id,
        _ => track.hires,
    }
}

fn env_number(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Scrolls `scrolled` to the end page by page. `loaded` turns true once the
/// whole list is in; `at_end` runs once the bottom has held for two ticks.
pub fn autoscroll(
    scrolled: &gtk::ScrolledWindow,
    loaded: Rc<Cell<bool>>,
    at_end: impl FnOnce() + 'static,
) {
    let delay = Duration::from_millis(env_number("SWELL_BENCH_AUTOSCROLL_DELAY_MS", 500.0) as u64);
    let interval =
        Duration::from_millis(env_number("SWELL_BENCH_SCROLL_INTERVAL_MS", 100.0).max(1.0) as u64);
    let fraction = env_number("SWELL_BENCH_SCROLL_PAGE_FRACTION", 1.0);
    let adjustment = scrolled.vadjustment();
    let mut at_end = Some(at_end);

    glib::timeout_add_local_once(delay, move || {
        let mut ticks_at_bottom = 0;
        glib::timeout_add_local(interval, move || {
            let bottom = adjustment.upper() - adjustment.page_size();
            adjustment
                .set_value((adjustment.value() + adjustment.page_size() * fraction).min(bottom));
            if loaded.get() && adjustment.value() >= bottom - 1.0 {
                ticks_at_bottom += 1;
            } else {
                ticks_at_bottom = 0;
            }
            if ticks_at_bottom < 2 {
                return glib::ControlFlow::Continue;
            }
            crate::timing::mark("scrolled to end");
            if let Some(at_end) = at_end.take() {
                at_end();
            }
            glib::ControlFlow::Break
        });
    });
}
