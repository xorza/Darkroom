//! Per-run filesystem identity collection.
//!
//! Filesystem metadata walks run on Tokio's blocking pool. One
//! [`ResourceStamper`] serves the producer-first digest pass and the late bound-path
//! restamps alike, so each path is observed once per run and digest folding itself
//! performs no I/O.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use common::CancelToken;
use hashbrown::{HashMap, HashSet};

use crate::StaticValue;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::digest::{Digest, DigestHasher};
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::index::NodeIdx;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::node::definition::FuncBehavior;

/// Metadata identity of one filesystem entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    len: u64,
    /// Nanoseconds from the Unix epoch, **negative before it**. Signed
    /// because `duration_since(..).ok().unwrap_or(0)` gave every pre-1970
    /// mtime the same `0` as the epoch itself.
    mtime_ns: i128,
}

/// Signed nanoseconds between `time` and the Unix epoch.
///
/// Split out from [`FileId::from_metadata`] because the pre-epoch arm is
/// the whole point and setting a real file's mtime to 1969 needs a
/// syscall this crate has no dependency for.
fn epoch_offset_ns(time: std::time::SystemTime) -> i128 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        // Pre-epoch: the error carries the distance the other way.
        Err(before) => -(before.duration().as_nanos() as i128),
    }
}

impl FileId {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Self {
            len: metadata.len(),
            mtime_ns,
        }
    }
}

#[derive(Debug)]
struct DirectoryEntryId {
    name: String,
    file: FileId,
}

#[derive(Debug)]
enum DirectoryId {
    Entries(Vec<DirectoryEntryId>),
    Unreadable,
}

#[derive(Debug)]
enum FsPathId {
    File(FileId),
    Directory(DirectoryId),
    Missing,
}

impl FsPathId {
    fn collect(path: &str, cancel: &CancelToken) -> Option<Self> {
        if cancel.is_cancelled() {
            return None;
        }
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => {
                Self::collect_directory(path, cancel).map(Self::Directory)
            }
            Ok(metadata) => Some(Self::File(FileId::from_metadata(&metadata))),
            Err(_) if cancel.is_cancelled() => None,
            Err(_) => Some(Self::Missing),
        }
    }

    fn collect_directory(path: &str, cancel: &CancelToken) -> Option<DirectoryId> {
        let read = match std::fs::read_dir(path) {
            Ok(read) => read,
            Err(_) if cancel.is_cancelled() => return None,
            Err(_) => return Some(DirectoryId::Unreadable),
        };
        let mut entries = Vec::new();
        for entry in read {
            if cancel.is_cancelled() {
                return None;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let file = entry
                .metadata()
                .map(|metadata| FileId::from_metadata(&metadata))
                .unwrap_or_default();
            entries.push(DirectoryEntryId { name, file });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Some(DirectoryId::Entries(entries))
    }

    fn hash(&self, hasher: &mut DigestHasher) {
        match self {
            Self::File(file) => {
                hasher
                    .write_bytes(&[0])
                    .write_pod(file.len)
                    .write_pod(file.mtime_ns);
            }
            Self::Directory(DirectoryId::Entries(entries)) => {
                hasher.write_bytes(&[1]).write_pod(entries.len() as u64);
                for entry in entries {
                    hasher
                        .write_str(&entry.name)
                        .write_pod(entry.file.len)
                        .write_pod(entry.file.mtime_ns);
                }
            }
            Self::Directory(DirectoryId::Unreadable) => {
                hasher.write_bytes(&[1]).write_pod(u64::MAX);
            }
            Self::Missing => {
                hasher.write_bytes(&[2]);
            }
        }
    }
}

fn resolve_paths(paths: HashSet<String>, cancel: &CancelToken) -> HashMap<String, FsPathId> {
    let mut resolved = HashMap::new();
    for path in paths {
        let Some(identity) = FsPathId::collect(&path, cancel) else {
            break;
        };
        resolved.insert(path, identity);
    }
    resolved
}

#[derive(Debug, Default)]
pub(crate) struct RunResourceStamps {
    fs_paths: HashMap<String, FsPathId>,
}

impl RunResourceStamps {
    fn collect_fs_paths(&self, requests: &mut HashSet<String>, paths: &[String]) {
        for path in paths {
            if !self.fs_paths.contains_key(path) {
                requests.insert(path.clone());
            }
        }
    }

    fn collect_node_paths(
        &self,
        requests: &mut HashSet<String>,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_idx: NodeIdx,
    ) {
        let e_node = &program[node_idx];
        if e_node.behavior != FuncBehavior::Pure {
            return;
        }
        for input in &program.inputs[e_node.inputs] {
            match &input.binding {
                ExecutionBinding::Const(StaticValue::FsPath(path)) => {
                    self.collect_fs_paths(requests, std::slice::from_ref(path));
                }
                ExecutionBinding::Const(StaticValue::FsPaths(paths)) => {
                    self.collect_fs_paths(requests, paths);
                }
                ExecutionBinding::Bind(address) if input.stamps_fs_path => {
                    let Some(value) = cache.slots[address.node_idx]
                        .output_values()
                        .and_then(|values| values.get(address.port_idx as usize))
                    else {
                        continue;
                    };
                    match value.as_static() {
                        Some(StaticValue::FsPath(path)) => {
                            self.collect_fs_paths(requests, std::slice::from_ref(path));
                        }
                        Some(StaticValue::FsPaths(paths)) => {
                            self.collect_fs_paths(requests, paths);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Request collection finishes before await so the non-`Sync` cache borrow stays synchronous.
    pub(crate) fn prepare_run<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &RuntimeCache,
        cancel: CancelToken,
    ) -> impl Future<Output = ()> + 'a {
        self.fs_paths.clear();
        let mut requests = HashSet::new();
        for &node_idx in &plan.process_order {
            if plan.verdicts[node_idx].wants_execute() {
                self.collect_node_paths(&mut requests, program, cache, node_idx);
            }
        }
        self.prepare(requests, cancel)
    }

    pub(crate) fn prepare_node<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_idx: NodeIdx,
        cancel: CancelToken,
    ) -> impl Future<Output = ()> + 'a {
        let mut requests = HashSet::new();
        self.collect_node_paths(&mut requests, program, cache, node_idx);
        self.prepare(requests, cancel)
    }

    async fn prepare(&mut self, requests: HashSet<String>, cancel: CancelToken) {
        if requests.is_empty() || cancel.is_cancelled() {
            return;
        }
        let worker_cancel = cancel.clone();
        let prepared = tokio::task::spawn_blocking(move || resolve_paths(requests, &worker_cancel))
            .await
            .expect("resource stamping task panicked");
        if cancel.is_cancelled() {
            return;
        }
        self.fs_paths.extend(prepared);
    }

    pub(crate) fn hash_fs_paths(&self, hasher: &mut DigestHasher, paths: &[String]) -> Option<()> {
        hasher.write_pod(paths.len() as u64);
        for path in paths {
            self.fs_paths.get(path)?.hash(hasher);
        }
        Some(())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;
    use hashbrown::HashSet;

    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::program::ExecutionProgram;
    use crate::execution::program::index::NodeIdx;

    use crate::execution::resource::{RunResourceStamps, resolve_paths};

    pub(crate) fn prepare_node(
        stamps: &mut RunResourceStamps,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_idx: NodeIdx,
    ) {
        let mut requests = HashSet::new();
        stamps.collect_node_paths(&mut requests, program, cache, node_idx);
        let prepared = resolve_paths(requests, &CancelToken::never());
        stamps.fs_paths.extend(prepared);
    }
}

#[cfg(test)]
mod tests;
