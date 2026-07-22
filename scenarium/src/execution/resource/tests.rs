use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use common::{CancelToken, Span};
use tokio::sync::Notify;

use crate::execution::cache::test_support::hydrate;
use crate::execution::cache::{OutputSnapshot, RuntimeCache};
use crate::execution::digest::{Digest, DigestHasher};
use crate::execution::identity::{ExecutionNodeId, ExecutionOutputPort};
use crate::execution::plan::{ExecutionPlan, NodeVerdict};
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionProgram, InputStamper,
};
use crate::execution::resource::{FsPathId, RunResourceStamps};
use crate::execution::{NodeMap, NodeSet};
use crate::node::definition::{FuncBehavior, FuncId};
use crate::{
    CustomValue, DataType, DynamicValue, ResourceStamp, ResourceStamper, StaticValue, TypeId,
};

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
    let mut program = ExecutionProgram {
        inputs: vec![
            ExecutionInput {
                binding: ExecutionBinding::Const(StaticValue::FsPath(path.to_string())),
                ..Default::default()
            },
            ExecutionInput {
                binding: ExecutionBinding::Const(StaticValue::FsPath(path.to_string())),
                ..Default::default()
            },
        ],
        output_types: vec![DataType::Int, DataType::Int],
        output_pinned: vec![false, false],
        ..Default::default()
    };
    for (e_node_id, input_start, output_start) in [(first, 0, 0), (second, 1, 1)] {
        program.e_nodes.insert(
            e_node_id,
            ExecutionNode {
                behavior: FuncBehavior::Pure,
                func_id: FuncId::from_u128(10),
                inputs: Span::new(input_start, 1),
                outputs: Span::new(output_start, 1),
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

#[derive(Debug)]
struct CountingStamper {
    calls: Arc<AtomicUsize>,
}

impl ResourceStamper for CountingStamper {
    fn stamp(&self, _value: &DynamicValue, _cancel: &CancelToken) -> ResourceStamp {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        ResourceStamp::from_bytes(&(call as u64).to_le_bytes())
    }
}

#[derive(Debug)]
struct BlockingStamper {
    calls: Arc<AtomicUsize>,
    started: Arc<Notify>,
}

impl ResourceStamper for BlockingStamper {
    fn stamp(&self, _value: &DynamicValue, cancel: &CancelToken) -> ResourceStamp {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        while !cancel.is_cancelled() {
            std::thread::yield_now();
        }
        ResourceStamp::from_bytes(&[1])
    }
}

#[derive(Debug)]
struct BoundResourceFixture {
    program: ExecutionProgram,
    plan: ExecutionPlan,
    cache: RuntimeCache,
    first_consumer: ExecutionNodeId,
}

#[derive(Debug)]
struct TestHandle;

impl fmt::Display for TestHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TestHandle")
    }
}

impl CustomValue for TestHandle {
    fn type_id(&self) -> TypeId {
        TypeId::from_u128(1)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

fn bound_resource_fixture(stamper: Arc<dyn ResourceStamper>) -> BoundResourceFixture {
    let producer = ExecutionNodeId::from_u128(1);
    let first_consumer = ExecutionNodeId::from_u128(2);
    let second_consumer = ExecutionNodeId::from_u128(3);
    let address = ExecutionOutputPort {
        e_node_id: producer,
        port_idx: 0,
    };
    let mut program = ExecutionProgram {
        inputs: vec![
            ExecutionInput {
                stamper: Some(InputStamper::Custom(stamper.clone())),
                binding: ExecutionBinding::Bind(address.clone()),
                ..Default::default()
            },
            ExecutionInput {
                stamper: Some(InputStamper::Custom(stamper)),
                binding: ExecutionBinding::Bind(address),
                ..Default::default()
            },
        ],
        output_types: vec![DataType::Int, DataType::Int, DataType::Int],
        output_pinned: vec![false, false, false],
        ..Default::default()
    };
    program.e_nodes.insert(
        producer,
        ExecutionNode {
            behavior: FuncBehavior::Pure,
            func_id: FuncId::from_u128(1),
            outputs: Span::new(0, 1),
            ..Default::default()
        },
    );
    for (e_node_id, input_start, output_start) in [(first_consumer, 0, 1), (second_consumer, 1, 2)]
    {
        program.e_nodes.insert(
            e_node_id,
            ExecutionNode {
                behavior: FuncBehavior::Pure,
                func_id: FuncId::from_u128(2),
                inputs: Span::new(input_start, 1),
                outputs: Span::new(output_start, 1),
                ..Default::default()
            },
        );
    }
    let verdicts = program
        .e_nodes
        .keys()
        .copied()
        .map(|e_node_id| (e_node_id, NodeVerdict::Execute))
        .collect();
    let plan = ExecutionPlan {
        process_order: vec![producer, first_consumer, second_consumer],
        verdicts,
        roots: [first_consumer, second_consumer].into_iter().collect(),
        pinned: NodeSet::new(),
    };
    let mut cache = RuntimeCache::default();
    cache.reconcile(&program);
    let producer_digest = Digest([7; 32]);
    cache.slots.get_mut(&producer).unwrap().current_digest = Some(producer_digest);
    hydrate(
        &mut cache,
        producer,
        OutputSnapshot::new(vec![DynamicValue::from_custom(TestHandle)]),
        producer_digest,
    );
    BoundResourceFixture {
        program,
        plan,
        cache,
        first_consumer,
    }
}

#[tokio::test]
async fn custom_resource_is_stamped_once_per_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let fixture = bound_resource_fixture(Arc::new(CountingStamper {
        calls: calls.clone(),
    }));
    let mut resource_stamps = RunResourceStamps::default();

    resource_stamps
        .prepare_run(
            &fixture.program,
            &fixture.plan,
            &fixture.cache,
            CancelToken::never(),
        )
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    resource_stamps
        .prepare_node(
            &fixture.program,
            &fixture.cache,
            fixture.first_consumer,
            CancelToken::never(),
        )
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    resource_stamps
        .prepare_run(
            &fixture.program,
            &fixture.plan,
            &fixture.cache,
            CancelToken::never(),
        )
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_stamper_yields_runtime_progress_and_observes_cancellation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let fixture = bound_resource_fixture(Arc::new(BlockingStamper {
        calls: calls.clone(),
        started: started.clone(),
    }));
    let cancel = CancelToken::new();
    let progressed = Arc::new(AtomicBool::new(false));
    let mut resource_stamps = RunResourceStamps::default();

    let heartbeat = {
        let cancel = cancel.clone();
        let progressed = progressed.clone();
        async move {
            started.notified().await;
            progressed.store(true, Ordering::SeqCst);
            cancel.cancel();
        }
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            resource_stamps.prepare_run(
                &fixture.program,
                &fixture.plan,
                &fixture.cache,
                cancel.clone()
            ),
            heartbeat
        );
    })
    .await
    .expect("blocking resource stamping must not stall the async runtime");

    assert!(progressed.load(Ordering::SeqCst));
    assert!(cancel.is_cancelled());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        resource_stamps.custom.is_empty(),
        "a stamp completed after cancellation is discarded"
    );
}
