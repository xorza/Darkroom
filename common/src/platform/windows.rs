//! Windows: a scratch file deleted when its last handle closes, the free space of a volume, the
//! memory limits of the job object the process runs in, and the cache under `%LOCALAPPDATA%`;
//! `MoveFileExW`, which replaces in one step and needs no directory sync, and the file's id and
//! change time, which come from its handle, as `Metadata` does not expose them.

use std::ffi::c_void;
use std::fs::{File, Metadata, OpenOptions};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};
use std::{env, io};

use memmap2::Mmap;
use sysinfo::System;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS,
    FileBasicInfo, FileIdInfo, GetDiskFreeSpaceExW, GetFileInformationByHandleEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
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
use windows::core::{Error, HSTRING, PCWSTR};

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

pub(super) fn replace(source: &Path, destination: &Path, mode: PublicationMode) -> io::Result<()> {
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (source, destination) = (wide(source), wide(destination));
    let mut flags = MOVEFILE_REPLACE_EXISTING;
    if mode == PublicationMode::Durable {
        flags |= MOVEFILE_WRITE_THROUGH;
    }
    unsafe { MoveFileExW(PCWSTR(source.as_ptr()), PCWSTR(destination.as_ptr()), flags) }
        .map_err(io_error)
}

/// Nothing: `MOVEFILE_WRITE_THROUGH` already returns once the rename is on disk.
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
        Some(code) => io::Error::from_raw_os_error(
            i32::try_from(code.0).expect("a Win32 error code is 16 bits"),
        ),
        None => error.into(),
    }
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
    pub(crate) const fn resident_share(_map: &Mmap) -> Option<f64> {
        None
    }

    /// Yes: a scratch file is deleted only when its last handle closes.
    pub(crate) const SCRATCH_KEEPS_NAME: bool = true;
}
