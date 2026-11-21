//! Projects for a driver to open, made in code: what a test of the driver or of a client of it
//! starts from.

use std::path::{Path, PathBuf};

use lens::MlModelPaths;
use scenarium::{
    Binding, Compound, CompoundId, ConstValue, InputPort, Interface, Node, NodeId, PortId,
};

use crate::core::document::internals::{DocumentFixture, insert_compound};
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
    save_to(fixture, dir)
}

/// The value [`nested_preview`]'s preview shows.
pub const NESTED_PREVIEW_VALUE: i64 = 7;

/// A project whose main graph places the compound `Outer` twice, `Outer` places `Inner`, and
/// `Inner` holds a `Preview` of the constant [`NESTED_PREVIEW_VALUE`], saved as
/// `dir/scene.darkroom`; its path. Each instance of `Outer` is one preview in the program, three
/// ids down.
///
/// # Panics
///
/// When the save fails.
pub fn nested_preview(dir: &Path) -> PathBuf {
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let preview = library
        .cur()
        .by_name("Preview")
        .expect("the editor's library holds the preview")
        .clone();
    let mut fixture = DocumentFixture::default();
    fixture.library.add(preview.clone());

    let mut inner = Compound::new("Inner", Interface::default());
    let shown = inner.graph.add_func_node(&preview);
    inner.graph.set_input_binding(
        InputPort::new(shown, PortId::new(1)),
        Binding::Const(ConstValue::Int(NESTED_PREVIEW_VALUE)),
    );
    let inner_id = CompoundId::unique();
    let mut outer = Compound::new("Outer", Interface::default());
    outer
        .graph
        .insert(NodeId::unique(), Node::instance(inner_id));
    insert_compound(&mut fixture.document, inner_id, inner);
    let outer_id = CompoundId::unique();
    insert_compound(&mut fixture.document, outer_id, outer);
    for _ in 0..2 {
        fixture.add_instance(outer_id);
    }
    save_to(fixture, dir)
}

/// The value [`previews_beside_a_save`]'s `Shown` preview shows.
pub const SHOWN_PREVIEW_VALUE: i64 = 5;

/// A project whose main graph holds three previews of constants, `Shown`, `Off`, which is
/// disabled, and `Spare`, beside a Save Image fed nothing, saved as `dir/scene.darkroom`; its
/// path. A run of every sink fails on the save, so a run that leaves it alone ran previews alone.
///
/// # Panics
///
/// When the save fails.
pub fn previews_beside_a_save(dir: &Path) -> PathBuf {
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let func = |name| {
        library
            .cur()
            .by_name(name)
            .unwrap_or_else(|| panic!("the editor's library has no func {name}"))
            .clone()
    };
    let (preview, save) = (func("Preview"), func("Save Image"));
    let mut fixture = DocumentFixture::default();
    for (name, value, disabled) in [
        ("Shown", SHOWN_PREVIEW_VALUE, false),
        ("Off", SHOWN_PREVIEW_VALUE + 1, true),
        ("Spare", SHOWN_PREVIEW_VALUE + 2, false),
    ] {
        let id = fixture.add(&preview);
        let graph = fixture.document.graphs.main_mut();
        graph.set_input_binding(
            InputPort::new(id, PortId::new(1)),
            Binding::Const(ConstValue::Int(value)),
        );
        let node = graph.find_mut(id).expect("just added");
        node.name = Some(name.to_string());
        node.disabled = disabled;
    }
    fixture.add(&save);
    save_to(fixture, dir)
}

/// Save `fixture`'s document as `dir/scene.darkroom`; its path.
fn save_to(fixture: DocumentFixture, dir: &Path) -> PathBuf {
    let path = dir.join("scene.darkroom");
    OpenDocument::over(fixture.document)
        .save_to(&fixture.library, &path, SavedPaths::Relative)
        .expect("a fixture project saves");
    path
}
