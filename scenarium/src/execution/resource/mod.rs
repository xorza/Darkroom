//! Per-run external-resource identity collection.
//!
//! Filesystem metadata walks and custom [`ResourceStamper`](crate::ResourceStamper)
//! calls run on Tokio's blocking pool. The resulting [`RunResourceStamps`] is shared by
//! the producer-first digest pass and late bound-resource restamps, so each resource is
//! observed once per run and digest folding itself performs no I/O.

use std::future::Future;
use std::sync::Arc;

use common::CancelToken;
use hashbrown::{HashMap, HashSet};

use crate::execution::cache::RuntimeCache;
use crate::execution::digest::DigestHasher;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{
    ExecutionBinding, ExecutionPortAddress, ExecutionProgram, InputStamper,
};
use crate::graph::NodeId;
use crate::node::definition::FuncBehavior;
use crate::{DynamicValue, ResourceStamp, ResourceStamper, StaticValue};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FileId {
    len: u64,
    mtime_ns: u128,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CustomResourceKey {
    stamper: usize,
    value: CustomValueKey,
}

impl CustomResourceKey {
    fn new(
        address: &ExecutionPortAddress,
        stamper: &Arc<dyn ResourceStamper>,
        value: &DynamicValue,
    ) -> Self {
        Self {
            stamper: Arc::as_ptr(stamper) as *const () as usize,
            value: match value {
                DynamicValue::Custom(value) => {
                    CustomValueKey::Custom(Arc::as_ptr(value) as *const () as usize)
                }
                _ => CustomValueKey::Source {
                    target: address.target,
                    port_idx: address.port_idx,
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CustomValueKey {
    Custom(usize),
    Source { target: NodeId, port_idx: usize },
}

#[derive(Debug)]
struct CustomRequest {
    stamper: Arc<dyn ResourceStamper>,
    value: DynamicValue,
}

#[derive(Debug, Default)]
struct ResourceStampRequests {
    fs_paths: HashSet<String>,
    custom: HashMap<CustomResourceKey, CustomRequest>,
}

impl ResourceStampRequests {
    fn is_empty(&self) -> bool {
        self.fs_paths.is_empty() && self.custom.is_empty()
    }

    fn collect_node(
        &mut self,
        stamps: &RunResourceStamps,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_id: NodeId,
    ) {
        let node = &program.e_nodes[&node_id];
        if node.behavior != FuncBehavior::Pure {
            return;
        }
        for input in program.node_inputs(node) {
            match &input.binding {
                ExecutionBinding::Const(StaticValue::FsPath(path)) => {
                    if !stamps.fs_paths.contains_key(path) {
                        self.fs_paths.insert(path.clone());
                    }
                }
                ExecutionBinding::Bind(address) => {
                    let Some(stamper) = &input.stamper else {
                        continue;
                    };
                    let Some(value) = cache.slots[&address.target]
                        .output_values()
                        .and_then(|values| values.get(address.port_idx))
                    else {
                        continue;
                    };
                    match stamper {
                        InputStamper::FsPath => {
                            if let Some(path) = value.as_fs_path()
                                && !stamps.fs_paths.contains_key(path)
                            {
                                self.fs_paths.insert(path.to_string());
                            }
                        }
                        InputStamper::Custom(stamper) => {
                            let key = CustomResourceKey::new(address, stamper, value);
                            if !stamps.custom.contains_key(&key) {
                                self.custom.entry(key).or_insert_with(|| CustomRequest {
                                    stamper: stamper.clone(),
                                    value: value.clone(),
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn resolve(self, cancel: &CancelToken) -> PreparedResourceStamps {
        let mut prepared = PreparedResourceStamps::default();
        for path in self.fs_paths {
            let Some(identity) = FsPathId::collect(&path, cancel) else {
                break;
            };
            prepared.fs_paths.insert(path, identity);
        }
        if cancel.is_cancelled() {
            return prepared;
        }
        for (key, request) in self.custom {
            let stamp = request.stamper.stamp(&request.value, cancel);
            if cancel.is_cancelled() {
                break;
            }
            prepared.custom.insert(key, stamp);
        }
        prepared
    }
}

#[derive(Debug, Default)]
struct PreparedResourceStamps {
    fs_paths: HashMap<String, FsPathId>,
    custom: HashMap<CustomResourceKey, ResourceStamp>,
}

#[derive(Debug, Default)]
pub(crate) struct RunResourceStamps {
    fs_paths: HashMap<String, FsPathId>,
    custom: HashMap<CustomResourceKey, ResourceStamp>,
}

impl RunResourceStamps {
    /// Request collection finishes before await so the non-`Sync` cache borrow stays synchronous.
    pub(crate) fn prepare_run<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &RuntimeCache,
        cancel: CancelToken,
    ) -> impl Future<Output = ()> + 'a {
        self.fs_paths.clear();
        self.custom.clear();
        let mut requests = ResourceStampRequests::default();
        for &node_id in &plan.process_order {
            if plan.verdicts[&node_id].wants_execute() {
                requests.collect_node(self, program, cache, node_id);
            }
        }
        self.prepare(requests, cancel)
    }

    pub(crate) fn prepare_node<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_id: NodeId,
        cancel: CancelToken,
    ) -> impl Future<Output = ()> + 'a {
        let requests = self.collect_node_requests(program, cache, node_id);
        self.prepare(requests, cancel)
    }

    fn collect_node_requests(
        &self,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_id: NodeId,
    ) -> ResourceStampRequests {
        let mut requests = ResourceStampRequests::default();
        requests.collect_node(self, program, cache, node_id);
        requests
    }

    async fn prepare(&mut self, requests: ResourceStampRequests, cancel: CancelToken) {
        if requests.is_empty() || cancel.is_cancelled() {
            return;
        }
        let worker_cancel = cancel.clone();
        let prepared = tokio::task::spawn_blocking(move || requests.resolve(&worker_cancel))
            .await
            .expect("resource stamping task panicked");
        if cancel.is_cancelled() {
            return;
        }
        self.fs_paths.extend(prepared.fs_paths);
        self.custom.extend(prepared.custom);
    }

    pub(crate) fn hash_fs_path(&self, hasher: &mut DigestHasher, path: &str) -> Option<()> {
        self.fs_paths.get(path)?.hash(hasher);
        Some(())
    }

    pub(crate) fn hash_custom(
        &self,
        hasher: &mut DigestHasher,
        address: &ExecutionPortAddress,
        stamper: &Arc<dyn ResourceStamper>,
        value: &DynamicValue,
    ) -> Option<()> {
        let stamp = self
            .custom
            .get(&CustomResourceKey::new(address, stamper, value))?;
        hasher.write_len_prefixed(stamp.as_bytes());
        Some(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use common::CancelToken;

    use crate::execution::cache::RuntimeCache;
    use crate::execution::program::ExecutionProgram;
    use crate::graph::NodeId;

    use crate::execution::resource::RunResourceStamps;

    pub(crate) fn prepare_node(
        stamps: &mut RunResourceStamps,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_id: NodeId,
    ) {
        let requests = stamps.collect_node_requests(program, cache, node_id);
        let prepared = requests.resolve(&CancelToken::never());
        stamps.fs_paths.extend(prepared.fs_paths);
        stamps.custom.extend(prepared.custom);
    }
}

#[cfg(test)]
mod tests;
