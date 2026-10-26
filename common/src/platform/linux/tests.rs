use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::platform::linux::internals::first_on_disk;
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

/// A temporary directory on tmpfs gives way to `/var/tmp`, and one on disk stays: `/tmp` as tmpfs
/// and as ext4 in turn, a `$TMPDIR` under `/home` beside a tmpfs `/tmp`, and none when `/var/tmp`
/// is tmpfs as well.
#[test]
fn the_disk_temp_dir_leaves_a_memory_backed_tmp() {
    let mounts = |tmp: &str, var_tmp: &str| {
        MountTable::parse(&format!(
            "22 1 8:2 / / rw - ext4 /dev/sda2 rw\n\
             30 22 0:40 / /tmp rw - {tmp} none rw\n\
             31 22 0:41 / /var/tmp rw - {var_tmp} none rw\n"
        ))
    };
    let first = |mounts: &MountTable, temp: &str| {
        first_on_disk(mounts, [PathBuf::from(temp), PathBuf::from("/var/tmp")])
    };
    let path = |path: &str| Some(PathBuf::from(path));
    assert_eq!(first(&mounts("tmpfs", "ext4"), "/tmp"), path("/var/tmp"));
    assert_eq!(first(&mounts("ext4", "ext4"), "/tmp"), path("/tmp"));
    assert_eq!(
        first(&mounts("tmpfs", "ext4"), "/home/u/tmp"),
        path("/home/u/tmp")
    );
    assert_eq!(first(&mounts("tmpfs", "tmpfs"), "/tmp"), None);
}

/// `XDG_CACHE_HOME` when absolute, then `~/.cache`, then `/var/tmp`, each with `lumos` below.
#[test]
fn the_cache_root_follows_xdg() {
    let root = |vars: &[(&str, &str)]| {
        UserCache::root(
            &|name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            },
            "lumos",
        )
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
