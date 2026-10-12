//! Windows: a scratch file deleted when its last handle closes, the free space of a volume, the
//! memory limits of the job object the process runs in, and the cache under `%LOCALAPPDATA%`.

use std::env;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use sysinfo::System;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
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
use windows::core::HSTRING;

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
    // SAFETY: the name is a null-terminated wide string that outlives the call, and the one
    // out-pointer is a local.
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
    if flags.contains(JOB_OBJECT_LIMIT_PROCESS_MEMORY)
        && let Some(committed) = committed_by(unsafe { GetCurrentProcess() })
    {
        available = available.min((limits.ProcessMemoryLimit as u64).saturating_sub(committed));
    }
    if flags.contains(JOB_OBJECT_LIMIT_JOB_MEMORY) {
        // No query reports a job's commit now, only its peak; the processes' own sum is it, and
        // the peak, never less than it, stands in when a process of the job cannot be read.
        let committed = job_committed().unwrap_or(limits.PeakJobMemoryUsed as u64);
        available = available.min((limits.JobMemoryLimit as u64).saturating_sub(committed));
    }
    available
}

/// The limits of the job this process runs in; `None` outside any job.
fn job_limits() -> Option<JOBOBJECT_EXTENDED_LIMIT_INFORMATION> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY: the buffer is the structure the class names, at its own size; a null job handle is
    // the calling process's job.
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
    // SAFETY: the extended counters begin with the plain ones, and `cb` says which were passed.
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
    let query = |buffer: &mut Vec<usize>| {
        // SAFETY: `buffer` holds the header and `len − 1` ids, as the length passed says.
        unsafe {
            QueryInformationJobObject(
                None,
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * size_of::<usize>()) as u32,
                None,
            )
        }
    };
    let mut buffer = vec![0usize; 2];
    let _ = query(&mut buffer);
    let header = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
    // SAFETY: the buffer holds a whole header.
    let assigned = unsafe { (*header).NumberOfAssignedProcesses } as usize;
    buffer = vec![0usize; 1 + assigned.max(1)];
    query(&mut buffer).ok()?;
    let header = buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
    // SAFETY: as above, and the list's count is what the query wrote.
    let listed = unsafe { (*header).NumberOfProcessIdsInList } as usize;
    buffer[1..=listed.min(buffer.len() - 1)]
        .iter()
        .map(|&id| {
            // SAFETY: a handle opened here is closed here.
            let process =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id as u32) }.ok()?;
            let committed = committed_by(process);
            // SAFETY: as above.
            let _ = unsafe { CloseHandle(process) };
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
    #[cfg(test)]
    pub(crate) const fn resident_share(_map: &Mmap) -> Option<f64> {
        None
    }
}
