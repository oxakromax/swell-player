//! Startup milestones, logged at `info` level (`RUST_LOG=swell_player=info`)
//! as milliseconds since the process started (not since `main`, so dynamic
//! linking and library constructors count too).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gtk::prelude::*;

static START: OnceLock<Instant> = OnceLock::new();

pub fn start() {
    START.get_or_init(|| {
        let now = Instant::now();
        since_process_start()
            .and_then(|elapsed| now.checked_sub(elapsed))
            .unwrap_or(now)
    });
}

/// Time since the kernel started this process: system uptime minus the
/// process start time from `/proc/self/stat` (in USER_HZ ticks, which the
/// Linux ABI fixes at 100 per second). 10 ms resolution.
#[cfg(target_os = "linux")]
fn since_process_start() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the parenthesised command name; starttime is field 22.
    let after_comm = &stat[stat.rfind(')')? + 2..];
    let start_ticks: f64 = after_comm.split(' ').nth(19)?.parse().ok()?;
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let uptime: f64 = uptime.split(' ').next()?.parse().ok()?;
    Duration::try_from_secs_f64(uptime - start_ticks / 100.0).ok()
}

/// Elsewhere the milestones count from `main`.
#[cfg(not(target_os = "linux"))]
fn since_process_start() -> Option<Duration> {
    None
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
