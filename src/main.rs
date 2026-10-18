// Swell: a native TIDAL client.
// SPDX-License-Identifier: GPL-3.0-or-later

// No console window behind the app in Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod api;
#[cfg(any(windows, target_os = "macos"))]
mod bundle;
mod cache;
mod config;
mod covers;
mod i18n;
mod library;
mod mem;
mod model;
mod playback;
mod runtime;
mod session;
mod settings;
mod tidal;
mod timing;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

/// Icons (`swell-*` symbolic icons and the app icon), compiled by build.rs.
const RESOURCES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/swell.gresource"));
const RESOURCE_PATH: &str = "/io/github/oxakromax/SwellPlayer";

fn main() -> glib::ExitCode {
    // First: before anything reads the environment or starts a thread.
    #[cfg(any(windows, target_os = "macos"))]
    bundle::setup();
    #[cfg(windows)]
    set_app_user_model_id();
    timing::start();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Some(language) = i18n::init(&settings::saved_language()) {
        // GTK's and libadwaita's own strings follow the chosen language.
        // GLib's setenv, not std's: on Windows libintl reads the C
        // runtime's copy of the environment, which only `_wputenv` (called
        // by GLib, beside SetEnvironmentVariableW) updates. No other thread
        // exists yet (GStreamer, GTK and the runtime start below).
        // Safety: with no other thread, nothing reads the environment
        // concurrently.
        if let Err(err) = unsafe { glib::setenv("LANGUAGE", language, true) } {
            log::warn!("could not set the interface language: {err}");
        }
    }

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
    #[cfg(unix)]
    for signal in [libc_signal::SIGTERM, libc_signal::SIGINT] {
        let app = app.downgrade();
        glib_unix::unix_signal_add_local(signal, move || {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
            glib::ControlFlow::Break
        });
    }
    let code = app.run();
    // Let go of the application (and its bus name) before a restart
    // starts the next instance.
    drop(app);
    if ui::window::restart_requested() {
        relaunch();
    }
    code
}

/// Groups the running window with the shortcuts the installer made (which
/// carry this ID) on the taskbar, so a pinned Swell doesn't show twice.
#[cfg(windows)]
fn set_app_user_model_id() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> i32;
    }
    let id: Vec<u16> = config::APP_ID.encode_utf16().chain([0]).collect();
    // Safety: `id` is a NUL-terminated UTF-16 string that outlives the call,
    // which copies it.
    let result = unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    if result != 0 {
        log::debug!("could not set the AppUserModelID: HRESULT {result:#x}");
    }
}

/// Starts this program again (after the Language preference changed).
fn relaunch() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => return log::warn!("cannot restart: {err}"),
    };
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/.flatpak-info").exists() {
        if let Err(err) = relaunch_in_flatpak(&exe) {
            log::warn!("cannot restart: {err}");
        }
        return;
    }
    let mut command = std::process::Command::new(exe);
    i18n::restore_environment(&mut command);
    if let Err(err) = command.spawn() {
        log::warn!("cannot restart: {err}");
    }
}

/// Inside Flatpak the sandbox ends with this process, children included,
/// so the Flatpak portal starts the new instance in a sandbox of its own.
/// The call returns once it runs. The new sandbox gets the session's
/// environment, not this process's, so the `LANGUAGE` set in `main` stays
/// behind.
#[cfg(target_os = "linux")]
fn relaunch_in_flatpak(exe: &std::path::Path) -> Result<(), glib::Error> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    bus.call_sync(
        Some("org.freedesktop.portal.Flatpak"),
        "/org/freedesktop/portal/Flatpak",
        "org.freedesktop.portal.Flatpak",
        "Spawn",
        Some(&spawn_args(&cwd, exe)),
        Some(glib::VariantTy::new("(u)").expect("valid type")),
        gio::DBusCallFlags::NONE,
        10_000,
        gio::Cancellable::NONE,
    )?;
    Ok(())
}

/// The arguments of the portal's `Spawn`: working directory, argv, file
/// descriptors, environment, flags and options.
#[cfg(target_os = "linux")]
fn spawn_args(cwd: &std::path::Path, exe: &std::path::Path) -> glib::Variant {
    use std::collections::HashMap;
    use std::os::unix::ffi::OsStrExt;

    let nul_terminated = |bytes: &[u8]| [bytes, b"\0"].concat();
    (
        nul_terminated(cwd.as_os_str().as_bytes()),
        vec![nul_terminated(exe.as_os_str().as_bytes())],
        HashMap::<u32, glib::variant::Handle>::new(),
        HashMap::<String, String>::new(),
        0u32,
        HashMap::<String, glib::Variant>::new(),
    )
        .to_variant()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn spawn_args_match_the_portal_signature() {
        let args = spawn_args("/".as_ref(), "/app/bin/swell-player".as_ref());
        assert_eq!(args.type_().as_str(), "(ayaaya{uh}a{ss}ua{sv})");
        assert_eq!(args.child_value(1).child_value(0).data(), b"/app/bin/swell-player\0");
    }
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

/// Signal numbers, the same on Linux, macOS and the BSDs.
#[cfg(unix)]
mod libc_signal {
    pub const SIGINT: i32 = 2;
    pub const SIGTERM: i32 = 15;
}
