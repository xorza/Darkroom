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

/// `$XDG_CACHE_HOME` when absolute, as the XDG specification says, else `~/.cache`, else
/// `/var/tmp`, a disk-backed directory every Linux system has.
pub(super) fn user_cache_root(var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    var("XDG_CACHE_HOME")
        .or_else(|| var("HOME").map(|home| home.join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/var/tmp"))
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

#[cfg(any(test, feature = "bench"))]
pub(crate) mod internals {
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    use memmap2::Mmap;

    /// [`evict`] drops a file's pages by `POSIX_FADV_DONTNEED`.
    pub(crate) const EVICTS: bool = true;

    /// The page size, in bytes.
    fn page_size() -> usize {
        // SAFETY: `sysconf` reads a constant of the system.
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
        let _ = file.sync_data();
        let start = map.as_ptr().cast::<c_void>().cast_mut();
        // SAFETY: the range is the map's own, a shared read-only map of a file, whose pages the
        // next touch reads back from the file unchanged.
        unsafe { libc::madvise(start, map.len(), libc::MADV_DONTNEED) };
        // SAFETY: the descriptor is the open file's own.
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
        // SAFETY: the range is the map's own, and the vector holds one byte per page of it.
        let answered = unsafe { libc::mincore(start, map.len(), residency.as_mut_ptr()) } == 0;
        answered
            .then(|| residency.iter().filter(|&&page| page & 1 == 1).count() as f64 / pages as f64)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use crate::platform::linux::memory_backed_in;
    use crate::platform::linux::mount_table::MountTable;
    use crate::platform::{FileSystem, UserCache};

    /// A directory on tmpfs or ramfs is memory-backed, one on ext4 is not, and a directory below
    /// the mount point is held to it: a table with `/data` mounted as each in turn.
    #[test]
    fn a_directory_on_tmpfs_or_ramfs_is_memory_backed() {
        let mounts = |filesystem: &str| {
            MountTable::parse(&format!(
                "22 1 8:2 / / rw - ext4 /dev/sda2 rw\n30 22 0:40 / /data rw - {filesystem} none rw\n"
            ))
        };
        for (filesystem, expected) in [
            ("tmpfs", Some("tmpfs")),
            ("ramfs", Some("ramfs")),
            ("ext4", None),
        ] {
            assert_eq!(
                memory_backed_in(&mounts(filesystem), Path::new("/data/cache/lumos")).as_deref(),
                expected,
                "{filesystem}"
            );
        }
        assert_eq!(
            memory_backed_in(&mounts("tmpfs"), Path::new("/home/u")),
            None
        );
    }

    /// `XDG_CACHE_HOME` when absolute, then `~/.cache`, then `/var/tmp`, each with `lumos` below.
    #[test]
    fn the_cache_root_follows_xdg() {
        let root = |vars: &[(&str, &str)]| {
            UserCache::root(&|name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            })
        };
        assert_eq!(
            root(&[("XDG_CACHE_HOME", "/data/cache"), ("HOME", "/home/u")]),
            PathBuf::from("/data/cache/lumos")
        );
        assert_eq!(
            root(&[("XDG_CACHE_HOME", "relative"), ("HOME", "/home/u")]),
            PathBuf::from("/home/u/.cache/lumos")
        );
        assert_eq!(root(&[]), PathBuf::from("/var/tmp/lumos"));
    }

    /// A directory on a listed disk has that disk's figure. `/proc` is on a device no disk is
    /// mounted from, so it has none, where a match on the path would give the root disk's.
    #[test]
    fn free_space_is_the_figure_of_the_directorys_own_device() {
        assert!(FileSystem::free_space(Path::new(env!("CARGO_MANIFEST_DIR"))).is_some());
        assert_eq!(FileSystem::free_space(Path::new("/proc")), None);
        assert_eq!(
            FileSystem::free_space(Path::new("/no/such/directory")),
            None
        );
    }
}
