//! Windows: a scratch file deleted when its last handle closes, the free space of a volume, the
//! memory limits of the job object the process runs in, and the cache under `%LOCALAPPDATA%`;
//! a replace in one step that holds when the destination is open and needs no directory sync, and
//! the file's id and change time, which come from its handle, as `Metadata` does not expose them.

use std::ffi::c_void;
use std::fs::{File, Metadata, OpenOptions};
use std::mem::offset_of;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};
use std::{env, io};

use memmap2::Mmap;
use sysinfo::System;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_WRITE_THROUGH, FILE_ID_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_RENAME_INFO, FileBasicInfo, FileIdInfo, FileRenameInfoEx,
    GetDiskFreeSpaceExW, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, SetFileInformationByHandle,
};
use windows::Win32::System::JobObjects::{
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject,
};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::{Error, HSTRING};

use crate::fs::PublicationMode;
use crate::fs::file_identity::FileId;
use crate::platform::{Observed, Stated};

/// A new file at `path`, opened `FILE_FLAG_DELETE_ON_CLOSE` with every share mode, so the map can
/// open the file beside the handle; the values are the Win32 headers'.
pub(super) fn create_deleted(path: &Path) -> io::Result<File> {
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    const FILE_SHARE_READ_WRITE_DELETE: u32 = 0x1 | 0x2 | 0x4;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .share_mode(FILE_SHARE_READ_WRITE_DELETE)
        .open(path)
}

/// The handle itself: Windows deletes the file when its last handle closes, and the map must not
/// outlive it.
pub(super) const SCRATCH_MAP_NEEDS_HANDLE: bool = true;

/// Nothing: Windows takes no advice on a view's access pattern.
pub(super) const fn advise_sequential(_map: &Mmap) {}

/// The bytes free to this user on the volume `path` lies on, with its disk quota counted.
pub(super) fn free_space(path: &Path) -> Option<u64> {
    let mut available = 0;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(path), Some(&raw mut available), None, None) }
        .ok()?;
    Some(available)
}

/// The host's available memory, or less where the job object this process runs in caps the
/// memory a process of it, or the whole job, may commit, less what is committed against the cap
/// now. A cap alone would overstate what a process that already holds memory can allocate.
pub(super) fn available_memory(system: &System) -> u64 {
    let Some(limits) = job_limits() else {
        return system.available_memory();
    };
    let mut available = system.available_memory();
    let flags = limits.BasicLimitInformation.LimitFlags;
    // Where the commit now cannot be read, the job's peak stands in: never less than it, and for
    // a process cap the peak of any process of the job, this one among them.
    if flags.contains(JOB_OBJECT_LIMIT_PROCESS_MEMORY) {
        let committed = committed_by(unsafe { GetCurrentProcess() })
            .unwrap_or(limits.PeakProcessMemoryUsed as u64);
        available = available.min((limits.ProcessMemoryLimit as u64).saturating_sub(committed));
    }
    if flags.contains(JOB_OBJECT_LIMIT_JOB_MEMORY) {
        // No query reports a job's commit now, only its peak; the processes' own sum is it.
        let committed = job_committed().unwrap_or(limits.PeakJobMemoryUsed as u64);
        available = available.min((limits.JobMemoryLimit as u64).saturating_sub(committed));
    }
    available
}

/// The limits of the job this process runs in; `None` outside any job.
fn job_limits() -> Option<JOBOBJECT_EXTENDED_LIMIT_INFORMATION> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    unsafe {
        QueryInformationJobObject(
            None,
            JobObjectExtendedLimitInformation,
            (&raw mut limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            None,
        )
    }
    .ok()?;
    Some(limits)
}

/// The memory `process` has committed: its private bytes.
fn committed_by(process: HANDLE) -> Option<u64> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    unsafe {
        GetProcessMemoryInfo(
            process,
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    }
    .ok()?;
    Some(counters.PrivateUsage as u64)
}

/// The memory every process of this process's job has committed; `None` when a process cannot be
/// read.
fn job_committed() -> Option<u64> {
    // The list's header is two `u32`s, one `usize` here, and its ids follow; a list sized for
    // one id reports how many the job holds.
    let query = |buffer: &mut Vec<usize>| unsafe {
        QueryInformationJobObject(
            None,
            JobObjectBasicProcessIdList,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * size_of::<usize>()) as u32,
            None,
        )
    };
    let mut buffer = vec![0usize; 2];
    query(&mut buffer).ok();
    let header = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
    let assigned = unsafe { (*header).NumberOfAssignedProcesses } as usize;
    buffer = vec![0usize; 1 + assigned.max(1)];
    query(&mut buffer).ok()?;
    let header = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
    let listed = unsafe { (*header).NumberOfProcessIdsInList } as usize;
    buffer[1..=listed.min(buffer.len() - 1)]
        .iter()
        .map(|&id| {
            let process =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id as u32) }.ok()?;
            let committed = committed_by(process);
            unsafe { CloseHandle(process) }.ok();
            committed
        })
        .sum()
}

/// `%LOCALAPPDATA%`, else the temporary directory.
pub(super) fn user_cache_root(var: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    var("LOCALAPPDATA").unwrap_or_else(env::temp_dir)
}

/// None to read: Windows lists no memory-backed file system, and a RAM disk is a volume like any
/// other.
pub(super) const fn memory_backed(_path: &Path) -> Option<String> {
    None
}

/// 100 ns ticks from 1601-01-01, the Windows file-time epoch, to the Unix epoch.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

/// A file link: Windows tells a link to a file from one to a directory when it makes it.
pub(super) fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(original, link)
}

pub(super) fn symlink_dir(original: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(original, link)
}

/// `MoveFileExW`, then, where it refuses because a handle holds the destination open — another
/// writer checking it, a reader — the rename with POSIX semantics, which replaces a file that
/// still has handles open. `std::fs::rename` takes the two in this order: every file system takes
/// `MoveFileExW`, where only some take the second.
pub(super) fn replace(source: &Path, destination: &Path, mode: PublicationMode) -> io::Result<()> {
    let destination = HSTRING::from(destination);
    let mut flags = MOVEFILE_REPLACE_EXISTING;
    if mode == PublicationMode::Durable {
        flags |= MOVEFILE_WRITE_THROUGH;
    }
    let Err(error) = (unsafe { MoveFileExW(&HSTRING::from(source), &destination, flags) }) else {
        return Ok(());
    };
    if WIN32_ERROR::from_error(&error) != Some(ERROR_ACCESS_DENIED) {
        return Err(io_error(error));
    }
    rename_posix(source, &destination, mode).map_err(|_| io_error(error))
}

/// `source` renamed to `destination` through `FileRenameInfoEx`; the flags' values are the Win32
/// headers'. `MOVEFILE_WRITE_THROUGH` opens its source handle write-through and renames through
/// it, as ReactOS's `MoveFileWithProgressW` shows, so a durable rename opens its handle the same
/// way.
fn rename_posix(source: &Path, destination: &HSTRING, mode: PublicationMode) -> io::Result<()> {
    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x1;
    const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 0x2;
    let mut options = OpenOptions::new();
    options.access_mode(DELETE.0);
    if mode == PublicationMode::Durable {
        options.custom_flags(FILE_FLAG_WRITE_THROUGH.0);
    }
    let file = options.open(source)?;

    // The name follows the struct in one buffer, and the buffer's zeros end it; a `u64` buffer
    // holds the struct's alignment.
    let name_len = size_of_val::<[u16]>(destination);
    let size = offset_of!(FILE_RENAME_INFO, FileName) + name_len + size_of::<u16>();
    let mut buffer = vec![0u64; size.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.Flags =
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS;
        (*info).FileNameLength = u32::try_from(name_len).expect("a path's length fits u32");
        (&raw mut (*info).FileName)
            .cast::<u16>()
            .copy_from_nonoverlapping(destination.as_ptr(), destination.len());
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfoEx,
            info.cast::<c_void>(),
            u32::try_from(size).expect("a path's length fits u32"),
        )
    }
    .map_err(io_error)
}

/// Nothing: a durable rename goes through a write-through handle, which returns once the rename is
/// on disk.
#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature on every system: Unix syncs the directory, which can fail"
)]
pub(super) const fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

/// Nothing: the id and the change time are the handle's to state, not `Metadata`'s.
pub(super) fn stated(_metadata: &Metadata) -> Stated {
    Stated::default()
}

/// The file opened for its attributes alone, so a file another process holds open for writing
/// still answers.
pub(super) fn observe(path: &Path) -> io::Result<Observed> {
    let file = OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    let metadata = file.metadata()?;
    let file_id = query::<FILE_ID_INFO>(&file, FileIdInfo).map(|info| FileId {
        volume: info.VolumeSerialNumber,
        file: u128::from_le_bytes(info.FileId.Identifier),
    });
    let changed_ns = query::<FILE_BASIC_INFO>(&file, FileBasicInfo)
        .map(|info| i128::from(info.ChangeTime - UNIX_EPOCH_TICKS) * 100);
    Ok(Observed {
        metadata,
        stated: Stated {
            file_id,
            changed_ns,
        },
    })
}

/// The `class` of information about `file`, or `None` when its file system does not answer it.
fn query<T: Default>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> Option<T> {
    let mut info = T::default();
    let answered = unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            class,
            (&raw mut info).cast::<c_void>(),
            u32::try_from(size_of::<T>()).expect("a file information struct's size fits u32"),
        )
    };
    answered.is_ok().then_some(info)
}

/// `error` as the `io::Error` of its Win32 code, which `io::ErrorKind` reads: the crate's own
/// conversion keeps the HRESULT, which it does not.
fn io_error(error: Error) -> io::Error {
    match WIN32_ERROR::from_error(&error) {
        Some(code) => io::Error::from_raw_os_error(raw_os_error(code)),
        None => error.into(),
    }
}

/// `code` as `io::Error` carries a raw OS error.
fn raw_os_error(code: WIN32_ERROR) -> i32 {
    i32::try_from(code.0).expect("a Win32 error code is 16 bits")
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::env;
    use std::ffi::OsString;
    use std::fs::{self, File, Permissions};
    use std::os::windows::ffi::OsStringExt as _;
    use std::path::{Path, PathBuf};

    use memmap2::Mmap;
    use windows::Win32::Foundation::ERROR_PRIVILEGE_NOT_HELD;

    use crate::platform::windows::{raw_os_error, symlink};

    /// This system offers no call that drops a file's pages from its cache here.
    pub(crate) const EVICTS: bool = false;

    /// Nothing: see [`EVICTS`].
    pub(crate) const fn evict(_map: &Mmap, _file: &File) -> bool {
        false
    }

    /// `None`: not read on this system.
    pub(crate) const fn resident_share(_map: &Mmap) -> Option<f64> {
        None
    }

    /// The system's temporary directory, which is on disk: Windows lists no memory-backed file
    /// system.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the API is the same on every system, and Linux can find no directory on disk"
    )]
    pub(crate) fn disk_temp_dir() -> Option<PathBuf> {
        Some(env::temp_dir())
    }

    /// Yes: a scratch file is deleted only when its last handle closes.
    pub(crate) const SCRATCH_KEEPS_NAME: bool = true;

    /// `None`: Windows has no mode bits. A path unreadable here takes an ACL that denies the
    /// user, and an open that asks for no access still reads the attributes of a file whose
    /// reading is denied, as `CreateFileW`'s docs state, so a Unix fixture's other effects would
    /// not follow.
    pub(crate) const fn take_permissions(_path: &Path) -> Option<Permissions> {
        None
    }

    /// Whether a link made in `dir` succeeds: Windows refuses one outside Developer Mode and an
    /// elevated process. A link to a missing file, removed at once, tells.
    pub(crate) fn symlinks_allowed(dir: &Path) -> bool {
        let link = dir.join("symlink-probe");
        match symlink(&dir.join("symlink-probe-target"), &link) {
            Ok(()) => {
                fs::remove_file(&link).expect("remove the symlink probe");
                true
            }
            Err(error) if error.raw_os_error() == Some(raw_os_error(ERROR_PRIVILEGE_NOT_HELD)) => {
                false
            }
            Err(error) => panic!("make a symlink probe in {}: {error}", dir.display()),
        }
    }

    /// One unpaired UTF-16 surrogate, `0xDC00 + idx`.
    pub(crate) fn non_unicode_name(idx: u8) -> OsString {
        OsString::from_wide(&[0xDC00 + u16::from(idx)])
    }

    /// No: the id and the change time come from the file's handle, not its `Metadata`.
    #[cfg(test)]
    pub(crate) const METADATA_HOLDS_STATED: bool = false;

    /// `None`: `Permissions` is the read-only flag alone here, and a publication refuses a
    /// read-only destination, so a writable file holds no permissions but the default.
    #[cfg(test)]
    pub(crate) const fn set_uncommon_permissions(_path: &Path) -> Option<Permissions> {
        None
    }
}
