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
        set_default(
            "GST_PLUGIN_SYSTEM_PATH_1_0",
            prefix.join("lib").join("gstreamer-1.0"),
        );
        set_default(
            "GST_PLUGIN_SCANNER_1_0",
            prefix
                .join("libexec")
                .join("gstreamer-1.0")
                .join("gst-plugin-scanner.exe"),
        );
        set_default(
            "GIO_MODULE_DIR",
            prefix.join("lib").join("gio").join("modules"),
        );
    } else {
        // Contents/MacOS/Swell (the launcher script) sets these, plus the dyld
        // and gdk-pixbuf ones; repeated for a direct start of the binary.
        set_default(
            "GST_PLUGIN_SYSTEM_PATH_1_0",
            prefix.join("PlugIns").join("gstreamer-1.0"),
        );
        set_default(
            "GST_PLUGIN_SCANNER_1_0",
            prefix.join("MacOS").join("gst-plugin-scanner"),
        );
        set_default("GIO_MODULE_DIR", prefix.join("PlugIns").join("gio-modules"));
        let share = prefix.join("Resources").join("share");
        set_default(
            "GSETTINGS_SCHEMA_DIR",
            share.join("glib-2.0").join("schemas"),
        );
        set_default("XDG_DATA_DIRS", share);
        // Started from Finder there is no LANG, so the C library's locale is
        // "C" and libintl ignores LANGUAGE: GTK's and libadwaita's strings
        // would stay English. Take the system's language instead.
        if ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .all(|key| std::env::var_os(key).is_none())
            && let Some(locale) = sys_locale::get_locale().as_deref().and_then(posix_locale)
        {
            set_default("LANG", locale);
        }
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
        prefix
            .join("Resources")
            .join("etc")
            .join("ssl")
            .join("cert.pem")
    };
    match gio::TlsFileDatabase::new(&ca) {
        Ok(database) => gio::TlsBackend::default().set_default_database(Some(&database)),
        Err(err) => eprintln!(
            "bundled CA certificates not loaded ({}): {err}",
            ca.display()
        ),
    }
}

/// `es_CL.UTF-8` for `es-CL` (or `es-Latn-CL`); `None` without a region,
/// since macOS has no `es.UTF-8` locale.
fn posix_locale(tag: &str) -> Option<String> {
    let mut parts = tag.split(['-', '_']);
    let language = parts.next().filter(|language| !language.is_empty())?;
    let region = parts.rfind(|part| part.len() == 2)?;
    Some(format!("{language}_{}.UTF-8", region.to_ascii_uppercase()))
}

/// macOS: GTK and libadwaita look for their translations at Homebrew's
/// path on the build machine; points them at the bundle's copies. Call once
/// they are initialised (from the application's `startup`), since their
/// own initialisation binds the domains again.
#[cfg(target_os = "macos")]
pub fn bind_translations() {
    use std::ffi::{CString, c_char};
    use std::os::unix::ffi::OsStrExt;

    // Homebrew's libintl, which GLib and GTK link to; its `bindtextdomain`
    // is exported under this name.
    #[link(name = "intl")]
    unsafe extern "C" {
        fn libintl_bindtextdomain(domain: *const c_char, dir: *const c_char) -> *mut c_char;
    }
    let Some(prefix) = prefix() else {
        return;
    };
    let locale = prefix.join("Resources").join("share").join("locale");
    let Ok(dir) = CString::new(locale.as_os_str().as_bytes()) else {
        return;
    };
    for domain in [c"gtk40", c"libadwaita"] {
        // Safety: both are NUL-terminated strings that outlive the call,
        // which copies them; this runs on the main thread.
        unsafe { libintl_bindtextdomain(domain.as_ptr(), dir.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn posix_locales() {
        assert_eq!(super::posix_locale("es-CL").as_deref(), Some("es_CL.UTF-8"));
        assert_eq!(
            super::posix_locale("zh-Hans-CN").as_deref(),
            Some("zh_CN.UTF-8")
        );
        assert_eq!(super::posix_locale("en_us").as_deref(), Some("en_US.UTF-8"));
        assert_eq!(super::posix_locale("es"), None);
        assert_eq!(super::posix_locale("es-419"), None);
    }
}
