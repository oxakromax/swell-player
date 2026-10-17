//! Private files of this process: DASH manifests and notification covers go
//! to `$XDG_RUNTIME_DIR/swell-player/<pid>/` (in memory, mode 0700; without
//! a runtime directory, `~/.cache/swell-player-run/<pid>/`), so two
//! instances never prune each other's files. The directory goes away on
//! sign-out and when playback is dropped; ones left by processes that are
//! gone are removed at startup.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gtk::glib;

/// Legacy files directly in `swell-player/` (older builds) older than this
/// are removed.
const LEGACY_MAX_AGE: Duration = Duration::from_secs(10 * 60);

fn base() -> PathBuf {
    base_in(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// `$XDG_RUNTIME_DIR/swell-player`, or (without a usable runtime
/// directory) a private directory in the user cache.
fn base_in(runtime: Option<PathBuf>) -> PathBuf {
    match runtime.filter(|dir| dir.is_absolute()) {
        Some(dir) => dir.join("swell-player"),
        None => glib::user_cache_dir().join("swell-player-run"),
    }
}

/// This process's directory (not created).
pub fn dir() -> PathBuf {
    base().join(std::process::id().to_string())
}

/// Creates this process's directory, and the base one, accessible to the
/// owner only (mode 0700, also when they already existed). Blocking.
pub fn create() -> std::io::Result<PathBuf> {
    let dir = dir();
    create_private(&base(), &dir)?;
    Ok(dir)
}

/// Creates `dir` (inside `base`) with mode 0700 and makes both 0700.
fn create_private(base: &Path, dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    for path in [base, dir] {
        if std::fs::metadata(path)?.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Removes this process's directory and what is in it (blocking).
pub fn remove_own() {
    let dir = dir();
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => log::debug!("removed {}", dir.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log::debug!("could not remove {}: {err}", dir.display()),
    }
}

/// A temporary name next to `path`, unique to this process and call, for
/// writing a file and renaming it into place.
pub fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.{n}.part", std::process::id()))
}

/// What to do with an entry of the base directory.
#[derive(Debug, PartialEq, Eq)]
enum Stale {
    Keep,
    RemoveDir,
    RemoveFile,
}

fn judge(
    name: &str,
    is_dir: bool,
    age: Option<Duration>,
    own: u32,
    alive: impl Fn(u32) -> bool,
) -> Stale {
    if is_dir {
        return match name.parse::<u32>() {
            Ok(pid) if pid != own && !alive(pid) => Stale::RemoveDir,
            _ => Stale::Keep,
        };
    }
    let legacy = name.ends_with(".mpd") || name.ends_with(".part") || name.starts_with("cover-");
    if legacy && age.is_some_and(|age| age > LEGACY_MAX_AGE) {
        Stale::RemoveFile
    } else {
        Stale::Keep
    }
}

/// Removes the directories of processes that no longer run (blocking).
pub fn prune_stale() {
    let Ok(entries) = std::fs::read_dir(base()) else {
        return;
    };
    let own = std::process::id();
    let alive = |pid: u32| Path::new("/proc").join(pid.to_string()).exists();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        let path = entry.path();
        let result = match judge(&name, kind.is_dir(), age, own, alive) {
            Stale::Keep => continue,
            Stale::RemoveDir => std::fs::remove_dir_all(&path),
            Stale::RemoveFile => std::fs::remove_file(&path),
        };
        match result {
            Ok(()) => log::debug!("removed stale {}", path.display()),
            Err(err) => log::debug!("could not remove {}: {err}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_unique() {
        let path = Path::new("/state/queue.json");
        let a = temp_path(path);
        let b = temp_path(path);
        assert_ne!(a, b);
        assert_eq!(a.parent(), path.parent());
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(".queue.json."));
        assert!(name.ends_with(".part"));
        assert!(name.contains(&std::process::id().to_string()));
    }

    #[test]
    fn falls_back_to_the_cache_without_a_runtime_dir() {
        assert_eq!(
            base_in(Some(PathBuf::from("/run/user/1000"))),
            Path::new("/run/user/1000/swell-player")
        );
        let fallback = glib::user_cache_dir().join("swell-player-run");
        assert_eq!(base_in(None), fallback);
        assert_eq!(base_in(Some(PathBuf::new())), fallback);
        assert_eq!(base_in(Some(PathBuf::from("relative"))), fallback);
    }

    #[test]
    fn directories_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let root = std::env::temp_dir().join(format!("swell-rundir-test-{}", std::process::id()));
        let base = root.join("swell-player");
        let dir = base.join("42");
        create_private(&base, &dir).unwrap();
        assert_eq!(mode(&base), 0o700);
        assert_eq!(mode(&dir), 0o700);
        // Left too open by an older build: tightened.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private(&base, &dir).unwrap();
        assert_eq!(mode(&base), 0o700);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn prunes_only_dead_processes() {
        let alive = |pid| pid == 10;
        let old = Some(Duration::from_secs(3600));
        assert_eq!(judge("10", true, None, 5, alive), Stale::Keep);
        assert_eq!(judge("5", true, None, 5, alive), Stale::Keep);
        assert_eq!(judge("11", true, None, 5, alive), Stale::RemoveDir);
        assert_eq!(judge("misc", true, None, 5, alive), Stale::Keep);
        assert_eq!(judge("123.mpd", false, old, 5, alive), Stale::RemoveFile);
        assert_eq!(
            judge("123.mpd", false, Some(Duration::ZERO), 5, alive),
            Stale::Keep
        );
        assert_eq!(judge("notes.txt", false, old, 5, alive), Stale::Keep);
    }
}
