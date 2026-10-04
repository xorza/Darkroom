//! [`MountTable`]: the file systems this process sees, from `/proc/self/mountinfo`.

use std::path::{Path, PathBuf};

/// The mounts of this process's mount namespace, as `proc(5)` lists them in
/// `/proc/self/mountinfo`. Linux only: elsewhere [`Self::read`] finds none.
#[derive(Debug, Default)]
pub(crate) struct MountTable {
    /// In the order the kernel lists them, so a later mount over the same point shadows an earlier.
    mounts: Vec<Mount>,
}

/// One line of `mountinfo`: what part of a file system is mounted where, and its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    /// The directory of the file system that forms the mount's root: `/` for a whole file system,
    /// a subdirectory for a bind mount or a cgroup namespace's view.
    pub(crate) root: PathBuf,
    pub(crate) mount_point: PathBuf,
    pub(crate) filesystem: String,
    /// The per-superblock options, comma-separated; a v1 cgroup hierarchy names its controllers
    /// here.
    pub(crate) super_options: String,
}

impl Mount {
    /// A file system that keeps its files in memory: writing to it spends RAM, not disk.
    pub(crate) fn is_memory_backed(&self) -> bool {
        matches!(self.filesystem.as_str(), "tmpfs" | "ramfs")
    }
}

impl MountTable {
    /// This process's mounts; empty where there is no `/proc/self/mountinfo` to read.
    pub(crate) fn read() -> Self {
        std::fs::read_to_string("/proc/self/mountinfo")
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// The mounts of `mountinfo`'s text, skipping a line that does not have its fields.
    ///
    /// A line is `id parent major:minor root mount-point options [optional…] - type source
    /// super-options`; the optional fields end at the lone `-`. Paths escape space, tab, newline
    /// and backslash as three octal digits.
    pub(crate) fn parse(mountinfo: &str) -> Self {
        let mounts = mountinfo
            .lines()
            .filter_map(|line| {
                let mut fields = line.split(' ');
                let root = fields.nth(3)?;
                let mount_point = fields.next()?;
                let mut after_separator = fields.skip_while(|&field| field != "-").skip(1);
                let filesystem = after_separator.next()?;
                let super_options = after_separator.nth(1).unwrap_or_default();
                Some(Mount {
                    root: PathBuf::from(unescape(root)),
                    mount_point: PathBuf::from(unescape(mount_point)),
                    filesystem: filesystem.to_owned(),
                    super_options: super_options.to_owned(),
                })
            })
            .collect();
        Self { mounts }
    }

    /// The mount `path` lies on: the one whose mount point is the longest ancestor of the path,
    /// the last listed of those on the same point. `path` is taken as given, so the caller
    /// resolves symbolic links first.
    pub(crate) fn mount_of(&self, path: &Path) -> Option<&Mount> {
        self.mounts
            .iter()
            .filter(|mount| path.starts_with(&mount.mount_point))
            .max_by_key(|mount| mount.mount_point.components().count())
    }

    /// The last mount of file system type `filesystem` whose super options satisfy `options`.
    pub(crate) fn find(&self, filesystem: &str, options: impl Fn(&str) -> bool) -> Option<&Mount> {
        self.mounts
            .iter()
            .rev()
            .find(|mount| mount.filesystem == filesystem && options(&mount.super_options))
    }
}

/// `mountinfo`'s octal escapes undone: `\040` is a space, `\011` a tab, `\012` a newline and
/// `\134` a backslash.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|digits| bytes[i] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)));
        if let Some(digits) = octal {
            out.push(
                digits
                    .iter()
                    .fold(0u8, |value, d| value.wrapping_mul(8).wrapping_add(d - b'0')),
            );
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::mount_table::MountTable;

    /// Lines in the shape `proc(5)` documents: optional fields of any count before the `-`, an
    /// escaped space in a mount point, a bind mount with its own root, and `/tmp` mounted twice,
    /// the second time as tmpfs over a disk directory.
    const MOUNTINFO: &str = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw,errors=remount-ro
25 22 0:22 / /sys/fs/cgroup rw,nosuid shared:9 - cgroup2 cgroup2 rw,nsdelegate
26 22 8:2 /srv/data /mnt/my\\040data rw - ext4 /dev/sda2 rw
27 22 8:2 /tmp /tmp rw shared:4 master:2 - ext4 /dev/sda2 rw
28 27 0:30 / /tmp rw,nosuid - tmpfs tmpfs rw,size=13631488k
29 22 0:31 / /sys/fs/cgroup/memory rw - cgroup cgroup rw,memory
malformed line
";

    #[test]
    fn a_path_lies_on_its_longest_mount_point() {
        let table = MountTable::parse(MOUNTINFO);
        let filesystem = |path: &str| table.mount_of(Path::new(path)).unwrap().filesystem.clone();
        assert_eq!(filesystem("/home/user/.cache/lumos"), "ext4");
        assert_eq!(filesystem("/tmp/lumos"), "tmpfs");
        assert_eq!(filesystem("/tmp"), "tmpfs");
        assert!(
            table
                .mount_of(Path::new("/tmp/lumos"))
                .unwrap()
                .is_memory_backed()
        );
        assert!(
            !table
                .mount_of(Path::new("/var/tmp"))
                .unwrap()
                .is_memory_backed()
        );

        let data = table.mount_of(Path::new("/mnt/my data/x")).unwrap();
        assert_eq!(data.mount_point, PathBuf::from("/mnt/my data"));
        assert_eq!(data.root, PathBuf::from("/srv/data"));
        // `/mnt/my` is not an ancestor of `/mnt/my data`: components, not string prefixes.
        assert_eq!(filesystem("/mnt/my"), "ext4");
        assert_eq!(
            table.mount_of(Path::new("/mnt/my")).unwrap().mount_point,
            Path::new("/")
        );

        let v1 = table
            .find("cgroup", |options| {
                options.split(',').any(|option| option == "memory")
            })
            .unwrap();
        assert_eq!(v1.mount_point, PathBuf::from("/sys/fs/cgroup/memory"));
        assert_eq!(
            table.find("cgroup2", |_| true).unwrap().super_options,
            "rw,nsdelegate"
        );
        assert!(MountTable::parse("").mount_of(Path::new("/")).is_none());
    }
}
