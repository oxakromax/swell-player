//! Giving freed memory back to the system.
//!
//! Closing a page frees its widgets and textures, but glibc keeps the freed
//! heap in its arenas, so RSS stays at its peak (measured: ~100 MB after
//! browsing 20 pages). A trim on idle after pages close (or a track's
//! pipeline stops) returns it, and [`configure`] keeps glibc from holding
//! on to much in the first place. Other allocators (musl, Windows, macOS)
//! have no such calls: nothing to do.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc {
    use std::cell::Cell;
    use std::time::Duration;

    use gtk::glib;

    unsafe extern "C" {
        /// glibc: releases free memory at the top of the heap and in arenas.
        fn malloc_trim(pad: usize) -> i32;
        /// glibc: sets one of the allocator's parameters.
        fn mallopt(param: i32, value: i32) -> i32;
    }

    /// `mallopt`'s parameter for the most arenas (glibc's `malloc.h`).
    const M_ARENA_MAX: i32 = -8;

    pub fn configure() {
        // The user's own setting wins (other tunables do not count).
        let tunables = std::env::var_os("GLIBC_TUNABLES").unwrap_or_default();
        if std::env::var_os("MALLOC_ARENA_MAX").is_some()
            || tunables
                .to_string_lossy()
                .contains("glibc.malloc.arena_max")
        {
            return;
        }
        // Safety: mallopt has no preconditions; it is called before any
        // other thread exists, as glibc advises.
        unsafe { mallopt(M_ARENA_MAX, 2) };
    }

    thread_local! {
        static PENDING: Cell<bool> = const { Cell::new(false) };
    }

    pub fn trim_soon() {
        if PENDING.with(|pending| pending.replace(true)) {
            return;
        }
        glib::timeout_add_local_once(Duration::from_secs(2), || {
            PENDING.with(|pending| pending.set(false));
            // Safety: malloc_trim has no preconditions; it only walks
            // glibc's own allocator state under its locks.
            let released = unsafe { malloc_trim(0) };
            log::debug!("heap trimmed (released: {})", released != 0);
        });
    }
}

/// Sets the allocator up for a long-running player. Call first in `main`,
/// before any other thread exists.
///
/// Every track allocates tens of MB of buffered audio on GStreamer's
/// streaming threads, which start anew for every track. glibc gives each
/// new thread an arena of its own (up to eight per core) and keeps what is
/// freed in each, so RSS rose with every track although nothing leaked
/// (measured: 85 MB after 3 tracks, 125 MB after 30 and still rising, with
/// 10–30 MB in use). With at most two arenas the next track reuses
/// what the last one freed; threads keep small caches of their own, so
/// contention stays low. The trims after a track's pipeline stops give
/// back the rest.
pub fn configure() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::configure();
}

/// Trims the heap a couple of seconds from now (coalescing bursts, e.g.
/// popping several pages, or a track change). Main thread only.
pub fn trim_soon() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    glibc::trim_soon();
}
