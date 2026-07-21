use std::sync::Arc;

use common::test_utils::test_output_path;
use scenarium::StaticValue;

use crate::core::document::Document;
use crate::core::document::open_document::OpenDocument;
use crate::core::io::cache::document_cache_root;
use crate::core::io::document;
use crate::core::io::preferences::{MlModelPreferences, Preferences};
use crate::core::runtime_host::test_support;
use crate::core::script::ScriptConfig;
use crate::core::workspace::Workspace;

#[test]
fn normalization_is_shared_across_run_and_replacement_boundaries() {
    let mut workspace = Workspace::new(
        &ScriptConfig::default(),
        Arc::new(|| {}),
        &Preferences::default(),
    );

    assert!(workspace.open.normalization_pending);
    assert!(
        workspace.run_once(),
        "the empty graph compiles and is queued"
    );
    assert!(!workspace.open.normalization_pending);

    workspace.replace_document(OpenDocument::default());
    assert!(workspace.open.normalization_pending);
    workspace.normalize_document();
    assert!(!workspace.open.normalization_pending);
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
    let mut workspace = Workspace::new(&ScriptConfig::default(), Arc::new(|| {}), &preferences);

    assert_eq!(workspace.open.path, Some(first_path.clone()));
    assert_eq!(
        workspace
            .runtime
            .library
            .current
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
            .current
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
            .current
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
            .current
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
}

#[test]
fn disabled_reopen_ignores_the_remembered_path() {
    let preferences = Preferences {
        document_path: Some("does-not-exist.darkroom".into()),
        load_last_document: false,
        ..Preferences::default()
    };

    let workspace = Workspace::new(&ScriptConfig::default(), Arc::new(|| {}), &preferences);

    assert!(workspace.open.path.is_none());
    assert!(workspace.open.document.graph.is_empty());
}

#[test]
fn startup_load_failure_falls_back_to_empty_and_reports_the_error() {
    let preferences = Preferences {
        document_path: Some("not-a-document.json".into()),
        ..Preferences::default()
    };

    let workspace = Workspace::new(&ScriptConfig::default(), Arc::new(|| {}), &preferences);

    assert!(workspace.open.path.is_none());
    assert!(workspace.open.document.graph.is_empty());
    assert_eq!(test_support::disk_root(&workspace.runtime), None);
    assert!(
        workspace
            .runtime
            .status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("load failed:"))
    );
}
