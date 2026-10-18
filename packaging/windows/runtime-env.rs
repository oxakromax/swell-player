// Reference for src/bundle.rs (not compiled from here): the environment a
// bundled Windows or macOS run needs. Use from main.rs:
//
//     #![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//     #[cfg(any(windows, target_os = "macos"))]
//     mod bundle;
//     fn main() -> glib::ExitCode {
//         #[cfg(any(windows, target_os = "macos"))]
//         bundle::setup(); // first: before threads, GStreamer and GTK
//         ...
//
// Checked with `cargo clippy -- -D warnings` (with the cfg removed, on Linux).

//! Windows and macOS packages (packaging/windows, packaging/macos) carry
//! GStreamer's plugins, GIO's TLS modules and a CA bundle next to the
//! executable. Points the libraries at them; does nothing when Swell runs
//! from a build tree or a Linux install.

use gtk::gio::{self, prelude::*};
use gtk::glib;
use std::ffi::OsStr;
use std::path::PathBuf;

/// `<root>` of `<root>\bin\swell-player.exe` (Windows) or `Swell.app/Contents`
/// (macOS), when the executable runs from such a bundle.
fn prefix() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let prefix = exe.parent()?.parent()?.to_path_buf();
    let plugins = if cfg!(windows) {
        prefix.join("lib").join("gstreamer-1.0")
    } else {
        prefix.join("PlugIns").join("gstreamer-1.0")
    };
    plugins.is_dir().then_some(prefix)
}

/// Sets `key` unless the user (or the macOS launcher) already did.
fn set_default(key: &str, value: impl AsRef<OsStr>) {
    if std::env::var_os(key).is_none() {
        // SAFETY: `setup` runs first in `main`, before any other thread exists.
        unsafe { std::env::set_var(key, value) };
    }
}

/// Call first in `main`: before any thread starts and before GStreamer or
/// GTK are initialised.
pub fn setup() {
    let Some(prefix) = prefix() else {
        return;
    };
    if cfg!(windows) {
        // gst-plugin-scanner.exe (in libexec\) finds the DLLs in bin\ through PATH.
        let mut path = prefix.join("bin").into_os_string();
        if let Some(old) = std::env::var_os("PATH") {
            path.push(";");
            path.push(old);
        }
        // SAFETY: as in `set_default`.
        unsafe { std::env::set_var("PATH", path) };
        set_default("GST_PLUGIN_SYSTEM_PATH_1_0", prefix.join("lib").join("gstreamer-1.0"));
        set_default(
            "GST_PLUGIN_SCANNER_1_0",
            prefix.join("libexec").join("gstreamer-1.0").join("gst-plugin-scanner.exe"),
        );
        set_default("GIO_MODULE_DIR", prefix.join("lib").join("gio").join("modules"));
    } else {
        // Contents/MacOS/Swell (the launcher script) sets these, plus the dyld
        // and gdk-pixbuf ones; repeated for a direct start of the binary.
        set_default("GST_PLUGIN_SYSTEM_PATH_1_0", prefix.join("PlugIns").join("gstreamer-1.0"));
        set_default("GST_PLUGIN_SCANNER_1_0", prefix.join("MacOS").join("gst-plugin-scanner"));
        set_default("GIO_MODULE_DIR", prefix.join("PlugIns").join("gio-modules"));
        let share = prefix.join("Resources").join("share");
        set_default("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0").join("schemas"));
        set_default("XDG_DATA_DIRS", share);
    }
    // A registry of our own: another GStreamer on the machine would make
    // every start rescan the plugins.
    set_default(
        "GST_REGISTRY_1_0",
        glib::user_cache_dir()
            .join("swell-player")
            .join(format!("gstreamer-registry-{}.bin", std::env::consts::ARCH)),
    );

    // TLS for GStreamer's HTTP (libsoup through glib-networking): the bundled
    // GnuTLS looks for CA certificates at a path on the build machine, so
    // trust the bundled Mozilla CA list instead.
    let ca = if cfg!(windows) {
        prefix.join("etc").join("ssl").join("cert.pem")
    } else {
        prefix.join("Resources").join("etc").join("ssl").join("cert.pem")
    };
    match gio::TlsFileDatabase::new(&ca) {
        Ok(database) => gio::TlsBackend::default().set_default_database(Some(&database)),
        Err(err) => eprintln!("bundled CA certificates not loaded ({}): {err}", ca.display()),
    }
}
