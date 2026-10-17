// Swell: a native TIDAL client for Linux.
// SPDX-License-Identifier: GPL-3.0-or-later

mod api;
mod bench;
mod cache;
mod config;
mod covers;
mod model;
mod player;
mod runtime;
mod session;
mod timing;
mod ui;

use adw::prelude::*;
use gtk::glib;

fn main() -> glib::ExitCode {
    timing::start();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    if let Err(err) = gst::init() {
        eprintln!("GStreamer could not be initialised: {err}");
        return glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .build();
    app.connect_activate(ui::window::build);
    // Quit cleanly (stopping playback, flushing the cache) on SIGTERM/SIGINT.
    for signal in [libc_signal::SIGTERM, libc_signal::SIGINT] {
        let app = app.downgrade();
        glib_unix::unix_signal_add_local(signal, move || {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
            glib::ControlFlow::Break
        });
    }
    app.run()
}

/// Signal numbers, identical on every Linux architecture.
mod libc_signal {
    pub const SIGINT: i32 = 2;
    pub const SIGTERM: i32 = 15;
}
