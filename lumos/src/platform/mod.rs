//! What differs between operating systems, behind one API that is the same on each: files a run
//! writes and reads back through a map, deleted while they are open; the memory the process can
//! allocate; the user's cache directory; and the file systems a directory lies on.
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
compile_error!("lumos runs on Linux, macOS and Windows");

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use sysinfo::System;

/// A file one run writes once and reads back through a map, deleted while it is open: on Unix it
/// is unlinked as soon as it is created, before any data goes in, and lives on in its map; on
/// Windows it is opened to be deleted when its last handle closes, and the map holds the handle.
/// Nothing is left for a later run after a crash, and no other run can open a file with no name.
#[derive(Debug)]
pub(crate) struct ScratchFile {
    file: File,
}

impl ScratchFile {
    /// A new file at `path`, created exclusively — an existing name fails with
    /// [`io::ErrorKind::AlreadyExists`] — and deleted while open.
    pub(crate) fn create_new(path: &Path) -> io::Result<Self> {
        files::create_deleted(path).map(|file| Self { file })
    }

    pub(crate) fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)
    }

    /// The file mapped back, with what keeps it alive for the map's life; its handle kept open
    /// for `MappedFile::evict` too when `eviction` allows it.
    pub(crate) fn map(self, eviction: Eviction) -> io::Result<MappedFile> {
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
pub(crate) enum Eviction {
    /// A run's scratch: no handle held per plane where the map does not need one.
    Never,
    /// A bench of cold reads' scratch, which `MappedFile::evict` drops between passes.
    #[cfg(feature = "internals")]
    Allowed,
}

/// A file mapped into memory for reading, read front to back.
#[derive(Debug)]
pub(crate) struct MappedFile {
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
    pub(crate) unsafe fn open(path: &Path) -> io::Result<Self> {
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
    pub(crate) unsafe fn of_file(file: &File) -> io::Result<Self> {
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

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.map
    }
}

/// The memory this process can allocate now: the host's available memory, or less where the
/// system confines the process — a Linux control group, a Windows job object. macOS has no such
/// limit to read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SystemMemory;

impl SystemMemory {
    /// Read once per run: a few tens of microseconds, against a decision every stage of the run
    /// keeps.
    pub(crate) fn available() -> u64 {
        let mut system = System::new();
        system.refresh_memory();
        os::available_memory(&system)
    }
}

/// The user's cache directory.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UserCache;

impl UserCache {
    /// The platform's user cache directory, from the environment `var` reads, with `lumos`
    /// beneath it: `$XDG_CACHE_HOME`, else `~/.cache`, on Linux; `~/Library/Caches` on macOS;
    /// `%LOCALAPPDATA%` on Windows; where none is stated, a disk-backed directory of the system's.
    pub(crate) fn root(var: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
        os::user_cache_root(&|name| {
            var(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        })
        .join("lumos")
    }
}

/// The file system a directory lies on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileSystem;

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
    pub(crate) fn memory_backed(path: &Path) -> Option<String> {
        os::memory_backed(path)
    }

    /// The bytes this user may still write to the file system `path` lies on; `None` when no
    /// figure for that file system can be read, rather than another's. `path` must exist.
    pub(crate) fn free_space(path: &Path) -> Option<u64> {
        files::free_space(path)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::platform::{MappedFile, os};
    #[cfg(test)]
    use crate::platform::{ScratchFile, files};

    #[cfg(test)]
    impl ScratchFile {
        /// Whether a scratch file keeps its name in its directory while it is open: not on Unix,
        /// which unlinks it as it is created; on Windows, which deletes it as its last handle
        /// closes.
        pub(crate) const KEEPS_NAME: bool = files::internals::SCRATCH_KEEPS_NAME;
    }

    impl MappedFile {
        /// Whether [`Self::evict`] can drop a file's pages on this system: on Linux, not on macOS
        /// or Windows here. A bench of cold reads runs only where it can.
        pub(crate) const EVICTS: bool = os::internals::EVICTS;

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
        pub(crate) fn evict(&self) -> bool {
            let file = self
                ._handle
                .as_ref()
                .expect("a map opened to evict keeps its file's handle");
            os::internals::evict(&self.map, file)
        }

        /// The share of the file's pages in memory; `None` where the system does not say.
        #[cfg(test)]
        #[cfg_attr(
            not(target_os = "linux"),
            expect(
                clippy::missing_const_for_fn,
                reason = "const only where the system does not say and the call does nothing"
            )
        )]
        pub(crate) fn resident_share(&self) -> Option<f64> {
            os::internals::resident_share(&self.map)
        }
    }
}
