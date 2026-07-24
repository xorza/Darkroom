use std::sync::Arc;

use common::test_utils::test_output_path;
use scenarium::{NodeId, StaticValue};

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::io::cache::document_cache_root;
use crate::core::io::document;
use crate::core::io::preferences::{MlModelPreferences, Preferences};
use crate::core::runtime_host::test_support;
use crate::core::script::ScriptConfig;
use crate::core::status::StatusLog;
use crate::core::workspace::{self as workspace_module, Workspace};

#[test]
fn stale_wiring_survives_install_and_still_compiles() {
    use scenarium::{Binding, Graph, InputPort};

    let mut preferences = Preferences::default();
    let mut workspace = Workspace::new(&ScriptConfig::default(), Arc::new(|| {}), &mut preferences);

    // Library drift: a wire into an output the func doesn't declare.
    // Nothing prunes it — the document keeps the authored wiring (it
    // revives if the library gets the port back) and compilation
    // tolerates it as an unbound input.
    let library = workspace.runtime.library.published.load();
    let func = library.by_name("ML Denoise").expect("built-in present");
    let mut graph = Graph::default();
    let producer = graph.add_func_node(func);
    let consumer = graph.add_func_node(func);
    let dangling = InputPort::new(consumer, 0);
    graph.set_input_binding(dangling, Binding::bind(producer, 99));
    drop(library);

    workspace.replace_document(OpenDocument {
        document: Document::from(graph),
        path: None,
    });
    assert_eq!(
        workspace.open.document.graph.bindings.get(&dangling),
        Some(&Binding::bind(producer, 99)),
        "the dangling wire is preserved, not pruned"
    );
    assert!(
        workspace.run_once(),
        "the drifted graph compiles and is queued"
    );
    assert!(
        workspace.evict_cache(NodeId::unique()),
        "the drifted graph compiles and queues an eviction"
    );
}

#[test]
fn startup_applies_preferences_and_replacement_repoints_the_runtime_cache() {
    let first_path = test_output_path("darkroom_workspace/first.darkroom");
    let second_path = test_output_path("darkroom_workspace/second.darkroom");
    let denoise_path = "/models/workspace-denoise.onnx";
    let star_removal_path = "/models/workspace-stars.onnx";
    document::save(&Document::default(), &first_path).unwrap();
    let mut preferences = Preferences {
        document_path: Some(first_path.clone()),
        ml_models: MlModelPreferences {
            denoise: denoise_path.into(),
            star_removal: star_removal_path.into(),
        },
        ..Preferences::default()
    };
    let mut workspace = Workspace::new(&ScriptConfig::default(), Arc::new(|| {}), &mut preferences);

    assert_eq!(workspace.open.path, Some(first_path.clone()));
    assert_eq!(
        workspace
            .runtime
            .library
            .published
            .load()
            .by_name("ML Denoise")
            .unwrap()
            .inputs[1]
            .default_value,
        Some(StaticValue::FsPath(denoise_path.to_owned()))
    );
    assert_eq!(
        workspace
            .runtime
            .library
            .published
            .load()
            .by_name("ML Star Removal")
            .unwrap()
            .inputs[1]
            .default_value,
        Some(StaticValue::FsPath(star_removal_path.to_owned()))
    );

    let updated_denoise_path = "/models/updated-denoise.onnx";
    let updated_star_removal_path = "/models/updated-stars.onnx";
    preferences.ml_models.denoise = updated_denoise_path.into();
    preferences.ml_models.star_removal = updated_star_removal_path.into();
    workspace.runtime.configure_ml_model_defaults(&preferences);
    assert_eq!(
        workspace
            .runtime
            .library
            .published
            .load()
            .by_name("ML Denoise")
            .unwrap()
            .inputs[1]
            .default_value,
        Some(StaticValue::FsPath(updated_denoise_path.to_owned()))
    );
    assert_eq!(
        workspace
            .runtime
            .library
            .published
            .load()
            .by_name("ML Star Removal")
            .unwrap()
            .inputs[1]
            .default_value,
        Some(StaticValue::FsPath(updated_star_removal_path.to_owned()))
    );
    assert_eq!(
        test_support::disk_root(&workspace.runtime),
        Some(document_cache_root(&first_path))
    );

    document::save(&Document::default(), &second_path).unwrap();
    workspace.replace_document(OpenDocument::load(second_path.clone()).unwrap());
    assert_eq!(workspace.open.path, Some(second_path.clone()));
    assert_eq!(
        test_support::disk_root(&workspace.runtime),
        Some(document_cache_root(&second_path))
    );

    preferences.document_path = Some("invalid.json".into());
    let mut status = StatusLog::default();
    let open =
        workspace_module::load_preferred_document_with(&mut preferences, &mut status, |_| {
            Err("preferences save failed: disk unavailable".into())
        });
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, None);
    assert_eq!(
        status.lines().collect::<Vec<_>>(),
        [
            "load failed: invalid.json must use the .darkroom extension",
            "preferences save failed: disk unavailable"
        ]
    );
}
