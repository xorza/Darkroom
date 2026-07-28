use std::sync::atomic::{AtomicU64, Ordering};

use common::CancelToken;
use hashbrown::HashSet;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::digest::{Digest, DigestHasher};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::plan::{ExecutionPlan, NodeVerdict};
use crate::execution::program::index::{NodeColumn, NodeIdx, NodeSet};
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput, ExecutionProgram,
};
use crate::execution::resource::{
    FileId, FsPathId, RunResourceStamps, Stamp, epoch_offset_ns, resolve_paths,
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

fn stamp(path: &str) -> std::io::Result<Stamp> {
    FsPathId::collect(path, &CancelToken::never())
}

fn fingerprint(path: &str) -> Digest {
    let Ok(Stamp::Known(identity)) = stamp(path) else {
        panic!("{path} has no determinate identity");
    };
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

        // A directory that will not list has no identity to stamp, and is
        // deliberately *not* handed one: a marker value would be perfectly
        // stable, so the node would go on reusing a cached result while
        // the contents it cannot see changed underneath it.
        let empty = fingerprint(&path);
        let permissions = |mode: u32| Permissions::from_mode(mode);
        std::fs::set_permissions(&dir.0, permissions(0o000)).unwrap();
        let unreadable = stamp(&path);
        let resolved = resolve_paths(HashSet::from([path.clone()]), &CancelToken::never());
        std::fs::set_permissions(&dir.0, permissions(0o755)).unwrap();

        assert!(
            unreadable.is_err(),
            "an unlistable directory must surface its error, not a stamp: {unreadable:?}",
        );
        // The consequence that matters: nothing lands in the map, so the
        // fold declines and `node_digest` comes out `None` — the node
        // recomputes rather than reusing a result keyed on a guess.
        assert!(resolved.is_empty(), "an unknown path is left unstamped");
        let stamps = RunResourceStamps { fs_paths: resolved };
        assert_eq!(
            stamps.hash_fs_paths(&mut DigestHasher::new(), std::slice::from_ref(&path)),
            None,
            "an unstamped path must refuse to produce a digest",
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

    // An empty directory is a real entry, not an absence.
    std::fs::create_dir(sub.join("deeper").join("empty")).unwrap();
    assert_ne!(fingerprint(&path), after_deep_add);
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

    std::fs::write(&first, b"same").unwrap();
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
    verdicts.reset(program.e_nodes.len(), NodeVerdict::Execute);
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
            verdicts,
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
    cache.stamp_digest(
        &fixture.program,
        &resource_stamps,
        fixture.program.e_node_index[&fixture.first],
    );

    std::fs::write(&file, b"longer").unwrap();
    cache.stamp_digest(
        &fixture.program,
        &resource_stamps,
        fixture.program.e_node_index[&fixture.second],
    );
    assert_eq!(
        cache.slots[fixture.program.e_node_index[&fixture.first]].current_digest,
        cache.slots[fixture.program.e_node_index[&fixture.second]].current_digest,
        "both consumers fold the run's one coherent resource identity"
    );

    let first_run = cache.slots[fixture.program.e_node_index[&fixture.first]].current_digest;
    resource_stamps
        .prepare_run(
            &fixture.program,
            &fixture.plan,
            &cache,
            CancelToken::never(),
        )
        .await;
    cache.stamp_digest(
        &fixture.program,
        &resource_stamps,
        fixture.program.e_node_index[&fixture.first],
    );
    assert_ne!(
        cache.slots[fixture.program.e_node_index[&fixture.first]].current_digest, first_run,
        "the next run refreshes resource identity"
    );
}
