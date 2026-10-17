//! Startup milestones, logged at `info` level (`RUST_LOG=swell_player=info`)
//! as milliseconds since the process started. Used for the benchmarks in the
//! README.

use std::sync::OnceLock;
use std::time::Instant;

use gtk::prelude::*;

static START: OnceLock<Instant> = OnceLock::new();

pub fn start() {
    START.get_or_init(Instant::now);
}

pub fn mark(what: &str) {
    if let Some(start) = START.get() {
        log::info!("timing: {what} after {} ms", start.elapsed().as_millis());
    }
}

/// Logs `what` once the next frame containing `widget` has been painted.
pub fn mark_after_next_frame(widget: &impl IsA<gtk::Widget>, what: &'static str) {
    let Some(clock) = widget.frame_clock() else {
        return mark(what);
    };
    let handler = std::rc::Rc::new(std::cell::Cell::new(None));
    let id = clock.connect_after_paint({
        let handler = handler.clone();
        move |clock| {
            mark(what);
            if let Some(id) = handler.take() {
                clock.disconnect(id);
            }
        }
    });
    handler.set(Some(id));
}
