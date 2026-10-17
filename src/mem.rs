//! Giving freed memory back to the system.
//!
//! Closing a page frees its widgets and textures, but glibc keeps the freed
//! heap in its arenas, so RSS stays at its peak (measured: ~100 MB after
//! browsing 20 pages). A trim on idle after pages close returns it.

use std::cell::Cell;
use std::time::Duration;

use gtk::glib;

unsafe extern "C" {
    /// glibc: releases free memory at the top of the heap and in arenas.
    fn malloc_trim(pad: usize) -> i32;
}

thread_local! {
    static PENDING: Cell<bool> = const { Cell::new(false) };
}

/// Trims the heap a couple of seconds from now (coalescing bursts, e.g.
/// popping several pages). Main thread only.
pub fn trim_soon() {
    if PENDING.with(|pending| pending.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(Duration::from_secs(2), || {
        PENDING.with(|pending| pending.set(false));
        // Safety: malloc_trim has no preconditions; it only walks glibc's
        // own allocator state under its locks.
        let released = unsafe { malloc_trim(0) };
        log::debug!("heap trimmed (released: {})", released != 0);
    });
}
