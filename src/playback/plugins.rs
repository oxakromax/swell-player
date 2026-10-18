//! The GStreamer elements playback cannot do without, and what to install
//! when one is missing (shown once at startup instead of failing silently).

use crate::i18n::tr;

/// The plugin set an element comes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Set {
    Base,
    Good,
}

/// `playbin3` plays everything; `dashdemux2` streams Hi-Res (MPEG-DASH).
const NEEDED: [(&str, Set); 2] = [("playbin3", Set::Base), ("dashdemux2", Set::Good)];

/// A message naming the first missing element and its package, if any.
/// Needs `gst::init` to have run.
pub fn missing_message() -> Option<String> {
    let (element, set) = NEEDED
        .into_iter()
        .find(|(element, _)| gst::ElementFactory::find(element).is_none())?;
    log::warn!("GStreamer element {element} is missing");
    Some(
        tr("Playback needs GStreamer’s “{element}”. Install {package}.")
            .replace("{element}", element)
            .replace("{package}", &package(set, &distribution())),
    )
}

/// The Linux distribution's `ID` and `ID_LIKE` (lowercase, in order).
fn distribution() -> Vec<String> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .map(|text| os_release_ids(&text))
        .unwrap_or_default()
}

fn os_release_ids(text: &str) -> Vec<String> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
    };
    let mut ids: Vec<String> = value("ID").into_iter().collect();
    if let Some(like) = value("ID_LIKE") {
        ids.extend(like.split_whitespace().map(str::to_owned));
    }
    ids
}

/// The package with `set` for this system: MSYS2 on Windows, Homebrew on
/// macOS, the distribution's own name on Linux (`ids` from os-release).
fn package(set: Set, ids: &[String]) -> String {
    let name = match set {
        Set::Base => "base",
        Set::Good => "good",
    };
    if cfg!(windows) {
        return format!("mingw-w64-ucrt-x86_64-gst-plugins-{name}");
    }
    if cfg!(target_os = "macos") {
        // Homebrew's formula has every plugin set.
        return "gstreamer (Homebrew)".to_owned();
    }
    for id in ids {
        match id.as_str() {
            "debian" | "ubuntu" => return format!("gstreamer1.0-plugins-{name}"),
            "fedora" | "rhel" | "centos" => return format!("gstreamer1-plugins-{name}"),
            "opensuse" | "suse" => return format!("gstreamer-plugins-{name}"),
            "arch" | "alpine" => return format!("gst-plugins-{name}"),
            _ => {}
        }
    }
    format!("gst-plugins-{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_os_release() {
        let text = "NAME=\"Linux Mint\"\nID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_ID=22\n";
        assert_eq!(os_release_ids(text), ["linuxmint", "ubuntu", "debian"]);
        assert_eq!(os_release_ids("ID=arch\n"), ["arch"]);
        assert!(os_release_ids("").is_empty());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn names_the_distribution_package() {
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            package(Set::Good, &ids(&["linuxmint", "ubuntu"])),
            "gstreamer1.0-plugins-good"
        );
        assert_eq!(
            package(Set::Base, &ids(&["fedora"])),
            "gstreamer1-plugins-base"
        );
        assert_eq!(
            package(
                Set::Good,
                &ids(&["opensuse-tumbleweed", "opensuse", "suse"])
            ),
            "gstreamer-plugins-good"
        );
        assert_eq!(package(Set::Good, &[]), "gst-plugins-good");
    }
}
