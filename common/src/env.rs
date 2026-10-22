//! The process environment: the one route to it outside `common`, as `clippy.toml` holds.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

pub fn var_os(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The system's directory for temporary files, as `std::env::temp_dir`.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

/// The path of this process's executable, as `std::env::current_exe`.
pub fn current_exe() -> io::Result<PathBuf> {
    std::env::current_exe()
}
