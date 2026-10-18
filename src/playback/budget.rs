//! The memory budget for prefetched audio, as pure accounting.
//!
//! Prefetched bytes stay in memory (never on disk: nothing to clean up, no
//! wear, nothing left behind after a crash). The budget is a fixed cap,
//! lowered when the system is short of memory; it is never exceeded:
//! space is reserved before bytes are downloaded, and when it does not fit,
//! entries that are less wanted are evicted (tracks no longer upcoming
//! first, then those furthest ahead, least recently used first among
//! equals) or the download is skipped. Evicting an entry only drops its
//! cached bytes: a track that plays anyway streams straight from TIDAL.

use std::hash::Hash;

/// The most prefetched audio kept, in bytes.
pub const MAX_BUDGET: usize = 64 << 20;
/// Below this, prefetching audio is not worth it.
const MIN_BUDGET: usize = 4 << 20;
/// The budget is at most this fraction of the memory available.
const MEMORY_SHARE: u64 = 16;

/// The budget, given the memory the system has available (bytes; `None`
/// when unknown): [`MAX_BUDGET`], or a sixteenth of what is available if
/// that is less, or nothing when even that is too little.
pub fn budget(available_memory: Option<u64>) -> usize {
    let share = available_memory.map_or(MAX_BUDGET as u64, |m| m / MEMORY_SHARE);
    let budget = usize::try_from(share).unwrap_or(usize::MAX).min(MAX_BUDGET);
    if budget < MIN_BUDGET { 0 } else { budget }
}

/// Memory available to start new work without swapping, in bytes (Linux:
/// `MemAvailable`); `None` elsewhere.
pub fn available_memory() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_mem_available(&meminfo)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_mem_available(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// A cached entry, as the budget sees it.
#[derive(Clone, Debug)]
pub struct Item<K> {
    pub key: K,
    pub bytes: usize,
    /// Position among the wanted tracks (0: playing now); `None`: not
    /// wanted any more.
    pub rank: Option<usize>,
    /// When it was last used (any increasing counter).
    pub last_used: u64,
}

/// The entries to evict so that `need` more bytes fit in `budget` when
/// `used` bytes are taken (cached or reserved), for an entry of `rank`.
/// Only entries less wanted than it may go: ones with a worse rank, or not
/// wanted at all. `None` when it cannot fit; an empty list when it fits as
/// it is.
pub fn evictions<K: Clone + Eq + Hash>(
    items: &[Item<K>],
    used: usize,
    budget: usize,
    need: usize,
    rank: usize,
) -> Option<Vec<K>> {
    if need > budget {
        return None;
    }
    let mut free = budget.saturating_sub(used);
    if need <= free {
        return Some(Vec::new());
    }
    let mut candidates: Vec<&Item<K>> = items
        .iter()
        .filter(|item| item.bytes > 0 && item.rank.is_none_or(|r| r > rank))
        .collect();
    // Not wanted first, then furthest ahead; least recently used first.
    candidates.sort_by_key(|item| {
        (
            std::cmp::Reverse(item.rank.unwrap_or(usize::MAX)),
            item.last_used,
        )
    });
    let mut evicted = Vec::new();
    for item in candidates {
        if need <= free {
            break;
        }
        free += item.bytes;
        evicted.push(item.key.clone());
    }
    (need <= free).then_some(evicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: u32, bytes: usize, rank: Option<usize>, last_used: u64) -> Item<u32> {
        Item {
            key,
            bytes,
            rank,
            last_used,
        }
    }

    #[test]
    fn budget_follows_available_memory() {
        assert_eq!(budget(None), MAX_BUDGET);
        assert_eq!(budget(Some(16 << 30)), MAX_BUDGET);
        assert_eq!(budget(Some(512 << 20)), 32 << 20);
        assert_eq!(budget(Some(32 << 20)), 0);
        assert_eq!(budget(Some(0)), 0);
        let meminfo = "MemTotal:       65000000 kB\nMemFree:  1 kB\nMemAvailable:   26000000 kB\n";
        assert_eq!(parse_mem_available(meminfo), Some(26_000_000 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB"), None);
    }

    #[test]
    fn fits_without_evicting() {
        let items = [item(1, 10, Some(1), 0)];
        assert_eq!(evictions(&items, 10, 100, 90, 2), Some(vec![]));
        assert_eq!(evictions(&items, 10, 100, 91, 0), Some(vec![1]));
        // Never more than the whole budget.
        assert_eq!(evictions::<u32>(&[], 0, 100, 101, 0), None);
    }

    #[test]
    fn evicts_unwanted_then_furthest_then_least_recent() {
        let items = [
            item(1, 30, Some(1), 5),
            item(2, 30, Some(3), 9),
            item(3, 30, None, 7),
            item(4, 30, Some(3), 2),
            item(5, 0, None, 0),
        ];
        // 10 free: the unwanted one, then the stalest of the furthest
        // ahead, only as many as needed.
        assert_eq!(evictions(&items, 120, 130, 40, 0), Some(vec![3]));
        assert_eq!(evictions(&items, 120, 130, 41, 0), Some(vec![3, 4]));
        // Rank 2 may only evict what is less wanted than itself.
        assert_eq!(evictions(&items, 120, 130, 71, 2), Some(vec![3, 4, 2]));
        assert_eq!(evictions(&items, 120, 130, 101, 2), None);
        // Rank 3 cannot evict its equals.
        assert_eq!(evictions(&items, 120, 130, 41, 3), None);
        assert_eq!(evictions(&items, 120, 130, 40, 3), Some(vec![3]));
    }

    #[test]
    fn accounting_never_exceeds_the_budget() {
        // Simulate a cache taking entries of varying sizes and ranks.
        let budget = 1000;
        let mut items: Vec<Item<u32>> = Vec::new();
        for n in 0..500u32 {
            let need = ((n * 37) % 400 + 1) as usize;
            let rank = (n % 5) as usize;
            let used: usize = items.iter().map(|i| i.bytes).sum();
            if let Some(evicted) = evictions(&items, used, budget, need, rank) {
                items.retain(|i| !evicted.contains(&i.key));
                items.push(item(n, need, Some(rank), u64::from(n)));
            }
            let used: usize = items.iter().map(|i| i.bytes).sum();
            assert!(used <= budget, "{used}");
            // Ranks age: the previous entries move one step closer.
            for i in &mut items {
                i.rank = i.rank.and_then(|r| r.checked_sub(1));
            }
        }
    }
}
