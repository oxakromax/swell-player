//! The About dialog (docs/design.md §3.16).

use adw::prelude::*;
use gtk::glib;

use crate::config;
use crate::i18n::tr;

pub fn present(window: &adw::ApplicationWindow) {
    let dialog = adw::AboutDialog::builder()
        .application_icon(config::APP_ID)
        .application_name(config::APP_NAME)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name(tr("Swell contributors"))
        .license_type(gtk::License::Gpl30)
        .comments(tr(
            "A native TIDAL client for Linux.\n\nNot affiliated with TIDAL.",
        ))
        .debug_info(debug_info())
        .debug_info_filename("swell-debug-info.txt")
        .build();
    dialog.present(Some(window));
}

/// Versions and paths for bug reports.
fn debug_info() -> String {
    let cache = glib::user_cache_dir().join("swell-player");
    let config = glib::user_config_dir().join("swell-player");
    format!(
        "Swell {} ({})\nGTK {}.{}.{}\nlibadwaita {}.{}.{}\nGStreamer {}\nCache: {}\nSettings: {}\n",
        env!("CARGO_PKG_VERSION"),
        crate::update::channel::current().name(),
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
        gst::version_string(),
        cache.display(),
        config.display(),
    )
}
