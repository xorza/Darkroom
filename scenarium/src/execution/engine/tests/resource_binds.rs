use super::*;

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::async_lambda;
use crate::{FsPathConfig, FsPathMode};

/// A temp file path removed on drop.
#[derive(Debug)]
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn temp_file(tag: &str) -> TempFile {
    static C: AtomicU64 = AtomicU64::new(0);
    let n = C.fetch_add(1, Ordering::Relaxed);
    TempFile(std::env::temp_dir().join(format!(
        "scenarium-resbind-{tag}-{}-{n}.bin",
        std::process::id()
    )))
}

/// A unique temp directory removed on drop (the disk store root).
#[derive(Debug)]
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scenarium-resbind-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How often each counted node ran, and what the sink last saw.
#[derive(Debug, Default, Clone)]
struct Observed {
    loads: Arc<AtomicUsize>,
    annotates: Arc<AtomicUsize>,
    captured: Arc<StdMutex<String>>,
}

impl Observed {
    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
    fn annotates(&self) -> usize {
        self.annotates.load(Ordering::SeqCst)
    }
    fn captured(&self) -> String {
        self.captured.lock().unwrap().clone()
    }
}

fn any_path() -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::default()))
}

fn existing_file() -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)))
}

/// Reads the file its declared-`FsPath` input names, counting invocations.
fn loader(observed: Observed) -> impl FnOnce(NodeSpec) -> NodeSpec {
    move |n: NodeSpec| {
        n.pure()
            .input(existing_file())
            .output(DataType::String)
            .lambda(async_lambda!(
                move |Invocation { inputs, outputs, .. }| { loads = observed.loads.clone() } => {
                    loads.fetch_add(1, Ordering::SeqCst);
                    let path = inputs[0].as_fs_path().unwrap().to_string();
                    let text = std::fs::read_to_string(&path).map_err(InvokeError::external)?;
                    outputs[0] = StaticValue::String(text).into();
                    Ok(())
                }
            ))
    }
}

/// The sink both fixtures share: records the received value's text.
fn capture(observed: Observed) -> impl FnOnce(NodeSpec) -> NodeSpec {
    move |n: NodeSpec| {
        n.sink().input(DataType::Any).lambda(async_lambda!(
            move |Invocation { inputs, .. }| { captured = observed.captured.clone() } => {
                *captured.lock().unwrap() = inputs[0].as_string().unwrap_or_default().to_string();
                Ok(())
            }
        ))
    }
}

/// `make_path(const name) → load_text → annotate → capture`, the three pure
/// nodes on `mode`.
///
/// `make_path` is pure `String → FsPath`: a producer whose *own* digest does
/// not track the file, like any path-computing node. `annotate` sits
/// downstream of the late-stamped loader, so it proves the reach-time
/// re-stamp cascades and downstream caches still hit.
///
/// Declaration order fixes the node ids, so a reopened engine addresses the
/// same slots.
fn path_graph(data_path: &str, mode: CacheMode, observed: Observed) -> TestGraph {
    let mut g = TestGraph::new();
    g.add("make_path", |n| {
        n.pure()
            .cache(mode)
            .input(DataType::String)
            .output(any_path())
            .compute(|inputs| StaticValue::FsPath(inputs[0].as_string().unwrap().to_string()))
    });
    g.add("load_text", |n| loader(observed.clone())(n).cache(mode));
    g.add("annotate", |n| {
        let annotates = observed.annotates.clone();
        n.pure()
            .cache(mode)
            .input(DataType::String)
            .output(DataType::String)
            .compute(move |inputs| {
                annotates.fetch_add(1, Ordering::SeqCst);
                StaticValue::String(format!("[{}]", inputs[0].as_string().unwrap()))
            })
    });
    g.add("capture", capture(observed));
    g.constant("make_path", 0, data_path);
    g.wire("make_path", 0, "load_text", 0);
    g.wire("load_text", 0, "annotate", 0);
    g.wire("annotate", 0, "capture", 0);
    g
}

/// A declared path with no determinate identity fails **the node that
/// declares it**, and its dependents skip as errored-upstream.
///
/// Leaving it unstamped is correct — the node recomputes rather than
/// reusing a result keyed on a guess — but it is also silent: the only
/// symptom is a cache that stops hitting, with nothing said. Failing
/// the whole run is the other extreme, and the pre-run sweep cannot
/// even name a culprit, since it batches every executing node's paths
/// at once. Both routes therefore converge on the node: a const path
/// the sweep could not stamp leaves its digest `None`, which sends it
/// through the same per-node stamp a producer-supplied path takes.
#[tokio::test]
#[cfg(unix)]
async fn an_unidentifiable_path_fails_only_the_node_declaring_it() {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("locked");
    let data = dir.0.join("data.txt");
    std::fs::write(&data, "v1").unwrap();
    let data_path = data.to_string_lossy().into_owned();
    let lock = |mode| std::fs::set_permissions(&dir.0, Permissions::from_mode(mode)).unwrap();

    // `loader` failed for want of a path identity, `dependent` was skipped
    // for reading it, and the run itself still succeeded.
    let assert_unavailable = |run: &RunOutcome, loader: &str, dependent: &str| {
        assert!(
            matches!(
                run.error(loader),
                Some(RunError::ResourceUnavailable { .. })
            ),
            "the node declaring the path must fail: {:?}",
            run.error(loader),
        );
        assert!(
            matches!(run.error(dependent), Some(RunError::SkippedUpstream { .. })),
            "its dependent must skip as errored-upstream: {:?}",
            run.error(dependent),
        );
    };

    // A const path, known before the run: the sweep cannot stamp it, so the
    // node reaches its turn with no digest and re-stamps there.
    let mut g = TestGraph::new();
    g.add("load_text", loader(Observed::default()));
    g.add("capture", capture(Observed::default()));
    g.constant("load_text", 0, StaticValue::FsPath(data_path.clone()));
    g.wire("load_text", 0, "capture", 0);
    let mut e = TestEngine::over(g);

    lock(0o000);
    let run = e.run(RunSeeds::sinks()).await;
    lock(0o755);
    assert_unavailable(
        &run.expect("a per-node failure must not abort the run"),
        "load_text",
        "capture",
    );

    // The same file reached through a producer's value, known only at the
    // node's turn. Same outcome, same route.
    let mut e = TestEngine::over(path_graph(&data_path, CacheMode::None, Observed::default()));
    lock(0o000);
    let run = e.run(RunSeeds::sinks()).await;
    lock(0o755);
    assert_unavailable(
        &run.expect("a per-node failure must not abort the run"),
        "load_text",
        "annotate",
    );
}

/// The core regression: a path arriving over a **Bind** edge keys the loader
/// on the file behind the *delivered value*. Editing the file re-keys and
/// recomputes the loader (pre-fix the chain's digests never changed, so the
/// stale decode was served forever), while an unchanged file still reuses
/// the cache — the reach-time re-stamp keeps wired-path loaders cacheable
/// instead of tainting them uncacheable.
#[tokio::test]
async fn wired_path_rekeys_loader_on_file_change() {
    let data = temp_file("ram");
    std::fs::write(&data.0, "v1").unwrap();
    let observed = Observed::default();
    let mut e = TestEngine::over(path_graph(
        &data.0.to_string_lossy(),
        CacheMode::Ram,
        observed.clone(),
    ));

    // Cold run: everything computes. The loader's pre-run digest is `None`
    // — the delivered value does not exist yet — so it re-stamps at reach
    // time and runs.
    e.run_sinks().await;
    assert_eq!((observed.loads(), observed.annotates()), (1, 1));
    assert_eq!(observed.captured(), "[v1]");

    // Unchanged file: the loader reuses its RAM value under the full digest
    // (producer port + live file identity), and its *downstream* — whose
    // digest folds the loader's — skips too.
    let run = e.run_sinks().await;
    assert_eq!(
        observed.loads(),
        1,
        "unchanged file ⇒ the loader stays cached"
    );
    assert_eq!(
        observed.annotates(),
        1,
        "downstream of the late-stamped loader skips compute on its hit"
    );
    assert_eq!(run.cached(), ["annotate", "load_text", "make_path"]);

    // Edit the file (different length ⇒ unambiguous identity change). The
    // loader re-keys off the delivered value's file identity and the change
    // propagates downstream — while the structural upstream stays a hit.
    std::fs::write(&data.0, "v2-longer").unwrap();
    let run = e.run_sinks().await;
    assert_eq!(
        observed.loads(),
        2,
        "a file edit re-keys the loader through the wired path"
    );
    assert_eq!(
        observed.annotates(),
        2,
        "the loader's new digest invalidates its downstream"
    );
    assert_eq!(
        observed.captured(),
        "[v2-longer]",
        "fresh content flows down"
    );
    assert_eq!(
        run.ran(),
        ["load_text", "annotate", "capture"],
        "the path producer itself stays cached — nothing structural changed"
    );
}

/// Disk persistence across a reopen with a wired path: the loader's blob is
/// keyed under the delivered path's live identity, so a fresh engine reuses
/// it while the file is unchanged — hydrating the on-disk path producer just
/// to stamp — and recomputes once the file changes, while the producer
/// itself stays a disk hit. The downstream `annotate` proves the re-stamp
/// *cascade*: on reopen its own pre-run digest is `None` too (it folds the
/// loader's), and its reach-time re-stamp lands on its blob — the whole
/// tainted cone skips compute, not just the loader.
#[tokio::test]
async fn wired_path_disk_reuse_survives_reopen_until_file_changes() {
    let dir = TempDir::new("disk");
    let data = temp_file("disk-data");
    std::fs::write(&data.0, "v1").unwrap();
    let observed = Observed::default();
    let path = data.0.to_string_lossy().into_owned();

    // A fresh engine over an identically-declared graph: `TestGraph` mints
    // ids in declaration order, so the reopened engine addresses the very
    // slots the blobs were written under.
    let reopen = |observed: Observed| {
        let mut e = TestEngine::over(path_graph(&path, CacheMode::Disk, observed));
        e.attach_disk_store(dir.0.clone());
        e
    };

    // Cold run: computes and stores the blobs.
    let mut e = reopen(observed.clone());
    e.run_sinks().await;
    assert_eq!((observed.loads(), observed.annotates()), (1, 1));

    // Reopen, unchanged file: the loader is a disk hit under the re-stamped
    // digest, and so is its downstream — each re-stamped at reach time,
    // producer-first.
    let mut e = reopen(observed.clone());
    let run = e.run_sinks().await;
    assert_eq!(
        observed.loads(),
        1,
        "reopen with an unchanged file serves the loader from disk"
    );
    assert_eq!(
        observed.annotates(),
        1,
        "downstream of the late-stamped loader is a disk hit too"
    );
    assert_eq!(run.cached(), ["annotate", "load_text", "make_path"]);
    assert_eq!(
        observed.captured(),
        "[v1]",
        "the sink reads the hydrated disk value"
    );

    // Reopen after an edit: the loader's key moved ⇒ recompute, propagating
    // downstream; the path producer's own digest is unchanged, so it stays a
    // disk hit feeding the recompute.
    std::fs::write(&data.0, "v2-longer").unwrap();
    let mut e = reopen(observed.clone());
    let run = e.run_sinks().await;
    assert_eq!(
        observed.loads(),
        2,
        "reopen after a file edit recomputes the loader"
    );
    assert_eq!(
        observed.annotates(),
        2,
        "the loader's new digest invalidates its downstream"
    );
    assert_eq!(observed.captured(), "[v2-longer]");
    assert_eq!(
        run.ran(),
        ["load_text", "annotate", "capture"],
        "the path producer is served from its blob, not recomputed"
    );
}
