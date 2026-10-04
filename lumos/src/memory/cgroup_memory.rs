//! [`CgroupMemory`]: what this process's control groups leave it to allocate.

use std::fs;
use std::path::{Path, PathBuf};

use crate::mount_table::{Mount, MountTable};

/// The memory limits of this process's control group and every group above it, against their
/// use; Linux only.
///
/// `MemAvailable` is the host's figure. In a container, a systemd unit with `MemoryMax`, or a CI
/// runner, the kernel enforces the group's limit instead, and a process planned against the host
/// is killed rather than told to spill. Each level of the hierarchy can cap the ones below it, so
/// the figure is the least any level leaves.
///
/// A group's use counts its page cache, which the kernel reclaims before it reaches the limit, so
/// the inactive part of that cache is taken as free — the working set Kubernetes evicts on. The
/// active part is reclaimed too, but only under pressure, so counting it as used keeps the plan
/// on the safe side. cgroup v2's `memory.high` throttles allocation past it, so it caps the
/// figure as `memory.max` does.
#[derive(Debug)]
pub(crate) struct CgroupMemory {
    /// The group directories from this process's own up to each hierarchy's mount point: the v2
    /// hierarchy's, the v1 memory controller's, or both on a host that mounts both — each enforces
    /// its own limits.
    levels: Vec<Level>,
}

/// One group directory, and the interface its hierarchy speaks.
#[derive(Debug)]
struct Level {
    path: PathBuf,
    version: CgroupVersion,
}

/// Which cgroup interface a hierarchy speaks: the files a level's limit, use and cache are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CgroupVersion {
    V2,
    V1,
}

impl CgroupMemory {
    /// This process's groups; `None` off Linux, or where its memory hierarchy cannot be found.
    pub(crate) fn of_process(mounts: &MountTable) -> Option<Self> {
        let membership = fs::read_to_string("/proc/self/cgroup").ok()?;
        Self::locate(&membership, mounts, Path::new("/"))
    }

    /// The groups `membership` — the text of `/proc/<pid>/cgroup` — names, under the hierarchies
    /// `mounts` holds, with every path taken below `prefix` (`/` but in tests).
    ///
    /// A membership line is `id:controllers:path`: `0::path` for the v2 hierarchy, a list holding
    /// `memory` for v1's memory controller. The path is relative to the hierarchy's root, which a
    /// mount shows only from its own root down — a container sees its group as `/` — so the mount's
    /// root is taken off the path before it is placed under the mount point.
    fn locate(membership: &str, mounts: &MountTable, prefix: &Path) -> Option<Self> {
        let v2 = membership
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .zip(mounts.find("cgroup2", |_| true))
            .map(|(path, mount)| (path, mount, CgroupVersion::V2));
        let v1 = membership
            .lines()
            .find_map(|line| {
                let (_, rest) = line.split_once(':')?;
                let (controllers, path) = rest.split_once(':')?;
                controllers
                    .split(',')
                    .any(|controller| controller == "memory")
                    .then_some(path)
            })
            .zip(mounts.find("cgroup", |options| {
                options.split(',').any(|option| option == "memory")
            }))
            .map(|(path, mount)| (path, mount, CgroupVersion::V1));
        let below = |path: &Path| prefix.join(path.strip_prefix("/").unwrap_or(path));
        let levels: Vec<Level> = [v2, v1]
            .into_iter()
            .flatten()
            .filter_map(|(path, mount, version)| {
                let group = below(&Self::group_directory(Path::new(path), mount)?);
                let top = below(&mount.mount_point);
                Some(
                    group
                        .ancestors()
                        .take_while(|level| level.starts_with(&top))
                        .map(|path| Level {
                            path: path.to_path_buf(),
                            version,
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();
        (!levels.is_empty()).then_some(Self { levels })
    }

    /// Where the group at hierarchy `path` sits under `mount`: `None` when the mount shows only
    /// another part of the hierarchy.
    fn group_directory(path: &Path, mount: &Mount) -> Option<PathBuf> {
        let below_root = path.strip_prefix(&mount.root).ok()?;
        Some(mount.mount_point.join(below_root))
    }

    /// The least `limit − (use − inactive cache)` over the levels that set a limit; `None` when no
    /// level does.
    pub(crate) fn available(&self) -> Option<u64> {
        self.levels.iter().filter_map(Level::available).min()
    }
}

impl Level {
    fn available(&self) -> Option<u64> {
        let level = &self.path;
        let (limit_files, usage_file, inactive_key): (&[&str], _, _) = match self.version {
            CgroupVersion::V2 => (
                &["memory.max", "memory.high"],
                "memory.current",
                "inactive_file",
            ),
            CgroupVersion::V1 => (
                &["memory.limit_in_bytes"],
                "memory.usage_in_bytes",
                "total_inactive_file",
            ),
        };
        let limit = limit_files
            .iter()
            .filter_map(|file| read_limit(&level.join(file)))
            .min()?;
        let usage = read_number(&level.join(usage_file))?;
        let inactive = fs::read_to_string(level.join("memory.stat"))
            .ok()
            .and_then(|stat| {
                stat.lines().find_map(|line| {
                    let (key, value) = line.split_once(' ')?;
                    (key == inactive_key).then(|| value.trim().parse().ok())?
                })
            })
            .unwrap_or(0);
        Some(limit.saturating_sub(usage.saturating_sub(inactive)))
    }
}

/// A limit file's value; `None` for `max`, the unlimited v2 setting, and for v1's unlimited
/// sentinel, a page-rounded `i64::MAX` past any memory a machine has.
fn read_limit(path: &Path) -> Option<u64> {
    let value = read_number(path)?;
    (value < 1 << 62).then_some(value)
}

fn read_number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use common::TempDir;

    use crate::memory::cgroup_memory::CgroupMemory;
    use crate::mount_table::MountTable;

    /// One level of a fake hierarchy under `root`: `files` written into `path`.
    fn level(root: &Path, path: &str, files: &[(&str, &str)]) {
        let directory = root.join(path);
        fs::create_dir_all(&directory).unwrap();
        for (name, contents) in files {
            fs::write(directory.join(name), contents).unwrap();
        }
    }

    /// A v2 hierarchy, by hand. The unit's own group sets `memory.max` 8000 and `memory.high`
    /// 7000, uses 6500 of which 1500 is inactive cache: `7000 − (6500 − 1500)` = 2000. Its slice
    /// sets 10000 against 9500 used, 3000 inactive: `10000 − 6500` = 3500. The top level sets
    /// nothing. The least is 2000. With the unit's limits lifted to `max`, the slice's 3500 is what
    /// remains.
    #[test]
    fn the_least_any_level_leaves_with_inactive_cache_free() {
        let root = TempDir::new("cgroup_v2");
        let mounts =
            MountTable::parse("33 24 0:28 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw,nsdelegate\n");
        level(root.path(), "sys/fs/cgroup", &[("memory.stat", "anon 1\n")]);
        level(
            root.path(),
            "sys/fs/cgroup/work.slice",
            &[
                ("memory.max", "10000\n"),
                ("memory.high", "max\n"),
                ("memory.current", "9500\n"),
                ("memory.stat", "anon 5000\nfile 4500\ninactive_file 3000\n"),
            ],
        );
        let unit = "sys/fs/cgroup/work.slice/run.service";
        level(
            root.path(),
            unit,
            &[
                ("memory.max", "8000\n"),
                ("memory.high", "7000\n"),
                ("memory.current", "6500\n"),
                (
                    "memory.stat",
                    "anon 4000\ninactive_file 1500\nactive_file 1000\n",
                ),
            ],
        );
        let membership = "0::/work.slice/run.service\n";
        let memory = CgroupMemory::locate(membership, &mounts, root.path()).unwrap();
        assert_eq!(memory.levels.len(), 3);
        assert_eq!(memory.available(), Some(2000));

        level(
            root.path(),
            unit,
            &[("memory.max", "max\n"), ("memory.high", "max\n")],
        );
        assert_eq!(memory.available(), Some(3500));
    }

    /// A container sees its group as `/` under a mount whose root is the group's host path, so the
    /// root comes off before the group is placed; v1 reads its own files, and its unlimited
    /// sentinel sets no limit. Here the container's group sets 4096 against 3000 used, 1000 of it
    /// inactive: 2096. A membership the mounts cannot place, or no memory hierarchy, is `None`.
    #[test]
    fn a_namespaced_v1_group_and_the_unplaceable_ones() {
        let root = TempDir::new("cgroup_v1");
        let mounts = MountTable::parse(
            "40 30 0:40 /docker/abc /sys/fs/cgroup/memory ro - cgroup cgroup rw,memory\n",
        );
        level(
            root.path(),
            "sys/fs/cgroup/memory",
            &[
                ("memory.limit_in_bytes", "4096\n"),
                ("memory.usage_in_bytes", "3000\n"),
                ("memory.stat", "cache 2000\ntotal_inactive_file 1000\n"),
            ],
        );
        let membership = "12:cpu,cpuacct:/docker/abc\n11:memory:/docker/abc\n";
        let memory = CgroupMemory::locate(membership, &mounts, root.path()).unwrap();
        assert_eq!(memory.available(), Some(2096));

        level(
            root.path(),
            "sys/fs/cgroup/memory",
            &[("memory.limit_in_bytes", "9223372036854771712\n")],
        );
        assert_eq!(memory.available(), None);

        assert!(CgroupMemory::locate("11:memory:/elsewhere\n", &mounts, root.path()).is_none());
        assert!(CgroupMemory::locate(membership, &MountTable::default(), root.path()).is_none());
    }
}
