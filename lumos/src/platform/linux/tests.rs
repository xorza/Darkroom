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
