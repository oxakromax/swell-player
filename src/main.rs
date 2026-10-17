// Swell: a native TIDAL client for Linux.
// SPDX-License-Identifier: GPL-3.0-or-later

mod api;
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
    app.run()
}
