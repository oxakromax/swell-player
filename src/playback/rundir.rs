//! Private files of this process: DASH manifests and notification covers go
//! to `$XDG_RUNTIME_DIR/swell-player/<pid>/` (in memory, owner only), so
//! two instances never prune each other's files. Directories left by
//! processes that are gone are removed at startup.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gtk::glib;

/// Legacy files directly in `swell-player/` (older builds) older than this
/// are removed.
const LEGACY_MAX_AGE: Duration = Duration::from_secs(10 * 60);

fn base() -> PathBuf {
    glib::user_runtime_dir().join("swell-player")
}

/// This process's directory (not created).
pub fn dir() -> PathBuf {
    base().join(std::process::id().to_string())
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
