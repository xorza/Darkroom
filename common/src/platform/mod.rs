//! What differs between operating systems, behind one API that is the same on each: files a run
//! writes and reads back through a map, deleted while they are open; the memory the process can
//! allocate; the user's cache directory; the file systems a directory lies on; replacing a file
//! in one step, making a rename durable, and what the system states of a file beyond its length
//! and modification time.
//!
//! Each system's side is a file of its own, picked here and nowhere else: `linux`, `macos` and
//! `windows` for the system, `unix` and `windows` for how files behave, which the two Unix systems
//! share. A capability a system lacks is part of the API — a documented `None` — not a branch a
//! caller has to know about.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(windows)]
use windows as os;

#[cfg(unix)]
use unix as files;
#[cfg(windows)]
use windows as files;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
compile_error!("common runs on Linux, macOS and Windows");

use std::ffi::OsString;
use std::fs::{File, Metadata};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::fs::PublicationMode;
use crate::fs::file_identity::FileId;

/// A file one run writes once and reads back through a map, deleted while it is open: on Unix it
/// is unlinked as soon as it is created, before any data goes in, and lives on in its map; on
/// Windows it is opened to be deleted when its last handle closes, and the map holds the handle.
/// Nothing is left for a later run after a crash, and no other run can open a file with no name.
#[derive(Debug)]
pub struct ScratchFile {
    file: File,
}

impl ScratchFile {
    /// A new file at `path`, created exclusively — an existing name fails with
    /// [`io::ErrorKind::AlreadyExists`] — and deleted while open.
    pub fn create_new(path: impl AsRef<Path>) -> io::Result<Self> {
        files::create_deleted(path.as_ref()).map(|file| Self { file })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)
    }

    /// The file mapped back, with what keeps it alive for the map's life; its handle kept open
    /// for `MappedFile::evict` too when `eviction` allows it.
    pub fn map(self, eviction: Eviction) -> io::Result<MappedFile> {
        let map = unsafe { Mmap::map(&self.file) }?;
        let keep_handle = match eviction {
            Eviction::Never => files::SCRATCH_MAP_NEEDS_HANDLE,
            #[cfg(feature = "internals")]
            Eviction::Allowed => true,
        };
        Ok(MappedFile::new(map, keep_handle.then_some(self.file)))
    }
}

/// Whether a scratch map may have its pages dropped from the page cache on demand, which keeps its
/// file's handle open for as long as it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eviction {
    /// A run's scratch: no handle held per plane where the map does not need one.
    Never,
    /// A bench of cold reads' scratch, which `MappedFile::evict` drops between passes.
    #[cfg(feature = "internals")]
    Allowed,
}

/// A file mapped into memory for reading, read front to back.
#[derive(Debug)]
pub struct MappedFile {
    map: Mmap,
    /// The file under the map, held open where the map needs it — Windows deletes a scratch file
    /// when its last handle closes — and for a scratch a bench of cold reads opens to evict, as a
    /// page cache is dropped by file, not by mapping. `None` elsewhere, so a run holds no handle
    /// per plane.
    _handle: Option<File>,
}

impl MappedFile {
    /// The named file at `path`, mapped.
    ///
    /// # Safety
    ///
    /// Nothing changes the file while it is mapped: a reader whose files are written whole under
    /// their names and replaced only by a rename meets this for every file it wrote.
    pub unsafe fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file) }?;
        Ok(Self::new(map, None))
    }

    /// The file `file` holds, mapped: for a writer that maps the handle it wrote through before the
    /// file takes its name, so another writer that replaces the name later is never read instead.
    ///
    /// # Safety
    ///
    /// As [`Self::open`]: nothing changes the file while it is mapped.
    pub unsafe fn of_file(file: &File) -> io::Result<Self> {
        let map = unsafe { Mmap::map(file) }?;
        Ok(Self::new(map, None))
    }

    /// `map`, with the handle of its file kept while it lives when one is given.
    fn new(map: Mmap, handle: Option<File>) -> Self {
        files::advise_sequential(&map);
        Self {
            map,
            _handle: handle,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }
}

/// The memory this process can allocate now: the host's available memory, or less where the
/// system confines the process — a Linux control group, a Windows job object. macOS has no such
/// limit to read.
#[derive(Debug, Clone, Copy)]
pub struct SystemMemory;

impl SystemMemory {
    /// Read once per run: a few tens of microseconds, against a decision every stage of the run
    /// keeps.
    pub fn available() -> u64 {
        let mut system = System::new();
        system.refresh_memory();
        os::available_memory(&system)
    }
}

/// This process's resident set, the figure Activity Monitor and Task Manager show against it.
///
/// Kept rather than rebuilt per reading: sysinfo carries per-process bookkeeping across
/// refreshes (on Linux it holds the `stat` file open), so a fresh one each time would redo that
/// setup for a single number.
#[derive(Debug)]
pub struct ResidentMemory {
    system: System,
    pid: Pid,
}

impl ResidentMemory {
    /// # Panics
    ///
    /// On a target sysinfo has no implementation for: a build-configuration error, not a runtime
    /// condition.
    pub fn new() -> Self {
        let pid = sysinfo::get_current_pid().expect("sysinfo cannot resolve this platform's pid");
        Self {
            system: System::new(),
            pid,
        }
    }

    /// The resident bytes now, read from the system.
    pub fn read(&mut self) -> u64 {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            // This is the process being refreshed: there is no corpse to reap.
            false,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.system.process(self.pid).map_or(0, Process::memory)
    }
}

impl Default for ResidentMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// The user's cache directory.
#[derive(Debug, Clone, Copy)]
pub struct UserCache;

impl UserCache {
    /// The platform's user cache directory, from the environment `var` reads, with `app`
    /// beneath it: `$XDG_CACHE_HOME`, else `~/.cache`, on Linux; `~/Library/Caches` on macOS;
    /// `%LOCALAPPDATA%` on Windows; where none is stated, a disk-backed directory of the system's.
    pub fn root(var: &dyn Fn(&str) -> Option<OsString>, app: &str) -> PathBuf {
        os::user_cache_root(&|name| {
            var(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        })
        .join(app)
    }
}

/// The file system a directory lies on.
#[derive(Debug, Clone, Copy)]
pub struct FileSystem;

impl FileSystem {
    /// The type of the memory-backed file system `path` lies on — tmpfs, ramfs — where writing
    /// spends RAM rather than disk; `None` on a disk, or where the system has no table of mounts
    /// to read, as macOS and Windows have none.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            clippy::missing_const_for_fn,
            reason = "const where there is no table to read, not on Linux"
        )
    )]
    pub fn memory_backed(path: &Path) -> Option<String> {
        os::memory_backed(path)
    }

    /// The bytes this user may still write to the file system `path` lies on; `None` when no
    /// figure for that file system can be read, rather than another's. `path` must exist.
    pub fn free_space(path: &Path) -> Option<u64> {
        files::free_space(path)
    }
}

/// What the system states of a file beside its length and modification time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stated {
    pub(crate) file_id: Option<FileId>,
    pub(crate) changed_ns: Option<i128>,
}

/// A file's metadata and what the system states of it beside, read together.
#[derive(Debug)]
pub(crate) struct Observed {
    pub(crate) metadata: Metadata,
    pub(crate) stated: Stated,
}

/// Replace `destination` with `source` in one step: a reader sees the old file or the new one.
pub(crate) fn replace(source: &Path, destination: &Path, mode: PublicationMode) -> io::Result<()> {
    files::replace(source, destination, mode)
}

/// A link at `link` to the file `original`.
pub(crate) fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    files::symlink(original, link)
}

/// Make the renames in `parent` durable, where the system needs a directory synced for that.
#[cfg_attr(
    not(unix),
    expect(
        clippy::missing_const_for_fn,
        reason = "const where no directory is synced, not on Unix"
    )
)]
pub(crate) fn sync_parent(parent: &Path) -> io::Result<()> {
    files::sync_parent(parent)
}

/// What `metadata` states beside length and modification time, on a system whose metadata holds
/// it: Unix's does, Windows' does not.
pub(crate) fn stated(metadata: &Metadata) -> Stated {
    files::stated(metadata)
}

/// The file `path` names, following a link, with everything the system states of it.
pub(crate) fn observe(path: &Path) -> io::Result<Observed> {
    files::observe(path)
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::path::PathBuf;

    use crate::platform::{FileSystem, MappedFile, ScratchFile, files, os};

    impl FileSystem {
        /// A temporary directory on disk: the system's — `%TMP%`, else `%TEMP%`, on Windows;
        /// `$TMPDIR`, else `/tmp`, on macOS and Linux — else `/var/tmp` on Linux; `None` when
        /// both are memory-backed.
        pub(crate) fn disk_temp_dir() -> Option<PathBuf> {
            os::internals::disk_temp_dir()
        }
    }

    impl ScratchFile {
        /// Whether a scratch file keeps its name in its directory while it is open: not on Unix,
        /// which unlinks it as it is created; on Windows, which deletes it as its last handle
        /// closes.
        pub const KEEPS_NAME: bool = files::internals::SCRATCH_KEEPS_NAME;
    }

    impl MappedFile {
        /// Whether [`Self::evict`] can drop a file's pages on this system: on Linux, not on macOS
        /// or Windows here. A bench of cold reads runs only where it can.
        pub const EVICTS: bool = os::internals::EVICTS;

        /// Drop the file's pages from the page cache, so the next read comes from the disk: what a
        /// bench of cold reads does between passes. `false` when a page stays, as on tmpfs, or
        /// where the system offers no way, as macOS and Windows here: the next read may come from
        /// memory.
        ///
        /// # Panics
        ///
        /// For a map not opened to evict: its handle is gone (see
        /// `RunScratch::create_evictable`).
        #[expect(
            clippy::used_underscore_binding,
            reason = "the handle lives for the map's life; an eviction is the one read of it"
        )]
        #[cfg_attr(
            not(target_os = "linux"),
            expect(
                clippy::missing_const_for_fn,
                reason = "const only where the system offers no eviction and the call does nothing"
            )
        )]
        pub fn evict(&self) -> bool {
            let file = self
                ._handle
                .as_ref()
                .expect("a map opened to evict keeps its file's handle");
            os::internals::evict(&self.map, file)
        }

        /// The share of the file's pages in memory; `None` where the system does not say.
        #[cfg_attr(
            not(target_os = "linux"),
            expect(
                clippy::missing_const_for_fn,
                reason = "const only where the system does not say and the call does nothing"
            )
        )]
        pub fn resident_share(&self) -> Option<f64> {
            os::internals::resident_share(&self.map)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::platform::SystemMemory;

    /// Each reading takes a system of its own, so the second is as good as the first: a stale or
    /// half-refreshed reading would show as a zero.
    #[test]
    fn available_memory_reports_a_figure_on_every_call() {
        assert!(
            SystemMemory::available() > 0,
            "no available memory reported"
        );
        assert!(SystemMemory::available() > 0, "second call reported none");
    }
}
