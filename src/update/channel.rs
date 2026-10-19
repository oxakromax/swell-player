//! How this copy of Swell was installed, which decides what a new release
//! can do: replace it (the Windows installer, the macOS app) or only be
//! announced (Linux packages, Flatpak, the tarball, the portable zip, a
//! development build).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Installed by the Inno Setup installer into this folder (which has
    /// its `unins000.exe`), and whether Swell may update it: whether the
    /// folder is writable. An all-users install (in Program Files) is
    /// not; its installer would ask for an administrator at every quit.
    WindowsInstaller {
        prefix: PathBuf,
        writable: bool,
    },
    /// The portable zip, unpacked anywhere.
    WindowsPortable,
    /// The app bundle (`Swell.app`, or whatever it was renamed to). Whether
    /// Swell may replace it is found only once there is a release to
    /// install ([`app_writable`]).
    MacApp {
        app: PathBuf,
    },
    Flatpak,
    /// Arch Linux's pacman (the AUR packages).
    Pacman,
    Deb,
    /// Another package of the system (`/usr/bin/swell-player`): the .rpm,
    /// or a distribution's own.
    SystemPackage,
    /// The release tarball's `install.sh` or `make install`, into
    /// `/usr/local` or `~/.local`.
    Tarball,
    /// Run from a Cargo target directory.
    Development,
    Unknown,
}

impl Channel {
    /// Whether a new release is downloaded and installed when Swell quits.
    /// A macOS app is, unless [`app_writable`] found otherwise.
    pub fn installs_updates(&self) -> bool {
        match self {
            Self::WindowsInstaller { writable, .. } => *writable,
            Self::MacApp { .. } => APP_WRITABLE.get().copied().unwrap_or(true),
            _ => false,
        }
    }

    /// Whether a new release's package is downloaded: to install it, or
    /// for the user to open (an app Swell may not replace).
    pub fn downloads_updates(&self) -> bool {
        matches!(
            self,
            Self::WindowsInstaller { writable: true, .. } | Self::MacApp { .. }
        )
    }

    /// For logs and the About dialog's debug information.
    pub fn name(&self) -> &'static str {
        match self {
            Self::WindowsInstaller { writable: true, .. } => "Windows installer",
            Self::WindowsInstaller {
                writable: false, ..
            } => "Windows installer (read-only)",
            Self::WindowsPortable => "Windows portable",
            Self::MacApp { .. } if self.installs_updates() => "macOS app",
            Self::MacApp { .. } => "macOS app (read-only)",
            Self::Flatpak => "Flatpak",
            Self::Pacman => "pacman",
            Self::Deb => "deb",
            Self::SystemPackage => "system package",
            Self::Tarball => "tarball",
            Self::Development => "development build",
            Self::Unknown => "unknown",
        }
    }
}

/// The operating system, as far as [`detect`] cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Other,
}

/// What [`detect`] decides on.
#[derive(Clone, Debug)]
pub struct Facts {
    pub os: Os,
    /// The running executable.
    pub exe: PathBuf,
    /// The executable is in a Cargo target directory.
    pub cargo_target: bool,
    /// The Windows or macOS bundle the executable runs from (bundle.rs):
    /// the folder with `bin\` (Windows) or `Swell.app/Contents` (macOS).
    pub bundle: Option<PathBuf>,
    /// Windows: the bundle has the installer's `unins000.exe`.
    pub uninstaller: bool,
    /// Windows: the installer's folder is writable.
    pub writable: bool,
    /// `FLATPAK_ID` is set, or `/.flatpak-info` exists.
    pub flatpak: bool,
    /// `/var/lib/pacman/local/swell-player-*` exists.
    pub pacman: bool,
    /// `/var/lib/dpkg/info/swell-player.list` exists.
    pub dpkg: bool,
    pub home: Option<PathBuf>,
}

/// The channel the facts describe.
pub fn detect(facts: &Facts) -> Channel {
    if facts.cargo_target {
        return Channel::Development;
    }
    match facts.os {
        Os::Windows => match &facts.bundle {
            Some(prefix) if facts.uninstaller => Channel::WindowsInstaller {
                prefix: prefix.clone(),
                writable: facts.writable,
            },
            Some(_) => Channel::WindowsPortable,
            None => Channel::Unknown,
        },
        Os::Mac => match facts.bundle.as_deref().and_then(|c| app_of(c, &facts.exe)) {
            Some(app) => Channel::MacApp { app },
            None => Channel::Unknown,
        },
        Os::Other => {
            let dir = facts.exe.parent();
            if facts.flatpak {
                Channel::Flatpak
            } else if facts.exe == Path::new("/usr/bin/swell-player") {
                if facts.pacman {
                    Channel::Pacman
                } else if facts.dpkg {
                    Channel::Deb
                } else {
                    Channel::SystemPackage
                }
            } else if dir == Some(Path::new("/usr/local/bin"))
                || facts
                    .home
                    .as_ref()
                    .is_some_and(|home| dir == Some(&home.join(".local").join("bin")))
            {
                Channel::Tarball
            } else {
                Channel::Unknown
            }
        }
    }
}

/// `…/Swell.app` for its `Contents` folder, when `exe` is in its
/// `Contents/MacOS`.
fn app_of(contents: &Path, exe: &Path) -> Option<PathBuf> {
    let app = contents.parent()?;
    let bundled = contents.file_name()? == "Contents"
        && app.extension()? == "app"
        && exe.parent() == Some(&contents.join("MacOS"));
    bundled.then(|| app.to_path_buf())
}

/// This install's channel, found once (it looks at a few files).
pub fn current() -> &'static Channel {
    static CURRENT: OnceLock<Channel> = OnceLock::new();
    CURRENT.get_or_init(|| {
        let channel = match std::env::current_exe() {
            Ok(exe) => detect(&gather(exe)),
            Err(err) => {
                log::info!("cannot tell how Swell was installed: {err}");
                Channel::Unknown
            }
        };
        log::debug!("install channel: {}", channel.name());
        channel
    })
}

/// Whether this is a development build, run from a Cargo target directory.
pub fn development_build() -> bool {
    std::env::current_exe().is_ok_and(|exe| in_cargo_target(&exe))
}

/// Whether `exe` is in a Cargo target directory (`target/release/`,
/// `target/<triple>/debug/deps/`…): one that has Cargo's `CACHEDIR.TAG`,
/// or a `target` next to a `Cargo.toml`.
fn in_cargo_target(exe: &Path) -> bool {
    exe.ancestors().skip(1).take(4).any(|dir| {
        dir.join("CACHEDIR.TAG").is_file()
            || (dir.file_name().is_some_and(|name| name == "target")
                && dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file()))
    })
}

/// The facts about the running executable `exe`.
fn gather(exe: PathBuf) -> Facts {
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Other
    };
    #[cfg(any(windows, target_os = "macos"))]
    let bundle = crate::bundle::prefix();
    #[cfg(not(any(windows, target_os = "macos")))]
    let bundle: Option<PathBuf> = None;
    let uninstaller = os == Os::Windows
        && bundle
            .as_ref()
            .is_some_and(|prefix| prefix.join("unins000.exe").is_file());
    let writable = uninstaller && bundle.as_deref().is_some_and(writable);
    // Packages are only asked about for the executable they install.
    let packaged = os == Os::Other && exe == Path::new("/usr/bin/swell-player");
    Facts {
        os,
        cargo_target: in_cargo_target(&exe),
        bundle,
        uninstaller,
        writable,
        flatpak: os == Os::Other
            && (std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists()),
        pacman: packaged && pacman_has_swell(),
        dpkg: packaged && Path::new("/var/lib/dpkg/info/swell-player.list").exists(),
        home: Some(gtk::glib::home_dir()),
        exe,
    }
}

/// Whether pacman's database has a `swell-player` package
/// (`swell-player-<version>`, `swell-player-git-<version>`…).
fn pacman_has_swell() -> bool {
    std::fs::read_dir("/var/lib/pacman/local").is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("swell-player-"))
        })
    })
}

/// Whether the macOS app may be replaced, once [`app_writable`] found out.
static APP_WRITABLE: OnceLock<bool> = OnceLock::new();

/// Whether Swell may replace the macOS app `app`: whether its folder is
/// writable. Asked only once there is a release to install, then
/// remembered: trying to write next to an app in ~/Desktop, ~/Documents or
/// ~/Downloads brings up macOS's privacy prompt.
pub fn app_writable(app: &Path) -> bool {
    *APP_WRITABLE.get_or_init(|| app.parent().is_some_and(writable))
}

/// Whether files can be created in `dir`: tries one (never inside the
/// signed macOS app bundle itself).
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".swell-write-test-{}", std::process::id()));
    match std::fs::File::create_new(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(os: Os, exe: &str) -> Facts {
        Facts {
            os,
            exe: exe.into(),
            cargo_target: false,
            bundle: None,
            uninstaller: false,
            writable: false,
            flatpak: false,
            pacman: false,
            dpkg: false,
            home: Some("/home/ana".into()),
        }
    }

    #[test]
    fn windows() {
        let exe = r"C:\Users\ana\AppData\Local\Programs\Swell\bin\swell-player.exe";
        let prefix = PathBuf::from(r"C:\Users\ana\AppData\Local\Programs\Swell");
        let bundled = Facts {
            bundle: Some(prefix.clone()),
            ..facts(Os::Windows, exe)
        };
        assert_eq!(detect(&bundled), Channel::WindowsPortable);
        let installed = Facts {
            uninstaller: true,
            writable: true,
            ..bundled.clone()
        };
        assert_eq!(
            detect(&installed),
            Channel::WindowsInstaller {
                prefix: prefix.clone(),
                writable: true
            }
        );
        assert!(detect(&installed).installs_updates());
        assert!(!detect(&bundled).downloads_updates());
        // For all users: only announced.
        let all_users = detect(&Facts {
            writable: false,
            ..installed
        });
        assert_eq!(
            all_users,
            Channel::WindowsInstaller {
                prefix,
                writable: false
            }
        );
        assert!(!all_users.installs_updates() && !all_users.downloads_updates());
        assert_eq!(detect(&facts(Os::Windows, exe)), Channel::Unknown);
    }

    #[test]
    fn macos() {
        let exe = "/Applications/Swell.app/Contents/MacOS/swell-player";
        let app = Facts {
            bundle: Some("/Applications/Swell.app/Contents".into()),
            ..facts(Os::Mac, exe)
        };
        let channel = detect(&app);
        assert_eq!(
            channel,
            Channel::MacApp {
                app: "/Applications/Swell.app".into()
            }
        );
        assert!(channel.installs_updates() && channel.downloads_updates());
        // A binary outside an app bundle (a Homebrew build, say).
        assert_eq!(
            detect(&facts(Os::Mac, "/opt/homebrew/bin/swell-player")),
            Channel::Unknown
        );
        let elsewhere = Facts {
            exe: "/Applications/Swell.app/Contents/Resources/swell-player".into(),
            ..app
        };
        assert_eq!(detect(&elsewhere), Channel::Unknown);
    }

    #[test]
    fn linux() {
        let usr = "/usr/bin/swell-player";
        let channel = |facts: Facts| detect(&facts);
        assert_eq!(channel(facts(Os::Other, usr)), Channel::SystemPackage);
        let pacman = Facts {
            pacman: true,
            ..facts(Os::Other, usr)
        };
        assert_eq!(channel(pacman), Channel::Pacman);
        let deb = Facts {
            dpkg: true,
            ..facts(Os::Other, usr)
        };
        assert_eq!(channel(deb), Channel::Deb);
        let flatpak = Facts {
            flatpak: true,
            ..facts(Os::Other, "/app/bin/swell-player")
        };
        assert_eq!(channel(flatpak), Channel::Flatpak);
        for tarball in [
            "/usr/local/bin/swell-player",
            "/home/ana/.local/bin/swell-player",
        ] {
            assert_eq!(channel(facts(Os::Other, tarball)), Channel::Tarball);
        }
        assert_eq!(
            channel(facts(Os::Other, "/home/ana/.cargo/bin/swell-player")),
            Channel::Unknown
        );
        let development = Facts {
            cargo_target: true,
            ..facts(
                Os::Other,
                "/home/ana/swell-player/target/release/swell-player",
            )
        };
        assert_eq!(channel(development), Channel::Development);
        for channel in [
            Channel::Flatpak,
            Channel::Pacman,
            Channel::Deb,
            Channel::SystemPackage,
            Channel::Tarball,
            Channel::Development,
            Channel::Unknown,
        ] {
            assert!(!channel.installs_updates() && !channel.downloads_updates());
        }
    }

    #[test]
    fn cargo_target_directories() {
        // The tests themselves run from target/<profile>/deps.
        assert!(development_build());
        assert!(!in_cargo_target(Path::new("/usr/bin/swell-player")));
    }
}
