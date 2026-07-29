use std::sync::atomic::{AtomicU64, Ordering};

use common::CancelToken;

use crate::execution::cache::digest::{Digest, DigestHasher};
use crate::execution::cache::resource::{
    FileId, FsPathId, ResourceStamper, StampJob, epoch_offset_ns,
};
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::plan::{ExecutionPlan, NodeState};
use crate::execution::program::index::{NodeColumn, NodeIdx, NodeSet};
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput, Program,
};
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

fn fingerprint_with(job: &mut StampJob, path: &str) -> Digest {
    let Ok(identity) = job.stamp(path, &CancelToken::never()) else {
        panic!("{path} has no determinate identity");
    };
    let mut hasher = DigestHasher::new();
    identity.hash(&mut hasher);
    hasher.finish()
}

fn fingerprint(path: &str) -> Digest {
    fingerprint_with(&mut StampJob::default(), path)
}

#[test]
fn directory_identity_tracks_entry_changes() {
    let dir = TempDir::new("dir");
    let path = dir.0.to_string_lossy().into_owned();

    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;

        // A directory that will not list has no identity to stamp, and is
        // deliberately *not* handed one: a marker value would be perfectly
        // stable, so the node would go on reusing a cached result while
        // the contents it cannot see changed underneath it.
        let empty = fingerprint(&path);
        let permissions = |mode: u32| Permissions::from_mode(mode);
        std::fs::set_permissions(&dir.0, permissions(0o000)).unwrap();
        let unreadable = StampJob::default().stamp(&path, &CancelToken::never());
        // The whole pass fails with it, rather than dropping the path and
        // leaving the node silently uncached forever.
        let mut stamps = ResourceStamper::default();
        stamps.job.requests.insert(path.clone());
        stamps.job.requests.insert("never-queued-twice".to_string());
        let resolved = stamps.stamp_queued(&CancelToken::never());
        std::fs::set_permissions(&dir.0, permissions(0o755)).unwrap();

        assert!(
            unreadable.is_err(),
            "an unlistable directory must surface its error, not a stamp: {unreadable:?}",
        );
        assert!(
            resolved.is_err(),
            "one unstampable path must fail the pass: {resolved:?}",
        );
        assert!(stamps.fs_paths.is_empty(), "and stamp nothing");
        // The pass drains as it walks, so a failure part-way leaves nothing
        // queued behind it — which is why queueing a node's paths never has
        // to clear the queue first.
        assert!(
            stamps.job.requests.is_empty(),
            "a failed pass must still empty its queue, including paths it never reached",
        );
        assert_eq!(
            fingerprint(&path),
            empty,
            "and it stamps again once readable"
        );
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

/// A pure function handed a directory consumes it recursively, so its
/// identity has to be the whole subtree. Stamping one level deep let
/// everything below the first level change under a fingerprint that
/// never moved, and the node reused output built from the old contents.
#[test]
fn directory_identity_tracks_nested_changes() {
    let dir = TempDir::new("nested");
    let path = dir.0.to_string_lossy().into_owned();
    let sub = dir.0.join("sub");
    std::fs::create_dir_all(sub.join("deeper")).unwrap();
    std::fs::write(sub.join("file.bin"), b"one").unwrap();
    let base = fingerprint(&path);
    assert_eq!(fingerprint(&path), base, "a still tree stamps stably");

    // The case the one-level stamp missed: a nested edit that does not
    // touch any immediate child of the root.
    std::fs::write(sub.join("file.bin"), b"one-plus").unwrap();
    let after_nested_edit = fingerprint(&path);
    assert_ne!(after_nested_edit, base, "nested edit must move the root");

    // Depth is not special-cased — the deepest level counts too.
    std::fs::write(sub.join("deeper").join("leaf.bin"), b"x").unwrap();
    let after_deep_add = fingerprint(&path);
    assert_ne!(after_deep_add, after_nested_edit);

    // Only files are folded, so an empty directory is an absence: there
    // is nothing beneath it for a node to read, and nothing it can change
    // without a file changing with it.
    std::fs::create_dir(sub.join("deeper").join("empty")).unwrap();
    assert_eq!(
        fingerprint(&path),
        after_deep_add,
        "an empty directory is not part of the identity"
    );
    // …and it stops being an absence the moment it holds something.
    std::fs::write(sub.join("deeper").join("empty").join("c.bin"), b"c").unwrap();
    assert_ne!(fingerprint(&path), after_deep_add);
}

/// One stamper walks every path of every run in the same buffers, so a
/// walk must leave nothing of itself behind — a stale entry from the last
/// directory would fold into the next directory's identity.
#[test]
fn a_reused_stamper_stamps_like_a_fresh_one() {
    let dir = TempDir::new("reuse");
    let deep = dir.0.join("deep");
    let shallow = dir.0.join("shallow");
    std::fs::create_dir_all(deep.join("nested")).unwrap();
    std::fs::create_dir(&shallow).unwrap();
    std::fs::write(deep.join("nested").join("a.bin"), b"one").unwrap();
    std::fs::write(shallow.join("b.bin"), b"two").unwrap();
    let deep_path = deep.to_string_lossy().into_owned();
    let shallow_path = shallow.to_string_lossy().into_owned();

    let mut job = StampJob::default();
    let expected = fingerprint_with(&mut job, &shallow_path);
    // The buffer is genuinely retained — which is what makes a leak
    // between walks possible. `deep` holds one file, `nested/a.bin`; the
    // `nested` directory itself is not listed.
    fingerprint_with(&mut job, &deep_path);
    assert_eq!(job.files.len(), 1, "the walked files are retained");

    assert_eq!(
        fingerprint_with(&mut job, &shallow_path),
        expected,
        "a reused job must fold only the tree it was handed",
    );
    assert_eq!(
        fingerprint_with(&mut job, &shallow_path),
        fingerprint(&shallow_path),
        "and agree with a job that never walked anything else",
    );
}

/// Entry names are folded as raw bytes. `to_string_lossy` collapses
/// every non-UTF-8 name onto one replacement string, so two distinct
/// names would be interchangeable without moving the fingerprint —
/// a rename that a pure node's cache key could not see.
#[test]
#[cfg(unix)]
fn directory_identity_separates_non_utf8_names() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = TempDir::new("bytes");
    let path = dir.0.to_string_lossy().into_owned();
    // Both lossy-convert to the same U+FFFD replacement character.
    let first = dir.0.join(OsStr::from_bytes(b"\xff"));
    let second = dir.0.join(OsStr::from_bytes(b"\xfe"));

    // APFS refuses a name that is not valid UTF-8 (`EILSEQ`), so on macOS the
    // input this test is about cannot be created at all — leave it to the
    // filesystems that can express one rather than failing on every dev box.
    if std::fs::write(&first, b"same").is_err() {
        return;
    }
    let with_first = fingerprint(&path);
    std::fs::rename(&first, &second).unwrap();
    // Same length, and the rename preserves mtime, so the *name* is the
    // only thing that moved.
    assert_ne!(
        fingerprint(&path),
        with_first,
        "a rename between two non-UTF-8 names must move the fingerprint",
    );
}

/// `duration_since(UNIX_EPOCH).ok().unwrap_or(0)` mapped *every* mtime
/// before 1970 onto the same `0` as the epoch itself, so two files
/// differing only in when they were modified shared a pure node's cache
/// key. (The third state it folded into that same `0` — a metadata read
/// that simply failed — is no longer a value at all; an unstampable file
/// is left out of the map entirely.)
#[test]
fn file_identity_separates_pre_epoch_mtimes() {
    use std::time::{Duration, UNIX_EPOCH};

    // The conversion: signed, so before and after the epoch are ordered
    // rather than folded together. Hand-computed against the epoch.
    assert_eq!(epoch_offset_ns(UNIX_EPOCH), 0);
    assert_eq!(
        epoch_offset_ns(UNIX_EPOCH + Duration::from_secs(1)),
        1_000_000_000,
    );
    assert_eq!(
        epoch_offset_ns(UNIX_EPOCH - Duration::from_secs(1)),
        -1_000_000_000,
    );
    assert_eq!(
        epoch_offset_ns(UNIX_EPOCH - Duration::from_nanos(3)),
        -3,
        "sub-second resolution survives on the pre-epoch side too",
    );

    // …and that the identities built from them stay apart. Same length
    // throughout, so mtime is the only field in play.
    let digest_of = |mtime_ns| {
        let mut hasher = DigestHasher::new();
        FsPathId::File(FileId { len: 4, mtime_ns }).hash(&mut hasher);
        hasher.finish()
    };
    let all = [
        ("1s before epoch", digest_of(-1_000_000_000)),
        ("2s before epoch", digest_of(-2_000_000_000)),
        ("epoch", digest_of(0)),
        ("1s after epoch", digest_of(1_000_000_000)),
    ];
    for (i, (left_name, left)) in all.iter().enumerate() {
        for (right_name, right) in &all[i + 1..] {
            assert_ne!(left, right, "{left_name} must not alias {right_name}");
        }
    }
}

#[derive(Debug)]
struct ConstPathFixture {
    program: Program,
    plan: ExecutionPlan,
    first: ExecutionNodeId,
    second: ExecutionNodeId,
}

fn const_path_fixture(path: &str) -> ConstPathFixture {
    let first = ExecutionNodeId::from_u128(1);
    let second = ExecutionNodeId::from_u128(2);
    let mut program = Program::default();
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
        }]),
        program.outputs.append([ExecutionOutput {
            data_type: DataType::Int,
        }]),
    ];
    for ((e_node_id, inputs), outputs) in [first, second]
        .into_iter()
        .zip(input_ranges)
        .zip(output_ranges)
    {
        program.push(
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
    let mut verdicts = NodeColumn::default();
    verdicts.reset(program.e_nodes.len(), NodeState::Cut);
    let mut roots = NodeSet::default();
    roots.reset(program.e_nodes.len());
    roots.insert(NodeIdx(0));
    roots.insert(NodeIdx(1));
    let mut seeded = NodeSet::default();
    seeded.reset(program.e_nodes.len());
    let mut event_sources = NodeSet::default();
    event_sources.reset(program.e_nodes.len());
    ConstPathFixture {
        program,
        plan: ExecutionPlan {
            process_order: vec![NodeIdx(0), NodeIdx(1)],
            states: verdicts,
            roots,
            seeded,
            event_sources,
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
    cache.reconcile_fresh(&fixture.program);

    cache
        .prepare(
            &fixture.program,
            fixture.plan.executing(),
            CancelToken::never(),
        )
        .await;
    cache.stamp_digest(
        &fixture.program,
        fixture.program.e_node_index[&fixture.first],
    );

    std::fs::write(&file, b"longer").unwrap();
    cache.stamp_digest(
        &fixture.program,
        fixture.program.e_node_index[&fixture.second],
    );
    assert_eq!(
        cache.slots[fixture.program.e_node_index[&fixture.first]].current_digest,
        cache.slots[fixture.program.e_node_index[&fixture.second]].current_digest,
        "both consumers fold the run's one coherent resource identity"
    );

    let first_run = cache.slots[fixture.program.e_node_index[&fixture.first]].current_digest;
    cache
        .prepare(
            &fixture.program,
            fixture.plan.executing(),
            CancelToken::never(),
        )
        .await;
    cache.stamp_digest(
        &fixture.program,
        fixture.program.e_node_index[&fixture.first],
    );
    assert_ne!(
        cache.slots[fixture.program.e_node_index[&fixture.first]].current_digest, first_run,
        "the next run refreshes resource identity"
    );
}
