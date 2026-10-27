use std::path::Path;
use std::sync::Arc;

use common::{TempDir, fs};
use lens::MlModelPaths;
use scenarium::{CacheMode, ConstValue, Graph, NodeId};

use crate::core::io::cache;
use crate::core::io::preferences::Preferences;
use crate::core::runtime_host::{CacheRootChange, RuntimeHost};
use crate::core::status_log::StatusLog;

/// The default value seeded into `func`'s model-path input (index 1),
/// read back through the published library.
fn ml_model_default(host: &RuntimeHost, func: &str) -> Option<ConstValue> {
    host.library.cur().by_name(func).unwrap().inputs()[1]
        .default_value
        .clone()
}

/// An eviction reaches the seed's consumer cone in the program it installs —
/// here the seed alone, as nothing reads it — and nothing for a node the
/// program holds no work for.
#[test]
fn an_eviction_reaches_the_seed_cone_of_the_installed_program() {
    let mut host = RuntimeHost::new(Arc::new(|| {}), &Preferences::default());
    let func = host
        .library
        .cur()
        .by_name("ML Denoise")
        .expect("built-in present");
    let mut graph = Graph::default();
    let node = graph.add_func_node(func);

    let mut status = StatusLog::default();
    host.run_once(&graph, &mut status);
    assert_eq!(host.evict_cache(&graph, node, &mut status), vec![node]);
    assert!(
        host.evict_cache(&graph, NodeId::unique(), &mut status)
            .is_empty(),
        "a node the program has no work for reaches nothing"
    );
    assert_eq!(status.cur(), None);
}

#[test]
fn ml_defaults_follow_preferences_and_the_cache_root_follows_the_document() {
    let dir = TempDir::new("darkroom-runtime-host");
    // Under the temp dir, so absolute on every platform: the host makes the paths absolute, and
    // a rooted Unix path has no drive on Windows.
    let model = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let denoise_path = model("host-denoise.onnx");
    let star_removal_path = model("host-stars.onnx");
    let mut preferences = Preferences {
        ml_models: MlModelPaths {
            denoise: denoise_path.as_str().into(),
            star_removal: star_removal_path.as_str().into(),
        },
        ..Preferences::default()
    };
    let mut host = RuntimeHost::new(Arc::new(|| {}), &preferences);

    // Startup seeds the ML nodes' path inputs from the preferences.
    assert_eq!(
        ml_model_default(&host, "ML Denoise"),
        Some(ConstValue::FsPath(denoise_path))
    );
    assert_eq!(
        ml_model_default(&host, "ML Star Removal"),
        Some(ConstValue::FsPath(star_removal_path))
    );

    // A later preferences edit republishes the library with the new paths.
    let updated_denoise_path = model("updated-denoise.onnx");
    let updated_star_removal_path = model("updated-stars.onnx");
    preferences.ml_models.denoise = updated_denoise_path.as_str().into();
    preferences.ml_models.star_removal = updated_star_removal_path.as_str().into();
    host.configure_ml_model_defaults(&preferences);
    assert_eq!(
        ml_model_default(&host, "ML Denoise"),
        Some(ConstValue::FsPath(updated_denoise_path))
    );
    assert_eq!(
        ml_model_default(&host, "ML Star Removal"),
        Some(ConstValue::FsPath(updated_star_removal_path))
    );

    // The disk cache is memory-only until a document has a path, then
    // repoints as documents open and again when the path goes away.
    // Pointing the cache at a document creates nothing on disk: its root
    // appears, with its `.gitignore`, only when a compiled program holds a
    // disk-backed node that can write there.
    let first_path = dir.join("first.darkroom");
    let second_path = dir.join("second.darkroom");
    let (first_root, second_root) = (cache::root(&first_path), cache::root(&second_path));
    assert_eq!(host.disk_root, None);
    host.set_document_cache(Some(&first_path));
    assert_eq!(host.disk_root.as_deref(), Some(first_root.as_path()));
    assert!(
        !fs::exists(&first_root),
        "opening a document writes nothing"
    );

    let mut status = StatusLog::default();
    let mut memory_only = Graph::default();
    let func = host
        .library
        .cur()
        .by_name("ML Denoise")
        .expect("built-in present");
    let node = memory_only.add_func_node(func);
    host.run_once(&memory_only, &mut status);
    assert!(
        !fs::exists(&first_root),
        "a memory-only program writes nothing"
    );

    let mut disk_backed = memory_only;
    disk_backed.find_mut(node).unwrap().cache = CacheMode::Disk;
    host.run_once(&disk_backed, &mut status);
    assert!(
        fs::is_file(first_root.join(".gitignore")),
        "a disk-backed program prepares the root before it can write"
    );

    // A new root is prepared again, on its own next disk-backed compile.
    host.set_document_cache(Some(&second_path));
    assert_eq!(host.disk_root.as_deref(), Some(second_root.as_path()));
    assert!(!fs::exists(&second_root));
    host.run_once(&disk_backed, &mut status);
    assert!(fs::is_file(second_root.join(".gitignore")));
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
    for (prev, cur, expected, why) in [
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
            CacheRootChange::of(prev, cur),
            expected,
            "{why}: {prev:?} -> {cur:?}"
        );
    }
}
