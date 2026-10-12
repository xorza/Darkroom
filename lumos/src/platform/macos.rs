//! macOS: no limit on a process's memory to read, no table of memory-backed mounts, and the cache
//! under `~/Library/Caches`.

use std::path::{Path, PathBuf};

use sysinfo::System;

/// The host's available memory.
pub(super) fn available_memory(system: &System) -> u64 {
    reported(
        system.available_memory(),
        system.total_memory(),
        system.used_memory(),
    )
}

/// The available memory macOS reported, or, when it reported none, the total less what is used:
/// macOS can report none when compressed pages exceed the free, inactive and purgeable ones.
const fn reported(available: u64, total: u64, used: u64) -> u64 {
    if available == 0 {
        total.saturating_sub(used)
    } else {
        available
    }
}

/// `~/Library/Caches`, else `/var/tmp`.
pub(super) fn user_cache_root(var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    var("HOME").map_or_else(
        || PathBuf::from("/var/tmp"),
        |home| home.join("Library/Caches"),
    )
}

/// None to read: macOS has no tmpfs, and a RAM disk is a disk image it does not list as one.
pub(super) const fn memory_backed(_path: &Path) -> Option<String> {
    None
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::fs::File;

    use memmap2::Mmap;

    /// This system offers no call that drops a file's pages from its cache here.
    pub(crate) const EVICTS: bool = false;

    /// Nothing: see [`EVICTS`].
    pub(crate) const fn evict(_map: &Mmap, _file: &File) -> bool {
        false
    }

    /// `None`: not read on this system.
    #[cfg(test)]
    pub(crate) const fn resident_share(_map: &Mmap) -> Option<f64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use crate::platform::UserCache;
    use crate::platform::macos::reported;

    /// The cache is under `~/Library/Caches`, and a reading of no available memory stands for the
    /// total less the used: 16 GiB less 10 GiB.
    #[test]
    fn the_cache_and_the_memory_follow_macos() {
        let home = |name: &str| (name == "HOME").then(|| OsString::from("/Users/u"));
        assert_eq!(
            UserCache::root(&home),
            PathBuf::from("/Users/u/Library/Caches/lumos")
        );
        let gib = 1 << 30;
        assert_eq!(reported(0, 16 * gib, 10 * gib), 6 * gib);
        assert_eq!(reported(3 * gib, 16 * gib, 10 * gib), 3 * gib);
    }
}
