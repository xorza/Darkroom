use std::sync::Arc;

use common::test_utils::test_output_path;

use crate::core::document::Document;
use crate::core::io::cache::document_cache_root;
use crate::core::io::preferences::Preferences;
use crate::core::open_document::OpenDocument;
use crate::core::runtime_host::test_support;
use crate::core::script::ScriptConfig;
use crate::core::workspace::Workspace;

#[test]
fn normalization_is_shared_across_run_and_replacement_boundaries() {
    let mut workspace = Workspace::new(
        OpenDocument::empty(),
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

    workspace.replace_document(Document::default(), None);
    assert!(workspace.open.normalization_pending);
    workspace.normalize_document();
    assert!(!workspace.open.normalization_pending);
}

#[test]
fn replacement_repoints_the_runtime_cache_to_the_new_document() {
    let first_path = test_output_path("darkroom_workspace/first.darkroom");
    let second_path = test_output_path("darkroom_workspace/second.darkroom");
    let mut workspace = Workspace::new(
        OpenDocument::empty(),
        &ScriptConfig::default(),
        Arc::new(|| {}),
        &Preferences::default(),
    );

    assert_eq!(test_support::disk_root(&workspace.runtime), None);

    workspace.replace_document(Document::default(), Some(first_path.clone()));
    assert_eq!(
        test_support::disk_root(&workspace.runtime),
        Some(document_cache_root(&first_path))
    );

    workspace.replace_document(Document::default(), Some(second_path.clone()));
    assert_eq!(workspace.open.path, Some(second_path.clone()));
    assert_eq!(
        test_support::disk_root(&workspace.runtime),
        Some(document_cache_root(&second_path))
    );
}
