#[cfg(unix)]
use common::Unreadable;
use common::{CancelToken, TempDir};

use crate::execution::cache::digest::{Digest, DigestHasher};
use crate::execution::cache::resource::error::StampError;
use crate::execution::cache::resource::{FsPathId, StampJob};
use crate::execution::cache::runtime::RuntimeCache;
use crate::graph::identity::FuncId;
use crate::testing::program::ProgramBuilder;
use crate::{ConstValue, DataType};
use std::fs;
use std::io;
use std::slice;

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
    let path = dir.path().to_string_lossy().into_owned();

    // A directory that will not list has no identity to stamp, and is
    // deliberately *not* handed one: a marker value would be perfectly stable,
    // so the node would go on reusing a cached result while the contents it
    // cannot see changed underneath it. Skipped where the process reads
    // through mode 000.
    #[cfg(unix)]
    {
        let empty = fingerprint(&path);
        if let Some(locked) = Unreadable::new(dir.path()) {
            let unreadable = StampJob::default().stamp(&path, &CancelToken::never());
            drop(locked);
            assert!(
                matches!(&unreadable, Err(StampError::Io { path, .. }) if path == dir.path()),
                "an unlistable directory surfaces its error, naming it: {unreadable:?}",
            );
            assert_eq!(
                fingerprint(&path),
                empty,
                "and it stamps again once readable"
            );
        }
    }

    fs::write(dir.join("a.fits"), b"one").unwrap();
    let base = fingerprint(&path);
    assert_eq!(fingerprint(&path), base);

    fs::write(dir.join("b.fits"), b"two").unwrap();
    let after_add = fingerprint(&path);
    assert_ne!(after_add, base);

    fs::write(dir.join("a.fits"), b"one-plus-more").unwrap();
    let after_edit = fingerprint(&path);
    assert_ne!(after_edit, after_add);

    fs::remove_file(dir.join("b.fits")).unwrap();
    assert_ne!(fingerprint(&path), after_edit);
}

/// One pass identifies the paths of every node in a run, so a path that will
/// not read must cost its own node's digest and nothing else. Aborting on the
/// first failure left every path behind it unstamped — and which ones those
/// were was queue order, not anything about the graph.
#[test]
fn one_unreadable_path_does_not_cost_the_pass() {
    let dir = TempDir::new("survivors");
    let in_dir = |name: String| dir.join(name).to_string_lossy().into_owned();
    // The queue is a `HashSet`, so the order a pass drains it in is the
    // hasher's, reseeded per process — one readable path beside one failure
    // would only catch an abort on the runs that happened to draw the failure
    // first. Twelve against three: an abort keeps every readable path only
    // when all three failures draw last, one arrangement of `C(15, 3)`.
    let present_paths = (0..12)
        .map(|i| {
            let path = in_dir(format!("present-{i}.bin"));
            fs::write(&path, b"x").unwrap();
            path
        })
        .collect::<Vec<_>>();
    let missing_paths = (0..3)
        .map(|i| in_dir(format!("missing-{i}.bin")))
        .collect::<Vec<_>>();
    let stamped_paths = |job: &StampJob| {
        let mut paths = job
            .stamped
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    };

    let mut job = StampJob::default();
    for path in present_paths.iter().chain(&missing_paths) {
        job.request(path.clone());
    }
    let resolved = job.run(&CancelToken::never());
    let Err(StampError::Io { path, source }) = &resolved else {
        panic!("a path that would not read is still reported: {resolved:?}");
    };
    assert!(
        missing_paths
            .iter()
            .any(|missing| path.as_os_str() == missing.as_str()),
        "the report names a path that would not read: {path:?}",
    );
    assert_eq!(source.kind(), io::ErrorKind::NotFound);
    let mut expected = present_paths.clone();
    expected.sort();
    assert_eq!(
        stamped_paths(&job),
        expected,
        "and every readable path still lands, wherever the failures fell",
    );
    // The pass drains as it walks, so nothing sits behind a failure — which is
    // why queueing a node's paths never has to clear the queue first.
    assert!(
        !job.is_queued(),
        "the pass drains whatever it reports, so nothing is left queued",
    );

    // Nothing records that a path was tried: the same path reads at the node's
    // turn once an upstream producer has written it.
    let written_late = in_dir("missing-0.bin".to_string());
    fs::write(&written_late, b"now here").unwrap();
    let mut job = StampJob::default();
    job.request(written_late.clone());
    assert!(
        job.run(&CancelToken::never()).is_ok(),
        "a failure is reported, not remembered",
    );
    assert_eq!(stamped_paths(&job), slice::from_ref(&written_late));

    // Cancellation is the one verdict that still stops the walk where it
    // stands, rather than raising itself once per remaining path.
    let mut job = StampJob::default();
    for path in &present_paths {
        job.request(path.clone());
    }
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(
        matches!(job.run(&cancel), Err(StampError::Cancelled)),
        "a cancelled pass reports the cancellation",
    );
    assert!(
        job.stamped.is_empty(),
        "and identifies nothing after it is raised",
    );
}

/// A pure function handed a directory consumes it recursively, so its
/// identity has to be the whole subtree. Stamping one level deep would let
/// everything below the first level change under a fingerprint that never
/// moved, and the node would reuse output built from the old contents.
#[test]
fn directory_identity_tracks_nested_changes() {
    let dir = TempDir::new("nested");
    let path = dir.path().to_string_lossy().into_owned();
    let sub = dir.join("sub");
    fs::create_dir_all(sub.join("deeper")).unwrap();
    fs::write(sub.join("file.bin"), b"one").unwrap();
    let base = fingerprint(&path);
    assert_eq!(fingerprint(&path), base, "a still tree stamps stably");

    // The case the one-level stamp missed: a nested edit that does not
    // touch any immediate child of the root.
    fs::write(sub.join("file.bin"), b"one-plus").unwrap();
    let after_nested_edit = fingerprint(&path);
    assert_ne!(after_nested_edit, base, "nested edit must move the root");

    // Depth is not special-cased — the deepest level counts too.
    fs::write(sub.join("deeper").join("leaf.bin"), b"x").unwrap();
    let after_deep_add = fingerprint(&path);
    assert_ne!(after_deep_add, after_nested_edit);

    // Only files are folded, so an empty directory is an absence: there
    // is nothing beneath it for a node to read, and nothing it can change
    // without a file changing with it.
    fs::create_dir(sub.join("deeper").join("empty")).unwrap();
    assert_eq!(
        fingerprint(&path),
        after_deep_add,
        "an empty directory is not part of the identity"
    );
    // …and it stops being an absence the moment it holds something.
    fs::write(sub.join("deeper").join("empty").join("c.bin"), b"c").unwrap();
    assert_ne!(fingerprint(&path), after_deep_add);
}

/// One stamper walks every path of every run in the same buffers, so a
/// walk must leave nothing of itself behind — a stale entry from the last
/// directory would fold into the next directory's identity.
#[test]
fn a_reused_stamper_stamps_like_a_fresh_one() {
    let dir = TempDir::new("reuse");
    let deep = dir.join("deep");
    let shallow = dir.join("shallow");
    fs::create_dir_all(deep.join("nested")).unwrap();
    fs::create_dir(&shallow).unwrap();
    fs::write(deep.join("nested").join("a.bin"), b"one").unwrap();
    fs::write(shallow.join("b.bin"), b"two").unwrap();
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
    let path = dir.path().to_string_lossy().into_owned();
    // Both lossy-convert to the same U+FFFD replacement character.
    let first = dir.join(OsStr::from_bytes(b"\xff"));
    let second = dir.join(OsStr::from_bytes(b"\xfe"));

    // APFS refuses a name that is not valid UTF-8 (`EILSEQ`), so on macOS the
    // input this test is about cannot be created at all — leave it to the
    // filesystems that can express one rather than failing on every dev box.
    if fs::write(&first, b"same").is_err() {
        return;
    }
    let with_first = fingerprint(&path);
    fs::rename(&first, &second).unwrap();
    // Same length, and the rename preserves mtime, so the *name* is the
    // only thing that moved.
    assert_ne!(
        fingerprint(&path),
        with_first,
        "a rename between two non-UTF-8 names must move the fingerprint",
    );
}

/// Files that differ only in an mtime before, at or after the epoch digest apart. Same length
/// throughout, so mtime is the only field in play; the signed conversion itself is
/// `common::FileIdentity`'s to test.
#[test]
fn file_identity_separates_pre_epoch_mtimes() {
    let digest_of = |mtime_ns| {
        let mut hasher = DigestHasher::new();
        FsPathId::file(4, mtime_ns).hash(&mut hasher);
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

/// Two content-cacheable nodes of one func, each declaring the *same* const
/// path — the pair whose digests must agree within a run and move between them.
#[tokio::test]
async fn same_path_uses_one_identity_until_the_next_run() {
    let dir = TempDir::new("snapshot");
    let file = dir.join("data.bin");
    fs::write(&file, b"x").unwrap();
    let path = ConstValue::FsPath(file.to_string_lossy().into_owned());

    let mut prog = ProgramBuilder::default();
    let shared_func = FuncId::from_u128(10);
    let const_path_node = |prog: &mut ProgramBuilder| {
        prog.node()
            .pure()
            .func(shared_func)
            .const_input(path.clone())
            .output_types([DataType::Int])
            .add()
    };
    let first = const_path_node(&mut prog);
    let second = const_path_node(&mut prog);
    let schedule = prog.planned();
    let program = prog.program();

    let mut cache = RuntimeCache::default();
    cache.install_for_test(program);

    cache
        .prepare(program, schedule.executing(), CancelToken::never())
        .await;
    cache.stamp_digest(program, &schedule.states, first.node_idx);

    fs::write(&file, b"longer").unwrap();
    cache.stamp_digest(program, &schedule.states, second.node_idx);
    assert_eq!(
        cache[first.node_idx].current_digest, cache[second.node_idx].current_digest,
        "both consumers fold the run's one coherent resource identity"
    );

    let first_run = cache[first.node_idx].current_digest;
    // A key only the memo has: a copy of the path would be the path's length.
    let path_text = file.to_string_lossy().into_owned();
    cache.widen_path_key(&path_text, 4096);
    cache
        .prepare(program, schedule.executing(), CancelToken::never())
        .await;
    cache.stamp_digest(program, &schedule.states, first.node_idx);
    assert_ne!(
        cache[first.node_idx].current_digest, first_run,
        "the next run refreshes resource identity"
    );
    assert!(
        cache.path_key(&path_text).unwrap().capacity() >= 4096,
        "and identifies the path under the key the last run held"
    );
}
