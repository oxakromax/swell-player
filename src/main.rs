// Swell: a native TIDAL client for Linux.
// SPDX-License-Identifier: GPL-3.0-or-later

mod api;
mod cache;
mod config;
mod covers;
mod library;
mod model;
mod tidal;
mod playback;
mod settings;
mod runtime;
mod session;
mod timing;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

/// Icons (`swell-*` symbolic icons and the app icon), compiled by build.rs.
const RESOURCES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/swell.gresource"));
const RESOURCE_PATH: &str = "/io/github/oxakromax/SwellPlayer";

fn main() -> glib::ExitCode {
    timing::start();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    if let Err(err) = gst::init() {
        eprintln!("GStreamer could not be initialised: {err}");
        return glib::ExitCode::FAILURE;
    }
    match gio::Resource::from_data(&glib::Bytes::from_static(RESOURCES)) {
        Ok(resource) => gio::resources_register(&resource),
        Err(err) => log::warn!("could not load the bundled icons: {err}"),
    }

    // Links (`tidal://…`, TIDAL web URLs) arrive through `open`.
    let mut flags = gio::ApplicationFlags::HANDLES_OPEN;
    if std::env::var_os("SWELL_SCREENSHOT").is_some() {
        // Screenshot runs (ui/debug.rs) may run beside a normal instance.
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder()
        .application_id(config::APP_ID)
        .flags(flags)
        .build();
    app.connect_startup(startup);
    app.connect_activate(ui::window::activate);
    app.connect_open(|app, files, _hint| {
        let uris: Vec<String> = files.iter().map(|file| file.uri().into()).collect();
        ui::window::open_links(app, &uris);
    });
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

/// Style sheet, icons, app actions and accelerators.
fn startup(app: &adw::Application) {
    if let Some(display) = gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(include_str!("../data/style.css"));
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::IconTheme::for_display(&display).add_resource_path(&format!("{RESOURCE_PATH}/icons"));
    }
    gtk::Window::set_default_icon_name(config::APP_ID);

    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| ui::window::quit(app))
        .build();
    app.add_action_entries([quit]);
    for (action, accels) in ui::window::ACCELS {
        app.set_accels_for_action(action, accels);
    }
}

/// Signal numbers, identical on every Linux architecture.
mod libc_signal {
    pub const SIGINT: i32 = 2;
    pub const SIGTERM: i32 = 15;
}
