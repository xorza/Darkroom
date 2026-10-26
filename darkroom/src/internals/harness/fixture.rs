//! Projects for a harness to open, made in code: what a test of the harness or of a client of it
//! starts from.

use std::path::{Path, PathBuf};

use lens::MlModelPaths;

use crate::core::document::internals::DocumentFixture;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::open_document::OpenDocument;
use crate::core::runtime_library::RuntimeLibrary;

/// A project of one node of each func named, from the editor's own library, saved as
/// `dir/scene.darkroom`; its path.
///
/// # Panics
///
/// When the library has no func of a name, or the save fails.
pub fn project(dir: &Path, funcs: &[&str]) -> PathBuf {
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let mut fixture = DocumentFixture::default();
    for name in funcs {
        fixture.add(
            library
                .cur()
                .by_name(name)
                .unwrap_or_else(|| panic!("the editor's library has no func {name}")),
        );
    }
    let path = dir.join("scene.darkroom");
    OpenDocument::over(fixture.document)
        .save_to(&path, SavedPaths::Relative)
        .expect("a fixture project saves");
    path
}
