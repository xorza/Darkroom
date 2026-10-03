//! The directory a run spills frames into, and the one rule for removing it.
//!
//! Lumos removes a directory only when that directory carries [`MARKER`], which lumos writes into
//! every directory it creates. A caller can point `IngestConfig::cache_dir` anywhere — a shared
//! cache root, the folder the lights live in — and the worst a run can do there is leave its own
//! subdirectory behind.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::frame_store::error::FrameStoreError;

/// The file that marks a directory as created by lumos.
const MARKER: &str = ".lumos-spill";
/// Prefix of a run's own directory: `run-<pid>-<n>`.
const RUN_PREFIX: &str = "run-";
/// The directory a `keep_cache` run writes into and a later run reuses.
const PERSISTENT: &str = "lumos-cache";

/// Owns the directory one run spills into.
///
/// Without `keep_cache` the directory is the run's own, `root/run-<pid>-<n>`, and is removed on
/// drop. With `keep_cache` it is `root/lumos-cache`, the same for every run so that a later run can
/// reuse the planes, and is never removed by lumos.
#[derive(Debug)]
pub(crate) struct SpillDirectory {
    path: PathBuf,
    remove_on_drop: bool,
}

impl SpillDirectory {
    pub(crate) fn create(root: &Path, keep: bool) -> Result<Self, FrameStoreError> {
        fs::create_dir_all(root).map_err(create_error(root))?;
        if keep {
            let path = root.join(PERSISTENT);
            fs::create_dir_all(&path).map_err(create_error(&path))?;
            write_marker(&path).map_err(create_error(&path))?;
            return Ok(Self {
                path,
                remove_on_drop: false,
            });
        }
        remove_stale_runs(root);
        let path = create_run_directory(root).map_err(create_error(root))?;
        Ok(Self {
            path,
            remove_on_drop: true,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SpillDirectory {
    fn drop(&mut self) {
        if self.remove_on_drop {
            remove_marked(&self.path);
        }
    }
}

/// Create `root/run-<pid>-<n>` with its marker. `create_dir`, not `create_dir_all`: a name that is
/// already taken belongs to someone else, so the counter moves on rather than adopting it.
fn create_run_directory(root: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!("{RUN_PREFIX}{}-{id}", process::id()));
        match fs::create_dir(&path) {
            Ok(()) => {
                write_marker(&path)?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

fn create_error(path: &Path) -> impl FnOnce(io::Error) -> FrameStoreError {
    let path = path.to_path_buf();
    move |source| FrameStoreError::CreateDirectory { path, source }
}

fn write_marker(directory: &Path) -> io::Result<()> {
    fs::write(
        directory.join(MARKER),
        b"created by lumos; removed with its directory\n",
    )
}

/// Remove `directory` if, and only if, lumos created it.
fn remove_marked(directory: &Path) {
    if !directory.join(MARKER).is_file() {
        return;
    }
    if let Err(error) = fs::remove_dir_all(directory) {
        tracing::warn!(path = %directory.display(), %error, "failed to remove spill directory");
    }
}

/// Remove run directories left by processes that ended without dropping their `SpillDirectory`
/// (a kill, a crash).
///
/// Only marked `run-<pid>-<n>` directories whose process no longer exists qualify. A reused pid
/// keeps its directory until that process ends too — the safe direction to be wrong in.
fn remove_stale_runs(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let own_pid = process::id();
    let mut system = System::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(RUN_PREFIX))
            .and_then(|rest| rest.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        let pid = Pid::from_u32(pid);
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        if system.process(pid).is_none() {
            remove_marked(&entry.path());
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::frame_store::spill_directory::{MARKER, RUN_PREFIX, write_marker};

    /// A marked run directory as a dead process would leave it. Pid `u32::MAX` is never a live
    /// process: Linux caps pids at 2²², and the other platforms well below `u32::MAX` too.
    pub(crate) fn stale_run_directory(root: &Path) -> PathBuf {
        let path = root.join(format!("{RUN_PREFIX}{}-0", u32::MAX));
        fs::create_dir_all(&path).unwrap();
        write_marker(&path).unwrap();
        path
    }

    pub(crate) const fn marker() -> &'static str {
        MARKER
    }
}
