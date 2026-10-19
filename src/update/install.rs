//! Replaces this install with a newer release, where Swell can
//! ([`Channel::installs_updates`]). [`prepare`] downloads the release's
//! package into the user cache, checks it against the release's
//! `SHA256SUMS` and keeps it; once it is [`stage`]d, `main` runs it when
//! Swell quits, or at once for a restart ([`install_staged`]):
//!
//! - Windows: the Inno Setup installer, silently, over the same folder;
//!   with `/relaunch=1` (a restart) it starts Swell again
//!   (packaging/windows/swell.iss).
//! - macOS: `install-macos.sh` waits for Swell to exit, copies `Swell.app`
//!   out of the disk image next to the running app, checks its version
//!   and swaps the two.
//!
//! Nothing runs that does not match its checksum, checked again just
//! before it runs.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use gtk::glib;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::channel::Channel;
use super::{Asset, DOWNLOAD_URL, Release, Version};

/// Larger packages are refused (they are a few tens of megabytes).
const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
/// `SHA256SUMS` lists a dozen files.
const MAX_SUMS: u64 = 64 * 1024;
/// A partial download untouched for this long was abandoned.
const ABANDONED: Duration = Duration::from_secs(60 * 60);
/// What every package name starts with (and nothing else in [`dir`]).
const PACKAGE_PREFIX: &str = "swell-player-";

/// Where packages are downloaded: `<user cache>/swell-player/updates/`.
fn dir() -> PathBuf {
    glib::user_cache_dir().join("swell-player").join("updates")
}

/// The name of `version`'s package for `channel` on `arch`
/// (`std::env::consts::ARCH`), as the release workflow names it: the
/// Windows installer, or the macOS disk image of that architecture.
fn package_name(channel: &Channel, version: Version, arch: &str) -> Option<String> {
    let suffix = match (channel, arch) {
        (Channel::WindowsInstaller(_), "x86_64") => "windows-x86_64-setup.exe",
        (Channel::MacApp { .. }, "aarch64") => "macos-arm64.dmg",
        (Channel::MacApp { .. }, "x86_64") => "macos-x86_64.dmg",
        _ => return None,
    };
    Some(format!("{PACKAGE_PREFIX}{version}-{suffix}"))
}

/// `release`'s file `name`, if it is downloaded from that very release of
/// this repository.
fn find_asset<'a>(release: &'a Release, name: &str) -> Option<&'a Asset> {
    let url = format!("{DOWNLOAD_URL}{}/{name}", release.tag);
    release
        .assets
        .iter()
        .find(|asset| asset.name == name && asset.url == url)
}

/// The SHA-256 that `sums`, in `sha256sum`'s format (`<hex>  <name>`, or
/// `<hex> *<name>`, per line), gives `name`. `None` when it lists none,
/// or two different ones.
fn checksum(sums: &str, name: &str) -> Option<[u8; 32]> {
    let mut found = None;
    for line in sums.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (Some(hex), Some(separator), Some(file)) =
            (line.get(..64), line.get(64..66), line.get(66..))
        else {
            continue;
        };
        if file != name || !matches!(separator, "  " | " *") {
            continue;
        }
        let digest = parse_hex(hex)?;
        if found.replace(digest).is_some_and(|other| other != digest) {
            return None;
        }
    }
    found
}

fn parse_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(digest)
}

fn finish(hasher: Sha256) -> [u8; 32] {
    let mut digest = [0; 32];
    digest.copy_from_slice(&hasher.finalize());
    digest
}

/// The SHA-256 of the file at `path` (blocking).
fn file_sha256(path: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 256 * 1024];
    loop {
        match file.read(&mut buffer)? {
            0 => return Ok(finish(hasher)),
            n => hasher.update(&buffer[..n]),
        }
    }
}

/// A downloaded and checked package, waiting to be installed.
#[derive(Clone, Debug)]
pub struct Staged {
    pub version: Version,
    path: PathBuf,
    sha256: [u8; 32],
    /// The install it replaces.
    channel: Channel,
}

/// What a newer release means for this install.
#[derive(Debug)]
pub enum Prepared {
    /// Its package is downloaded and checked: [`stage`] it.
    Staged(Staged),
    /// A macOS app that Swell may not replace: the release's disk image,
    /// downloaded and checked, for the user to open.
    DiskImage(PathBuf),
    /// No package for this install in the release (another system or
    /// architecture, or a release without it): only announce it.
    Announce,
}

/// Downloads `release`'s package for `channel` and checks it (or finds it
/// downloaded already), then removes older downloads.
pub async fn prepare(release: Release, channel: Channel) -> Result<Prepared> {
    let Some(name) = package_name(&channel, release.version, std::env::consts::ARCH) else {
        return Ok(Prepared::Announce);
    };
    let (Some(package), Some(sums)) = (
        find_asset(&release, &name),
        find_asset(&release, "SHA256SUMS"),
    ) else {
        log::info!("release {} has no {name} or no SHA256SUMS", release.tag);
        return Ok(Prepared::Announce);
    };
    let sums = super::read_all(get(&sums.url).await?, MAX_SUMS).await?;
    let sha256 = checksum(&String::from_utf8_lossy(&sums), &name)
        .with_context(|| format!("SHA256SUMS has no checksum for {name}"))?;
    let path = download(package, sha256).await?;
    clean(Some(name)).await;
    Ok(if channel.installs_updates() {
        Prepared::Staged(Staged {
            version: release.version,
            path,
            sha256,
            channel,
        })
    } else {
        Prepared::DiskImage(path)
    })
}

async fn get(url: &str) -> Result<reqwest::Response> {
    Ok(super::client()?.get(url).send().await?.error_for_status()?)
}

/// Downloads `asset` into [`dir`], unless a copy with its checksum is
/// there already (from an earlier run, or another Swell). It is written
/// under a name of this process's and renamed once whole and checked.
async fn download(asset: &Asset, sha256: [u8; 32]) -> Result<PathBuf> {
    if asset.size > MAX_PACKAGE {
        bail!("{} is too large ({} bytes)", asset.name, asset.size);
    }
    let dir = dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(&asset.name);
    let existing = path.clone();
    if tokio::task::spawn_blocking(move || file_sha256(&existing))
        .await?
        .ok()
        == Some(sha256)
    {
        log::debug!("{} was downloaded already", asset.name);
        return Ok(path);
    }
    let part = dir.join(format!("{}.{}.part", asset.name, std::process::id()));
    let result = download_to(asset, &part, sha256).await;
    if result.is_ok() {
        tokio::fs::rename(&part, &path).await?;
    } else {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result.map(|()| path)
}

async fn download_to(asset: &Asset, part: &Path, sha256: [u8; 32]) -> Result<()> {
    let mut response = get(&asset.url).await?;
    if response
        .content_length()
        .is_some_and(|length| length != asset.size)
    {
        bail!("{} does not have the size its release gives", asset.name);
    }
    let mut file = tokio::fs::File::create(part).await?;
    let mut hasher = Sha256::new();
    let mut received = 0;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        if received > asset.size {
            bail!("{} is larger than its release says", asset.name);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    if received != asset.size {
        bail!("{} was cut short", asset.name);
    }
    if finish(hasher) != sha256 {
        log::warn!("{} does not match its checksum in SHA256SUMS", asset.name);
        bail!("{} does not match its checksum", asset.name);
    }
    Ok(())
}

/// Removes earlier downloads from [`dir`], except `keep` and downloads in
/// progress (another Swell's partial file, written to in the last hour).
pub async fn clean(keep: Option<String>) {
    let result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let entries = match std::fs::read_dir(dir()) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            entries => entries?,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(PACKAGE_PREFIX) || Some(name) == keep.as_deref() {
                continue;
            }
            let recent = || {
                entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|time| time.elapsed().is_ok_and(|age| age < ABANDONED))
            };
            if name.ends_with(".part") && recent() {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => log::debug!("removed the old download {name}"),
                Err(err) => log::debug!("could not remove the old download {name}: {err}"),
            }
        }
        Ok(())
    })
    .await;
    if let Ok(Err(err)) = result {
        log::debug!("could not clean the update downloads: {err}");
    }
}

/// The update to install when Swell quits.
static STAGED: Mutex<Option<Staged>> = Mutex::new(None);

fn staged() -> MutexGuard<'static, Option<Staged>> {
    STAGED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Installs `update` when Swell quits (or restarts).
pub fn stage(update: Staged) {
    staged().replace(update);
}

/// Installs nothing when Swell quits (update checks were turned off).
pub fn unstage() {
    staged().take();
}

/// Starts installing the [`stage`]d update, if there is one: called by
/// `main` once the application has quit, so its files can be replaced.
/// With `restart`, Swell is started again afterwards (by the installer or
/// the helper). Returns whether an installation was started.
pub fn install_staged(restart: bool) -> bool {
    let Some(update) = staged().take() else {
        return false;
    };
    // Checked again: the file waited in the cache since it was downloaded.
    match file_sha256(&update.path) {
        Ok(sha256) if sha256 == update.sha256 => {}
        Ok(_) => {
            log::warn!(
                "the download of Swell {} changed; not installing it",
                update.version
            );
            return false;
        }
        Err(err) => {
            log::warn!(
                "cannot read the download of Swell {}: {err}",
                update.version
            );
            return false;
        }
    }
    match run(&update, restart) {
        Ok(()) => {
            log::info!("installing Swell {}", update.version);
            true
        }
        Err(err) => {
            log::warn!("could not install Swell {}: {err:#}", update.version);
            false
        }
    }
}

fn run(update: &Staged, restart: bool) -> Result<()> {
    #[cfg(windows)]
    if let Channel::WindowsInstaller(_) = update.channel {
        return run_installer(&update.path, restart);
    }
    #[cfg(target_os = "macos")]
    if let Channel::MacApp { app, .. } = &update.channel {
        return run_helper(&update.path, app, update.version, restart);
    }
    let _ = restart;
    bail!("a {} install cannot install updates", update.channel.name())
}

/// Runs the Inno Setup installer `setup` with no questions: it shows only
/// its progress, closes a Swell that still runs and never reboots, and
/// starts Swell again only for a restart (`/relaunch=1`, see swell.iss);
/// Inno Setup's own restarting of the applications it closed would also
/// start it after a plain quit. Not a child of this process's console or
/// Ctrl+C group, so it outlives it.
#[cfg(windows)]
fn run_installer(setup: &Path, restart: bool) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let mut log = std::ffi::OsString::from("/LOG=");
    log.push(dir().join("install.log"));
    let mut command = std::process::Command::new(setup);
    command
        .args([
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
            "/NORESTARTAPPLICATIONS",
        ])
        .arg(if restart {
            "/relaunch=1"
        } else {
            "/relaunch=0"
        })
        .arg(log)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::i18n::restore_environment(&mut command);
    command.spawn()?;
    Ok(())
}

/// Starts `install-macos.sh` (written next to the downloads) to replace
/// `app` with the one in the disk image `dmg` once this process has
/// exited. Paths go to it as arguments, never inside its text.
#[cfg(target_os = "macos")]
fn run_helper(dmg: &Path, app: &Path, version: Version, restart: bool) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("install-macos.sh");
    std::fs::write(&script, include_str!("install-macos.sh"))?;
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(dmg)
        .arg(app)
        .arg(version.to_string())
        .arg(if restart { "1" } else { "0" })
        // Its own process group: not stopped along with this one's.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::i18n::restore_environment(&mut command);
    command.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: Version = Version(1, 2, 0);

    fn asset(name: &str, url: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            url: url.to_owned(),
            size: 100,
        }
    }

    #[test]
    fn package_names() {
        let windows = Channel::WindowsInstaller("C:\\Swell".into());
        let mac = Channel::MacApp {
            app: "/Applications/Swell.app".into(),
            writable: true,
        };
        let name = |channel: &Channel, arch: &str| package_name(channel, V, arch);
        assert_eq!(
            name(&windows, "x86_64").as_deref(),
            Some("swell-player-1.2.0-windows-x86_64-setup.exe")
        );
        assert_eq!(name(&windows, "aarch64"), None);
        assert_eq!(
            name(&mac, "aarch64").as_deref(),
            Some("swell-player-1.2.0-macos-arm64.dmg")
        );
        assert_eq!(
            name(&mac, "x86_64").as_deref(),
            Some("swell-player-1.2.0-macos-x86_64.dmg")
        );
        for other in [Channel::WindowsPortable, Channel::Flatpak, Channel::Tarball] {
            assert_eq!(name(&other, "x86_64"), None);
        }
    }

    #[test]
    fn finds_assets_of_the_release_only() {
        let base = "https://github.com/oxakromax/swell-player/releases/download";
        let setup = "swell-player-1.2.0-windows-x86_64-setup.exe";
        let release = Release {
            version: V,
            tag: "v1.2.0".to_owned(),
            page: String::new(),
            assets: vec![
                asset("SHA256SUMS", &format!("{base}/v1.2.0/SHA256SUMS")),
                // Same name, from elsewhere or another release.
                asset(setup, &format!("https://example.com/v1.2.0/{setup}")),
                asset(setup, &format!("{base}/v1.1.0/{setup}")),
                asset(setup, &format!("{base}/v1.2.0/{setup}")),
                asset(
                    "swell-player-1.2.0-macos-arm64.dmg",
                    "http://github.com/oxakromax/swell-player/releases/download/v1.2.0/swell-player-1.2.0-macos-arm64.dmg",
                ),
            ],
        };
        assert_eq!(
            find_asset(&release, setup).map(|a| a.url.as_str()),
            Some(format!("{base}/v1.2.0/{setup}").as_str())
        );
        assert!(find_asset(&release, "SHA256SUMS").is_some());
        assert!(find_asset(&release, "swell-player-1.2.0-macos-arm64.dmg").is_none());
        assert!(find_asset(&release, "swell-player-1.2.0-macos-x86_64.dmg").is_none());
    }

    #[test]
    fn reads_sha256sums() {
        let a = "a".repeat(64);
        let b = "0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789abcdef";
        let sums = format!(
            "{a}  swell-player-1.2.0-linux-x86_64.tar.gz\n\
             {b}  swell-player-1.2.0-windows-x86_64-setup.exe\r\n\
             {a} *swell-player-1.2.0-macos-arm64.dmg\n\
             {b}  SHA256SUMS.sig\n"
        );
        let setup = checksum(&sums, "swell-player-1.2.0-windows-x86_64-setup.exe").unwrap();
        assert_eq!(setup[..4], [0x01, 0x23, 0x45, 0x67]);
        assert_eq!(setup[31], 0xef);
        assert_eq!(
            checksum(&sums, "swell-player-1.2.0-macos-arm64.dmg"),
            Some([0xaa; 32])
        );
        // Names match whole, never as a prefix or suffix.
        assert_eq!(checksum(&sums, "SHA256SUMS"), None);
        assert_eq!(
            checksum(&sums, "swell-player-1.2.0-windows-x86_64-setup"),
            None
        );
        assert_eq!(checksum(&sums, "1.2.0-macos-arm64.dmg"), None);
        // Two different checksums for one name, or a malformed one.
        let twice = format!("{a}  x.dmg\n{b}  x.dmg\n");
        assert_eq!(checksum(&twice, "x.dmg"), None);
        let same = format!("{a}  x.dmg\n{a}  x.dmg\n");
        assert_eq!(checksum(&same, "x.dmg"), Some([0xaa; 32]));
        let bad = format!("{}g  x.dmg\n", "a".repeat(63));
        assert_eq!(checksum(&bad, "x.dmg"), None);
        assert_eq!(checksum(&format!("{a} x.dmg"), "x.dmg"), None);
        assert_eq!(checksum("", "x.dmg"), None);
    }

    #[test]
    fn hashes_files() {
        let path = std::env::temp_dir().join(format!("swell-update-test-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let digest = file_sha256(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            digest.unwrap(),
            parse_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn macos_helper_is_valid_sh() {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/src/update/install-macos.sh");
        let status = std::process::Command::new("sh")
            .arg("-n")
            .arg(script)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
