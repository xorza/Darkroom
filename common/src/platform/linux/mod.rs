//! Linux: control groups bound the process's memory, `mountinfo` names the file systems, and the
//! XDG base directories name the cache.

mod cgroup_memory;
mod mount_table;

use std::path::{Path, PathBuf};

use sysinfo::System;

use crate::platform::linux::cgroup_memory::CgroupMemory;
use crate::platform::linux::mount_table::MountTable;

/// The host's available memory, or less where this process's control groups leave less.
pub(super) fn available_memory(system: &System) -> u64 {
    let host = system.available_memory();
    CgroupMemory::of_process(&MountTable::read())
        .and_then(|groups| groups.available())
        .map_or(host, |groups| host.min(groups))
}

/// The disk-backed directory every Linux system has: `file-hierarchy(7)` names it the place for
/// larger temporary files, and the FHS keeps it across reboots.
const VAR_TMP: &str = "/var/tmp";

/// `$XDG_CACHE_HOME` when absolute, as the XDG specification says, else `~/.cache`, else
/// [`VAR_TMP`].
pub(super) fn user_cache_root(var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    var("XDG_CACHE_HOME")
        .or_else(|| var("HOME").map(|home| home.join(".cache")))
        .unwrap_or_else(|| PathBuf::from(VAR_TMP))
}

/// The memory-backed file system `path` lies on, from this process's `mountinfo`.
pub(super) fn memory_backed(path: &Path) -> Option<String> {
    memory_backed_in(&MountTable::read(), path)
}

/// The memory-backed file system `path` lies on among `mounts`.
fn memory_backed_in(mounts: &MountTable, path: &Path) -> Option<String> {
    mounts
        .mount_of(path)
        .filter(|mount| mount.is_memory_backed())
        .map(|mount| mount.filesystem.clone())
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::env;
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;

    use memmap2::Mmap;

    use crate::fs;
    use crate::platform::linux::mount_table::MountTable;
    use crate::platform::linux::{VAR_TMP, memory_backed_in};

    /// The system's temporary directory, else [`VAR_TMP`], whichever resolves first to a directory
    /// on disk by this process's `mountinfo`: `/tmp` is tmpfs on Debian 13 and Arch. Resolved, as
    /// the mount a link leads to is the one written to.
    pub(crate) fn disk_temp_dir() -> Option<PathBuf> {
        let resolved = [env::temp_dir(), PathBuf::from(VAR_TMP)]
            .into_iter()
            .filter_map(|dir| fs::canonicalize(dir).ok());
        first_on_disk(&MountTable::read(), resolved)
    }

    /// The first of `dirs` not memory-backed among `mounts`.
    pub(super) fn first_on_disk(
        mounts: &MountTable,
        dirs: impl IntoIterator<Item = PathBuf>,
    ) -> Option<PathBuf> {
        dirs.into_iter()
            .find(|dir| memory_backed_in(mounts, dir).is_none())
    }

    /// [`evict`] drops a file's pages by `POSIX_FADV_DONTNEED`.
    pub(crate) const EVICTS: bool = true;

    /// The page size, in bytes.
    fn page_size() -> usize {
        let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        usize::try_from(size).expect("the page size is positive")
    }

    /// Drop the pages of `file`, which `map` maps, from the page cache, as `vmtouch -e` does:
    /// `fdatasync` cleans them, `MADV_DONTNEED` takes them out of this process's map, which reads
    /// the file again on its next touch, and `POSIX_FADV_DONTNEED` drops every clean page no map
    /// holds, draining each CPU's batch of pages not yet on the LRU first. `MADV_PAGEOUT` alone
    /// misses the pages still in such a batch, and a file just written holds many. Whether no page
    /// is left in memory after, by `mincore`: a file on tmpfs, whose pages are the file, keeps
    /// every one while each call succeeds.
    pub(crate) fn evict(map: &Mmap, file: &File) -> bool {
        if map.is_empty() {
            return true;
        }
        // A failed call leaves pages in memory, which the count after reports.
        file.sync_data().ok();
        let start = map.as_ptr().cast::<c_void>().cast_mut();
        unsafe { libc::madvise(start, map.len(), libc::MADV_DONTNEED) };
        unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        resident_share(map) == Some(0.0)
    }

    /// The share of `map`'s pages in memory, by `mincore`; `None` when the call fails.
    pub(crate) fn resident_share(map: &Mmap) -> Option<f64> {
        let pages = map.len().div_ceil(page_size());
        if pages == 0 {
            return Some(1.0);
        }
        let mut residency = vec![0u8; pages];
        let start = map.as_ptr().cast::<c_void>().cast_mut();
        let answered = unsafe { libc::mincore(start, map.len(), residency.as_mut_ptr()) } == 0;
        answered
            .then(|| residency.iter().filter(|&&page| page & 1 == 1).count() as f64 / pages as f64)
    }
}

#[cfg(test)]
mod tests;
