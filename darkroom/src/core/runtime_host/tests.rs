use std::path::Path;
use std::sync::Arc;

use crate::core::status::StatusLog;
use common::TempDir;
use lens::MlModelPaths;
use scenarium::{Binding, CacheMode, ConstValue, Graph, InputPort, NodeId};

use crate::core::io::cache::document_cache_root;
use crate::core::io::preferences::Preferences;
use crate::core::runtime_host::{CacheRootChange, RuntimeHost};

/// The default value seeded into `func`'s model-path input (index 1),
/// read back through the published library.
fn ml_model_default(host: &RuntimeHost, func: &str) -> Option<ConstValue> {
    host.library.current().by_name(func).unwrap().inputs[1]
        .default_value
        .clone()
}

#[test]
fn stale_wiring_survives_compilation_and_still_runs() {
    let mut host = RuntimeHost::new(Arc::new(|| {}), &Preferences::default());

    // Library drift: a wire into an output the func doesn't declare.
    // Nothing prunes it — the authored wiring stays (it revives if the
    // library gets the port back) and compilation tolerates it as an
    // unbound input.
    let func = host
        .library
        .current()
        .by_name("ML Denoise")
        .expect("built-in present");
    let mut graph = Graph::default();
    let producer = graph.add_func_node(func);
    let consumer = graph.add_func_node(func);
    let dangling = InputPort::new(consumer, 0);
    graph.set_input_binding(dangling, Binding::bind(producer, 99));

    let mut status = StatusLog::default();
    assert!(
        host.run_once(&graph, &mut status),
        "the drifted graph compiles and is queued"
    );
    assert_eq!(
        host.evict_cache(&graph, consumer, &mut status),
        vec![consumer],
        "the drifted graph compiles and queues an eviction reaching only \
         the seed — its one incoming wire is unbound, so nothing reads it"
    );
    assert!(
        host.evict_cache(&graph, NodeId::unique(), &mut status)
            .is_empty(),
        "a node the program has no work for reaches nothing"
    );
    assert_eq!(status.current(), None, "no compile failure was reported");
    assert_eq!(
        graph.bindings.get(&dangling),
        Some(&Binding::bind(producer, 99)),
        "the dangling wire is preserved, not pruned"
    );
}

#[test]
fn ml_defaults_follow_preferences_and_the_cache_root_follows_the_document() {
    let denoise_path = "/models/host-denoise.onnx";
    let star_removal_path = "/models/host-stars.onnx";
    let mut preferences = Preferences {
        ml_models: MlModelPaths {
            denoise: denoise_path.into(),
            star_removal: star_removal_path.into(),
        },
        ..Preferences::default()
    };
    let mut host = RuntimeHost::new(Arc::new(|| {}), &preferences);

    // Startup seeds the ML nodes' path inputs from the preferences.
    assert_eq!(
        ml_model_default(&host, "ML Denoise"),
        Some(ConstValue::FsPath(denoise_path.to_owned()))
    );
    assert_eq!(
        ml_model_default(&host, "ML Star Removal"),
        Some(ConstValue::FsPath(star_removal_path.to_owned()))
    );

    // A later preferences edit republishes the library with the new paths.
    let updated_denoise_path = "/models/updated-denoise.onnx";
    let updated_star_removal_path = "/models/updated-stars.onnx";
    preferences.ml_models.denoise = updated_denoise_path.into();
    preferences.ml_models.star_removal = updated_star_removal_path.into();
    host.configure_ml_model_defaults(&preferences);
    assert_eq!(
        ml_model_default(&host, "ML Denoise"),
        Some(ConstValue::FsPath(updated_denoise_path.to_owned()))
    );
    assert_eq!(
        ml_model_default(&host, "ML Star Removal"),
        Some(ConstValue::FsPath(updated_star_removal_path.to_owned()))
    );

    // The disk cache is memory-only until a document has a path, then
    // repoints as documents open and again when the path goes away.
    // Pointing the cache at a document creates nothing on disk: its root
    // appears, with its `.gitignore`, only when a compiled program holds a
    // disk-backed node that can write there.
    let dir = TempDir::new("darkroom-runtime-host");
    let first_path = dir.join("first.darkroom");
    let second_path = dir.join("second.darkroom");
    let (first_root, second_root) = (
        document_cache_root(&first_path),
        document_cache_root(&second_path),
    );
    assert_eq!(host.disk_root, None);
    host.set_document_cache(Some(&first_path));
    assert_eq!(host.disk_root.as_deref(), Some(first_root.as_path()));
    assert!(!first_root.exists(), "opening a document writes nothing");

    let mut status = StatusLog::default();
    let mut memory_only = Graph::default();
    let func = host
        .library
        .current()
        .by_name("ML Denoise")
        .expect("built-in present");
    let node = memory_only.add_func_node(func);
    assert!(host.run_once(&memory_only, &mut status));
    assert!(!first_root.exists(), "a memory-only program writes nothing");

    let mut disk_backed = memory_only;
    disk_backed.find_mut(node).unwrap().cache = CacheMode::Disk;
    assert!(host.run_once(&disk_backed, &mut status));
    assert!(
        first_root.join(".gitignore").is_file(),
        "a disk-backed program prepares the root before it can write"
    );

    // A new root is prepared again, on its own next disk-backed compile.
    host.set_document_cache(Some(&second_path));
    assert_eq!(host.disk_root.as_deref(), Some(second_root.as_path()));
    assert!(!second_root.exists());
    assert!(host.run_once(&disk_backed, &mut status));
    assert!(second_root.join(".gitignore").is_file());
    host.set_document_cache(None);
    assert_eq!(host.disk_root, None);
}

/// The whole policy behind what a root change tells the worker. Only the
/// first save of an unsaved document owes a flush of what is already
/// resident; everything else either repoints silently or does nothing at
/// all. Getting this wrong is what wrote a closed document's values into
/// the newly opened one's cache directory.
#[test]
fn only_gaining_a_first_root_owes_the_worker_a_flush() {
    let a = Path::new("/docs/a.darkroom-cache");
    let b = Path::new("/docs/b.darkroom-cache");
    for (previous, current, expected, why) in [
        (None, None, CacheRootChange::Unchanged, "still unsaved"),
        (Some(a), Some(a), CacheRootChange::Unchanged, "a re-save"),
        (None, Some(a), CacheRootChange::Gained, "the first save"),
        (
            Some(a),
            Some(b),
            CacheRootChange::Repointed,
            "Save-As / open",
        ),
        (Some(a), None, CacheRootChange::Repointed, "File ▸ New"),
    ] {
        assert_eq!(
            CacheRootChange::of(previous, current),
            expected,
            "{why}: {previous:?} -> {current:?}"
        );
    }
}
