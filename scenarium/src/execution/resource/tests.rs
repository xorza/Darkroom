use std::sync::atomic::{AtomicU64, Ordering};

use common::CancelToken;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::digest::{Digest, DigestHasher};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::plan::{ExecutionPlan, NodeVerdict};
use crate::execution::program::index::{NodeMap, NodeSet};
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput, ExecutionProgram,
};
use crate::execution::resource::{FsPathId, RunResourceStamps};
use crate::node::definition::{FuncBehavior, FuncId};
use crate::{DataType, StaticValue};

#[derive(Debug)]
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "scenarium-resource-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn fingerprint(path: &str) -> Digest {
    let identity = FsPathId::collect(path, &CancelToken::never()).unwrap();
    let mut hasher = DigestHasher::new();
    identity.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn directory_identity_tracks_entry_changes() {
    let dir = TempDir::new("dir");
    let path = dir.0.to_string_lossy().into_owned();

    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;

        let empty = fingerprint(&path);
        let permissions = |mode: u32| Permissions::from_mode(mode);
        std::fs::set_permissions(&dir.0, permissions(0o000)).unwrap();
        let unreadable = fingerprint(&path);
        std::fs::set_permissions(&dir.0, permissions(0o755)).unwrap();
        assert_ne!(unreadable, empty);
        assert_eq!(fingerprint(&path), empty);
    }

    std::fs::write(dir.0.join("a.fits"), b"one").unwrap();
    let base = fingerprint(&path);
    assert_eq!(fingerprint(&path), base);

    std::fs::write(dir.0.join("b.fits"), b"two").unwrap();
    let after_add = fingerprint(&path);
    assert_ne!(after_add, base);

    std::fs::write(dir.0.join("a.fits"), b"one-plus-more").unwrap();
    let after_edit = fingerprint(&path);
    assert_ne!(after_edit, after_add);

    std::fs::remove_file(dir.0.join("b.fits")).unwrap();
    assert_ne!(fingerprint(&path), after_edit);
}

#[derive(Debug)]
struct ConstPathFixture {
    program: ExecutionProgram,
    plan: ExecutionPlan,
    first: ExecutionNodeId,
    second: ExecutionNodeId,
}

fn const_path_fixture(path: &str) -> ConstPathFixture {
    let first = ExecutionNodeId::from_u128(1);
    let second = ExecutionNodeId::from_u128(2);
    let mut program = ExecutionProgram::default();
    let input_ranges = [
        program.inputs.append([ExecutionInput {
            binding: ExecutionBinding::Const(StaticValue::FsPath(path.to_string())),
            ..Default::default()
        }]),
        program.inputs.append([ExecutionInput {
            binding: ExecutionBinding::Const(StaticValue::FsPath(path.to_string())),
            ..Default::default()
        }]),
    ];
    let output_ranges = [
        program.outputs.append([ExecutionOutput {
            data_type: DataType::Int,
            ..Default::default()
        }]),
        program.outputs.append([ExecutionOutput {
            data_type: DataType::Int,
            ..Default::default()
        }]),
    ];
    for ((e_node_id, inputs), outputs) in [first, second]
        .into_iter()
        .zip(input_ranges)
        .zip(output_ranges)
    {
        program.e_nodes.insert(
            e_node_id,
            ExecutionNode {
                behavior: FuncBehavior::Pure,
                func_id: FuncId::from_u128(10),
                inputs,
                outputs,
                ..Default::default()
            },
        );
    }
    let verdicts: NodeMap<NodeVerdict> = [first, second]
        .into_iter()
        .map(|e_node_id| (e_node_id, NodeVerdict::Execute))
        .collect();
    ConstPathFixture {
        program,
        plan: ExecutionPlan {
            process_order: vec![first, second],
            verdicts,
            roots: [first, second].into_iter().collect(),
            pinned: NodeSet::new(),
            event_sources: NodeSet::new(),
        },
        first,
        second,
    }
}

#[tokio::test]
async fn same_path_uses_one_identity_until_the_next_run() {
    let dir = TempDir::new("snapshot");
    let file = dir.0.join("data.bin");
    std::fs::write(&file, b"x").unwrap();
    let fixture = const_path_fixture(&file.to_string_lossy());
    let mut cache = RuntimeCache::default();
    cache.reconcile(&fixture.program);
    let mut resource_stamps = RunResourceStamps::default();

    resource_stamps
        .prepare_run(
            &fixture.program,
            &fixture.plan,
            &cache,
            CancelToken::never(),
        )
        .await;
    cache.stamp_digest(&fixture.program, &resource_stamps, fixture.first);

    std::fs::write(&file, b"longer").unwrap();
    cache.stamp_digest(&fixture.program, &resource_stamps, fixture.second);
    assert_eq!(
        cache.slots[&fixture.first].current_digest, cache.slots[&fixture.second].current_digest,
        "both consumers fold the run's one coherent resource identity"
    );

    let first_run = cache.slots[&fixture.first].current_digest;
    resource_stamps
        .prepare_run(
            &fixture.program,
            &fixture.plan,
            &cache,
            CancelToken::never(),
        )
        .await;
    cache.stamp_digest(&fixture.program, &resource_stamps, fixture.first);
    assert_ne!(
        cache.slots[&fixture.first].current_digest, first_run,
        "the next run refreshes resource identity"
    );
}
