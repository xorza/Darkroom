//! Windows: `MoveFileExW` replaces in one step, a rename needs no directory sync, and the file's
//! id and change time come from its handle, which `Metadata` does not expose.

#![expect(
    unsafe_code,
    reason = "`MoveFileExW` and `GetFileInformationByHandleEx` are the Win32 calls that do this"
)]

use std::ffi::c_void;
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::Path;

use windows::Win32::Foundation::{HANDLE, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS,
    FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::{Error, PCWSTR};

use crate::file_utils::PublicationMode;
use crate::file_utils::file_identity::FileId;
use crate::file_utils::platform::{Observed, Stated};

/// 100 ns ticks from 1601-01-01, the Windows file-time epoch, to the Unix epoch.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

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
