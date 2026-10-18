//! The disk tier of prefetched audio: the first seconds of tracks (a
//! lossless file's start; a DASH stream's init segment and first media
//! segments), kept in `<user cache>/swell-player/audio/` so they survive
//! restarts and can be many without using memory. The local proxy
//! (`proxy.rs`) serves a track's start from here (the OS's page cache,
//! mostly) instead of the CDN, and continues from TIDAL where it ends.
//!
//! - **Only media bytes.** An entry is keyed by the track, its format and
//!   the identity of the media file (a hash of its URL's path, or of its
//!   DASH manifest without the URLs), never by a signed URL: those change
//!   every time a stream is resolved, the bytes do not. No URL or token is
//!   ever written here.
//! - **One file per entry**, named by a hash of its key, with a header
//!   holding the key, the payload's length and a checksum: a file that is
//!   short, altered or holds another key is deleted when read. Files are
//!   written under a temporary name and renamed into place, so a crash
//!   never leaves a partial entry under its name (no fsync: losing an entry
//!   only costs a download); temporary files left by a crash are deleted
//!   at startup. There is no index on disk to get out of step: it is
//!   rebuilt from the directory (sizes, and modification times as the last
//!   use).
//! - **Budget.** At most the configured cap (`audio_cache_mb` in the
//!   settings, [`DEFAULT_CAP_MB`] by default), and never so much that the
//!   disk runs low on space (see [`allowed`]). The least recently used
//!   entries go first. Room is reserved before a file is written, so
//!   concurrent writers never exceed it either.
//! - Everything here is blocking file I/O: call it from `spawn_blocking`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;
use sha2::{Digest, Sha256};

/// The cap when the settings say nothing else, in MB.
pub const DEFAULT_CAP_MB: u32 = 512;
/// What the disk keeps free: a twentieth of it (5 %), but no more than
/// 2 GiB. These are GNOME's own thresholds for warning about low disk
/// space (gsd-housekeeping: below 5 % free, unless more than 2 GB are),
/// so the cache never pushes a disk into that state, and gives its space
/// back as the disk fills up. Small disks keep a share, large ones do not
/// have tens of gigabytes held back for a cache of a few hundred MB.
const RESERVE_SHARE: u64 = 20;
const MAX_RESERVE: u64 = 2 << 30;

const MAGIC: &[u8; 4] = b"SWAC";
const VERSION: u32 = 1;
/// Magic, version, payload length, total, checksum, key and content-type
/// lengths.
const HEADER: usize = 4 + 4 + 8 + 8 + 8 + 2 + 2;
const EXTENSION: &str = "swa";
const TEMP_EXTENSION: &str = "part";
/// Temporary files older than this are a crash's leftovers.
const STALE_TEMP: Duration = Duration::from_secs(10 * 60);
/// A file's modification time (its last use, for the next session's LRU)
/// is brought up to date at most this often.
const TOUCH_EVERY: Duration = Duration::from_secs(10 * 60);

/// The default directory.
pub fn default_dir() -> PathBuf {
    glib::user_cache_dir().join("swell-player").join("audio")
}

/// Space on the disk holding the cache, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub free: u64,
    pub total: u64,
}

/// How many bytes the cache may hold: `cap`, but no more than what keeps
/// the disk's reserve (see [`RESERVE_SHARE`]) free, counting the `used`
/// bytes the cache holds already as its own. When the disk is fuller than
/// that, the answer is below `used`: the cache shrinks. Unknown space:
/// the cap.
pub fn allowed(cap: u64, used: u64, space: Option<Space>) -> u64 {
    let Some(space) = space else {
        return cap;
    };
    let reserve = (space.total / RESERVE_SHARE).min(MAX_RESERVE);
    cap.min(used.saturating_add(space.free).saturating_sub(reserve))
}

/// The disk's space (GIO: the same on every platform); `None` when it
/// cannot say.
fn space(dir: &Path) -> Option<Space> {
    let info = gio::File::for_path(dir)
        .query_filesystem_info("filesystem::free,filesystem::size", gio::Cancellable::NONE)
        .ok()?;
    let known = |name| {
        info.has_attribute(name)
            .then(|| info.attribute_uint64(name))
    };
    Some(Space {
        free: known("filesystem::free")?,
        total: known("filesystem::size").filter(|total| *total > 0)?,
    })
}

/// A cheap checksum, against files cut short or altered on disk (not
/// against tampering): four lanes of xor, multiply and rotate over 64-bit
/// words, each step a bijection, so changing any one word always changes
/// the result. Several GB/s, against a few hundred MB/s for SHA-256
/// without hardware support.
pub fn checksum(data: &[u8]) -> u64 {
    const K: [u64; 4] = [
        0x9E37_79B9_7F4A_7C15,
        0xC2B2_AE3D_27D4_EB4F,
        0x1656_67B1_9E37_79F9,
        0x27D4_EB2F_1656_67C5,
    ];
    let mut lanes = K;
    let (chunks, remainder) = data.as_chunks::<32>();
    for chunk in chunks {
        for (i, lane) in lanes.iter_mut().enumerate() {
            let mut word = [0u8; 8];
            word.copy_from_slice(&chunk[i * 8..i * 8 + 8]);
            *lane = (*lane ^ u64::from_le_bytes(word))
                .wrapping_mul(K[i])
                .rotate_left(31);
        }
    }
    let mut tail = 0xCBF2_9CE4_8422_2325u64;
    for &byte in remainder {
        tail = (tail ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3);
    }
    let mut hash = (data.len() as u64).wrapping_mul(K[0]) ^ tail;
    for lane in lanes {
        hash = (hash ^ lane).wrapping_mul(K[1]).rotate_left(27);
    }
    hash ^ (hash >> 29)
}

/// What an entry is, as text: also stored in its header, so a file is
/// only ever read back for its own key. Holds no URL or token.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key(String);

impl Key {
    /// `file` (`""` for a lossless file's start; `i/<name>`, `m/<name>`
    /// for a DASH stream's segments) of `track_id` in the media
    /// `identity` (its format and the hash of what identifies the file).
    pub fn new(track_id: u64, identity: &str, file: &str) -> Self {
        Self(format!("track={track_id} media={identity} file={file}"))
    }

    /// The name of its file: a hash of the key.
    pub fn file_name(&self) -> String {
        let digest = Sha256::digest(self.0.as_bytes());
        let mut name = String::with_capacity(40);
        for byte in &digest[..16] {
            name.push_str(&format!("{byte:02x}"));
        }
        name.push('.');
        name.push_str(EXTENSION);
        name
    }
}

/// The hex of the first 64 bits of SHA-256 of `text` (identities of media
/// files, from what never changes about them).
pub fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// An entry read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub bytes: Bytes,
    /// The size of the whole file the bytes start (lossless files), or
    /// their own length.
    pub total: u64,
    pub content_type: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct Meta {
    /// Bytes of its file, header included.
    size: u64,
    used_at: SystemTime,
    /// When its file's modification time was last set.
    touched_at: SystemTime,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Meta>,
    /// Bytes of the entries' files.
    used: u64,
    /// Bytes reserved for files being written.
    reserved: u64,
    /// Bumped by [`DiskCache::clear`]: writes that began before it are
    /// dropped.
    epoch: u64,
}

/// Why an entry could not be read.
enum ReadError {
    Missing,
    Corrupt(&'static str),
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => ReadError::Missing,
            io::ErrorKind::UnexpectedEof => ReadError::Corrupt("cut short"),
            _ => ReadError::Io(err),
        }
    }
}

pub struct DiskCache {
    dir: PathBuf,
    /// Bytes; 0 turns the disk tier off.
    cap: AtomicU64,
    state: Mutex<State>,
}

impl DiskCache {
    /// Opens (creating it) the cache in `dir` with a cap of `cap` bytes:
    /// reads the directory, deletes what crashes left behind, and evicts
    /// what is over the budget.
    pub fn open(dir: PathBuf, cap: u64) -> io::Result<Self> {
        create_private(&dir)?;
        let cache = Self {
            dir,
            cap: AtomicU64::new(cap),
            state: Mutex::default(),
        };
        let (entries, used) = scan(&cache.dir)?;
        {
            let mut state = cache.lock();
            state.entries = entries;
            state.used = used;
        }
        cache.prune();
        let state = cache.lock();
        log::debug!(
            "audio cache: {} entries, {} KiB of {} MiB in {}",
            state.entries.len(),
            state.used >> 10,
            cache.cap() >> 20,
            cache.dir.display()
        );
        drop(state);
        Ok(cache)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn cap(&self) -> u64 {
        self.cap.load(Ordering::Relaxed)
    }

    /// Changes the cap, evicting what no longer fits.
    pub fn set_cap(&self, cap: u64) {
        if self.cap.swap(cap, Ordering::Relaxed) > cap {
            self.prune();
        }
    }

    /// Bytes held (files, headers included).
    pub fn used(&self) -> u64 {
        self.lock().used
    }

    /// Whether an entry for `key` is there, as far as the index knows (no
    /// I/O).
    pub fn contains(&self, key: &Key) -> bool {
        self.lock().entries.contains_key(&key.file_name())
    }

    /// About the payload's size of the entry for `key`, from the index: its
    /// file's size less the header (the content type, a few bytes, is
    /// counted in).
    pub fn size_of(&self, key: &Key) -> usize {
        self.lock().entries.get(&key.file_name()).map_or(0, |meta| {
            usize::try_from(meta.size)
                .unwrap_or(usize::MAX)
                .saturating_sub(HEADER + key.0.len())
        })
    }

    /// Reads the entry for `key`; a file that is not a valid entry for it
    /// is deleted.
    pub fn get(&self, key: &Key) -> Option<Blob> {
        let name = key.file_name();
        let path = self.dir.join(&name);
        match read_entry(&path, key) {
            Ok(blob) => {
                self.touch(&name, &path);
                Some(blob)
            }
            Err(ReadError::Missing) => {
                self.forget(&name);
                None
            }
            Err(ReadError::Corrupt(why)) => {
                log::debug!("audio cache: deleting {name} ({why})");
                let mut state = self.lock();
                if let Err(err) = fs::remove_file(&path)
                    && err.kind() != io::ErrorKind::NotFound
                {
                    log::debug!("audio cache: could not delete {name}: {err}");
                }
                if let Some(meta) = state.entries.remove(&name) {
                    state.used = state.used.saturating_sub(meta.size);
                }
                None
            }
            Err(ReadError::Io(err)) => {
                log::debug!("audio cache: could not read {name}: {err}");
                None
            }
        }
    }

    /// Stores `payload` for `key` (`total`: the size of the whole file it
    /// starts). False when it does not fit in the budget, or the cache was
    /// cleared meanwhile.
    pub fn put(
        &self,
        key: &Key,
        payload: &[u8],
        total: u64,
        content_type: Option<&str>,
    ) -> io::Result<bool> {
        let name = key.file_name();
        let content_type = content_type.unwrap_or("");
        let (Ok(key_len), Ok(type_len)) = (
            u16::try_from(key.0.len()),
            u16::try_from(content_type.len()),
        ) else {
            return Ok(false);
        };
        let mut header = Vec::with_capacity(HEADER + key.0.len() + content_type.len());
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        header.extend_from_slice(&total.to_le_bytes());
        header.extend_from_slice(&checksum(payload).to_le_bytes());
        header.extend_from_slice(&key_len.to_le_bytes());
        header.extend_from_slice(&type_len.to_le_bytes());
        header.extend_from_slice(key.0.as_bytes());
        header.extend_from_slice(content_type.as_bytes());
        let size = (header.len() + payload.len()) as u64;

        let space = space(&self.dir);
        let epoch = {
            let mut state = self.lock();
            if !self.make_room(&mut state, &name, size, space) {
                return Ok(false);
            }
            state.reserved += size;
            state.epoch
        };
        let written = self.write_temp(&name, &header, payload, epoch);
        let mut state = self.lock();
        state.reserved = state.reserved.saturating_sub(size);
        let temp = written?;
        if state.epoch != epoch {
            let _ = fs::remove_file(&temp);
            return Ok(false);
        }
        // Renamed under the lock, so the index always matches the
        // directory, also with writers of the same key racing.
        if let Err(err) = fs::rename(&temp, self.dir.join(&name)) {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
        let now = SystemTime::now();
        let meta = Meta {
            size,
            used_at: now,
            touched_at: now,
        };
        if let Some(old) = state.entries.insert(name, meta) {
            state.used = state.used.saturating_sub(old.size);
        }
        state.used += size;
        Ok(true)
    }

    /// Deletes the entry for `key`.
    pub fn remove(&self, key: &Key) {
        let name = key.file_name();
        let mut state = self.lock();
        let _ = fs::remove_file(self.dir.join(&name));
        if let Some(meta) = state.entries.remove(&name) {
            state.used = state.used.saturating_sub(meta.size);
        }
    }

    /// Deletes everything, the directory included (sign-out, "Clear"):
    /// writes under way are dropped, later ones create it again.
    pub fn clear(&self) -> io::Result<()> {
        let mut state = self.lock();
        state.epoch += 1;
        state.entries.clear();
        state.used = 0;
        // Under the lock: nothing is renamed into place meanwhile.
        match fs::remove_dir_all(&self.dir) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// Evicts until what is held fits the budget now.
    pub fn prune(&self) {
        let space = space(&self.dir);
        let mut state = self.lock();
        self.make_room(&mut state, "", 0, space);
    }

    /// Evicts the least recently used entries (never `name`) until `size`
    /// more bytes fit in the budget, counting the entry for `name` as
    /// replaced. False when they cannot fit.
    fn make_room(&self, state: &mut State, name: &str, size: u64, space: Option<Space>) -> bool {
        let replaced = state.entries.get(name).map_or(0, |meta| meta.size);
        let allowed = allowed(self.cap(), state.used, space);
        let held = |state: &State| state.used + state.reserved - replaced.min(state.used);
        if size > allowed {
            return false;
        }
        if held(state) + size <= allowed {
            return true;
        }
        let mut order: Vec<(SystemTime, String)> = state
            .entries
            .iter()
            .filter(|(entry, _)| entry.as_str() != name)
            .map(|(entry, meta)| (meta.used_at, entry.clone()))
            .collect();
        order.sort();
        for (_, victim) in order {
            if held(state) + size <= allowed {
                break;
            }
            match fs::remove_file(self.dir.join(&victim)) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => {
                    log::debug!("audio cache: could not evict {victim}: {err}");
                    continue;
                }
                _ => {}
            }
            if let Some(meta) = state.entries.remove(&victim) {
                state.used = state.used.saturating_sub(meta.size);
            }
        }
        held(state) + size <= allowed
    }

    /// Writes an entry's file under a temporary name; returns that name.
    fn write_temp(
        &self,
        name: &str,
        header: &[u8],
        payload: &[u8],
        epoch: u64,
    ) -> io::Result<PathBuf> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = self.dir.join(format!(
            "{name}.{}.{n}.{TEMP_EXTENSION}",
            std::process::id()
        ));
        let mut file = match File::create(&temp) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.lost_dir(epoch)?;
                File::create(&temp)?
            }
            other => other?,
        };
        let written = file
            .write_all(header)
            .and_then(|()| file.write_all(payload));
        drop(file);
        if let Err(err) = written {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
        Ok(temp)
    }

    /// The directory went away (deleted while running): creates it again
    /// and forgets the entries that were in it, unless the cache was
    /// cleared since `epoch` (then it stays gone).
    fn lost_dir(&self, epoch: u64) -> io::Result<()> {
        let mut state = self.lock();
        if state.epoch != epoch {
            return Err(io::Error::new(io::ErrorKind::NotFound, "cleared"));
        }
        if !self.dir.is_dir() {
            log::debug!("audio cache: the directory is gone, starting afresh");
            state.entries.clear();
            state.used = 0;
            create_private(&self.dir)?;
        }
        Ok(())
    }

    fn forget(&self, name: &str) {
        let mut state = self.lock();
        if let Some(meta) = state.entries.remove(name) {
            state.used = state.used.saturating_sub(meta.size);
        }
    }

    /// Marks an entry used now (and, now and then, its file).
    fn touch(&self, name: &str, path: &Path) {
        let now = SystemTime::now();
        let stale = {
            let mut state = self.lock();
            let Some(meta) = state.entries.get_mut(name) else {
                // Read but not indexed (the directory was rebuilt by hand):
                // counted from now on.
                drop(state);
                self.adopt(name, path);
                return;
            };
            meta.used_at = now;
            let stale = now
                .duration_since(meta.touched_at)
                .is_ok_and(|age| age >= TOUCH_EVERY);
            if stale {
                meta.touched_at = now;
            }
            stale
        };
        if stale {
            let _ = File::options()
                .write(true)
                .open(path)
                .and_then(|file| file.set_modified(now));
        }
    }

    fn adopt(&self, name: &str, path: &Path) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let now = SystemTime::now();
        let mut state = self.lock();
        if state.entries.contains_key(name) {
            return;
        }
        state.entries.insert(
            name.to_owned(),
            Meta {
                size: metadata.len(),
                used_at: now,
                touched_at: metadata.modified().unwrap_or(now),
            },
        );
        state.used += metadata.len();
    }
}

/// The entries in `dir` (by file name) and their bytes; deletes temporary
/// files that a crash left behind.
fn scan(dir: &Path) -> io::Result<(HashMap<String, Meta>, u64)> {
    let mut entries = HashMap::new();
    let mut used = 0;
    let now = SystemTime::now();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        let modified = metadata.modified().unwrap_or(now);
        if name.ends_with(&format!(".{TEMP_EXTENSION}")) {
            let old = now
                .duration_since(modified)
                .is_ok_and(|age| age >= STALE_TEMP);
            if old {
                let _ = fs::remove_file(item.path());
            }
            continue;
        }
        if !is_entry_name(&name) {
            continue;
        }
        used += metadata.len();
        entries.insert(
            name,
            Meta {
                size: metadata.len(),
                used_at: modified,
                touched_at: modified,
            },
        );
    }
    Ok((entries, used))
}

/// `<32 hex digits>.swa`.
fn is_entry_name(name: &str) -> bool {
    name.strip_suffix(EXTENSION)
        .and_then(|stem| stem.strip_suffix('.'))
        .is_some_and(|stem| stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn read_entry(path: &Path, key: &Key) -> Result<Blob, ReadError> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut header = [0u8; HEADER];
    file.read_exact(&mut header)?;
    let u64_at = |at: usize| {
        let mut word = [0u8; 8];
        word.copy_from_slice(&header[at..at + 8]);
        u64::from_le_bytes(word)
    };
    let u16_at = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
    if &header[..4] != MAGIC || header[4..8] != VERSION.to_le_bytes() {
        return Err(ReadError::Corrupt("not an entry of this version"));
    }
    let payload_len = u64_at(8);
    let total = u64_at(16);
    let sum = u64_at(24);
    let (key_len, type_len) = (usize::from(u16_at(32)), usize::from(u16_at(34)));
    let expected = (HEADER + key_len + type_len) as u64 + payload_len;
    if len != expected || payload_len > 1 << 30 {
        return Err(ReadError::Corrupt("wrong length"));
    }
    let mut names = vec![0u8; key_len + type_len];
    file.read_exact(&mut names)?;
    if &names[..key_len] != key.0.as_bytes() {
        return Err(ReadError::Corrupt("another key"));
    }
    let content_type = std::str::from_utf8(&names[key_len..])
        .map_err(|_| ReadError::Corrupt("bad content type"))?
        .to_owned();
    let mut payload = vec![0u8; payload_len as usize];
    file.read_exact(&mut payload)?;
    if checksum(&payload) != sum {
        return Err(ReadError::Corrupt("checksum mismatch"));
    }
    Ok(Blob {
        bytes: Bytes::from(payload),
        total,
        content_type: (!content_type.is_empty()).then_some(content_type),
    })
}

/// Creates `dir`, accessible to the owner only on Unix (what was listened
/// to is private). Windows: the user's profile is private already.
fn create_private(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        if fs::metadata(dir)?.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Deletes the cache in `dir` when no [`DiskCache`] has it open.
pub fn remove_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A fresh directory for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("swell-audio-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn payload(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31) ^ seed).collect()
    }

    #[test]
    fn keys_name_files_by_track_format_and_file() {
        let a = Key::new(42, "LOSSLESS/STEREO/16/44100/flac|p=0123", "");
        assert_eq!(a, Key::new(42, "LOSSLESS/STEREO/16/44100/flac|p=0123", ""));
        let name = a.file_name();
        assert!(is_entry_name(&name), "{name}");
        // Every part matters.
        for other in [
            Key::new(43, "LOSSLESS/STEREO/16/44100/flac|p=0123", ""),
            Key::new(42, "HI_RES_LOSSLESS/STEREO/24/96000/flac|p=0123", ""),
            Key::new(42, "LOSSLESS/STEREO/16/44100/flac|p=0124", ""),
            Key::new(42, "LOSSLESS/STEREO/16/44100/flac|p=0123", "m/1.mp4"),
        ] {
            assert_ne!(other.file_name(), name, "{other:?}");
        }
        assert_ne!(
            Key::new(1, "x", "m/1.mp4").file_name(),
            Key::new(1, "x", "m/2.mp4").file_name()
        );
        assert_eq!(short_hash("/mediatracks/abc/0.flac").len(), 16);
        assert_eq!(short_hash("a"), short_hash("a"));
        assert_ne!(short_hash("a"), short_hash("b"));
        assert!(!is_entry_name("0123.swa"));
        assert!(!is_entry_name(&format!("{name}.1.2.part")));
    }

    #[test]
    fn checksum_sees_any_change() {
        let data = payload(10_007, 3);
        let sum = checksum(&data);
        assert_eq!(sum, checksum(&data.clone()));
        for at in [0, 7, 8, 31, 32, 5000, 9_999, 10_006] {
            let mut changed = data.clone();
            changed[at] ^= 1;
            assert_ne!(checksum(&changed), sum, "byte {at}");
        }
        assert_ne!(checksum(&data[..10_000]), sum);
        assert_ne!(checksum(&[]), checksum(&[0]));
        assert_ne!(checksum(&[0; 64]), checksum(&[0; 65]));
    }

    #[test]
    fn free_space_policy() {
        const GIB: u64 = 1 << 30;
        let space = |free, total| Some(Space { free, total });
        // Plenty of space: the cap.
        assert_eq!(
            allowed(512 << 20, 0, space(500 * GIB, 1000 * GIB)),
            512 << 20
        );
        assert_eq!(allowed(512 << 20, 0, None), 512 << 20);
        // A big disk keeps 2 GiB free, not 5 % of it.
        assert_eq!(allowed(u64::MAX, 0, space(3 * GIB, 1000 * GIB)), GIB);
        // A small one keeps 5 %: 1 GiB of 20.
        assert_eq!(allowed(u64::MAX, 0, space(GIB + 100, 20 * GIB)), 100);
        // What the cache holds counts as its own.
        assert_eq!(allowed(u64::MAX, 300, space(2 * GIB, 1000 * GIB)), 300);
        // Fuller than the reserve: the cache gives space back.
        assert_eq!(
            allowed(u64::MAX, 500, space(2 * GIB - 200, 1000 * GIB)),
            300
        );
        assert_eq!(allowed(u64::MAX, 100, space(GIB, 1000 * GIB)), 0);
        assert_eq!(allowed(512, 0, space(0, 0)), 0);
    }

    #[test]
    fn stores_and_survives_a_restart() {
        let dir = temp_dir("restart");
        let key = Key::new(7, "LOSSLESS|p=1", "");
        let data = payload(300_000, 1);
        {
            let cache = DiskCache::open(dir.clone(), 10 << 20).unwrap();
            assert!(cache.get(&key).is_none());
            assert!(
                cache
                    .put(&key, &data, 9_000_000, Some("audio/flac"))
                    .unwrap()
            );
            assert!(cache.contains(&key));
            assert_eq!(cache.size_of(&key), data.len() + "audio/flac".len());
        }
        // Dropped and opened again: still there, byte for byte.
        let cache = DiskCache::open(dir.clone(), 10 << 20).unwrap();
        assert!(cache.contains(&key));
        let blob = cache.get(&key).unwrap();
        assert!(blob.bytes == data);
        assert_eq!(blob.total, 9_000_000);
        assert_eq!(blob.content_type.as_deref(), Some("audio/flac"));
        assert_eq!(
            cache.used(),
            fs::metadata(dir.join(key.file_name())).unwrap().len()
        );
        // No URL or token in the files: only what the key says.
        let file = fs::read(dir.join(key.file_name())).unwrap();
        assert!(file.windows(4).all(|w| w != b"http"));
        cache.clear().unwrap();
        assert!(!dir.exists());
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.used(), 0);
        // Usable again after clearing.
        assert!(cache.put(&key, &data, 0, None).unwrap());
        assert!(cache.get(&key).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_and_partial_files_are_deleted() {
        let dir = temp_dir("corrupt");
        let cache = DiskCache::open(dir.clone(), 10 << 20).unwrap();
        let key = Key::new(1, "x", "m/1.mp4");
        let data = payload(50_000, 2);
        let path = dir.join(key.file_name());
        type Damage = fn(&mut Vec<u8>);
        let damage: [(&str, Damage); 5] = [
            ("cut short", |f| f.truncate(f.len() - 10)),
            ("a flipped bit", |f| {
                let at = f.len() - 100;
                f[at] ^= 0x10;
            }),
            ("grown", |f| f.extend_from_slice(b"junk")),
            ("another magic", |f| f[0] = b'X'),
            ("empty", |f| f.clear()),
        ];
        for (what, change) in damage {
            assert!(cache.put(&key, &data, 0, None).unwrap());
            let mut file = fs::read(&path).unwrap();
            change(&mut file);
            fs::write(&path, &file).unwrap();
            assert!(cache.get(&key).is_none(), "{what}");
            assert!(!path.exists(), "{what}: deleted");
            assert!(!cache.contains(&key));
            assert_eq!(cache.used(), 0, "{what}");
        }
        // Another key's file under this key's name.
        let other = Key::new(2, "x", "m/1.mp4");
        assert!(cache.put(&other, &data, 0, None).unwrap());
        fs::rename(dir.join(other.file_name()), &path).unwrap();
        assert!(cache.get(&key).is_none());
        assert!(!path.exists());
        // A crash's leftovers: an old temporary file goes at startup, a
        // recent one (a write under way elsewhere) stays; strangers stay.
        let old = dir.join(format!("{}.1.1.part", key.file_name()));
        let recent = dir.join(format!("{}.1.2.part", key.file_name()));
        fs::write(&old, b"partial").unwrap();
        fs::write(&recent, b"partial").unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - 2 * STALE_TEMP)
            .unwrap();
        fs::write(dir.join("notes.txt"), b"x").unwrap();
        drop(cache);
        let cache = DiskCache::open(dir.clone(), 10 << 20).unwrap();
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(dir.join("notes.txt").exists());
        assert_eq!(cache.used(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evicts_least_recently_used_within_the_cap() {
        let dir = temp_dir("lru");
        let entry = 100_000;
        // Room for three entries and their headers.
        let cap = 3 * (entry as u64 + 200);
        let cache = DiskCache::open(dir.clone(), cap).unwrap();
        let keys: Vec<Key> = (0..5).map(|n| Key::new(n, "x", "")).collect();
        let on_disk = || -> u64 {
            fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().metadata().unwrap().len())
                .sum()
        };
        for key in &keys[..3] {
            assert!(cache.put(key, &payload(entry, 0), 0, None).unwrap());
            std::thread::sleep(Duration::from_millis(5));
        }
        // Using the first makes the second the least recently used.
        assert!(cache.get(&keys[0]).is_some());
        assert!(cache.put(&keys[3], &payload(entry, 0), 0, None).unwrap());
        assert!(cache.contains(&keys[0]));
        assert!(!cache.contains(&keys[1]));
        assert!(!dir.join(keys[1].file_name()).exists());
        assert!(cache.contains(&keys[2]) && cache.contains(&keys[3]));
        assert!(cache.used() <= cap && on_disk() <= cap);
        assert_eq!(cache.used(), on_disk());
        // Bigger than the whole cap: refused, nothing evicted.
        assert!(
            !cache
                .put(&keys[4], &payload(cap as usize, 0), 0, None)
                .unwrap()
        );
        assert!(cache.contains(&keys[0]));
        // A lower cap evicts at once.
        cache.set_cap(entry as u64 + 200);
        assert!(cache.used() <= entry as u64 + 200);
        assert_eq!(cache.used(), on_disk());
        assert!(cache.contains(&keys[3]), "the most recent stays");
        // Zero: nothing is kept.
        cache.set_cap(0);
        assert_eq!(cache.used(), 0);
        assert!(!cache.put(&keys[0], &payload(10, 0), 0, None).unwrap());
        // Reopening with a lower cap prunes at startup.
        cache.set_cap(cap);
        for key in &keys[..3] {
            assert!(cache.put(key, &payload(entry, 0), 0, None).unwrap());
        }
        drop(cache);
        let cache = DiskCache::open(dir.clone(), entry as u64 + 200).unwrap();
        assert!(cache.used() <= entry as u64 + 200);
        assert_eq!(cache.used(), on_disk());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writers_stay_consistent() {
        let dir = temp_dir("concurrent");
        let cap = 2_000_000;
        let cache = Arc::new(DiskCache::open(dir.clone(), cap).unwrap());
        let same = Key::new(1, "x", "");
        let threads: Vec<_> = (0..8u8)
            .map(|t| {
                let cache = cache.clone();
                let same = same.clone();
                std::thread::spawn(move || {
                    for round in 0..20u64 {
                        // Everyone writes the same key, with the same bytes…
                        cache.put(&same, &payload(150_000, 9), 1, None).unwrap();
                        // …and keys of their own, more than fit.
                        let own = Key::new(100 + u64::from(t) * 100 + round, "x", "");
                        cache.put(&own, &payload(120_000, t), 0, None).unwrap();
                        assert!(cache.used() <= cap);
                        if let Some(blob) = cache.get(&same) {
                            assert!(blob.bytes == payload(150_000, 9));
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let on_disk: u64 = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert_eq!(cache.used(), on_disk, "the index matches the directory");
        assert!(on_disk <= cap);
        // No temporary file left over.
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .all(|e| is_entry_name(&e.unwrap().file_name().to_string_lossy()))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn survives_its_directory_being_deleted() {
        let dir = temp_dir("deleted");
        let cache = DiskCache::open(dir.clone(), 10 << 20).unwrap();
        let key = Key::new(5, "x", "");
        assert!(cache.put(&key, &payload(1000, 0), 0, None).unwrap());
        fs::remove_dir_all(&dir).unwrap();
        assert!(cache.get(&key).is_none());
        assert_eq!(cache.used(), 0);
        let other = Key::new(6, "x", "");
        assert!(!cache.contains(&key));
        assert!(cache.put(&other, &payload(1000, 0), 0, None).unwrap());
        assert!(cache.get(&other).is_some());
        assert_eq!(
            cache.used(),
            fs::metadata(dir.join(other.file_name())).unwrap().len()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
