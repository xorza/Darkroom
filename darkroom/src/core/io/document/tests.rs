use common::TempDir;
use glam::Vec2;
use lens::MlModelPaths;
use scenarium::{
    Binding, BindingEntry, ConstValue, DetachedNode, DetachedNodeParts, InputPort, NodeId,
};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::core::runtime_library::RuntimeLibrary;

#[test]
fn document_round_trips_as_one_ron_entry() {
    let dir = TempDir::new("darkroom-document-roundtrip");
    let path = dir.join("roundtrip.darkroom");
    // Populated, not `Document::default()`: an empty document has an empty
    // placement map, which would never exercise a `NodeId` as a key.
    let document = DocumentFixture::sample().document;
    assert!(
        !document.main_view.item_placements.is_empty(),
        "the fixture must place nodes for this to prove anything"
    );

    save(&document, &path, SavedPaths::Relative).expect("save document");
    assert_eq!(load(&path).expect("load document"), document);

    let mut archive = ZipArchive::new(fs::open(path).unwrap()).unwrap();
    assert_eq!(archive.len(), 1, "archives contain only the document entry");
    let mut entry = archive.by_name(DOCUMENT_ENTRY).unwrap();
    assert_eq!(entry.compression(), CompressionMethod::Deflated);
    let mut encoded = String::new();
    entry.read_to_string(&mut encoded).unwrap();
    let decoded: Document = ron::from_str(&encoded).unwrap();
    assert_eq!(decoded, document, "the archive payload is plain RON");
}

#[test]
fn document_extension_is_required_case_insensitively() {
    let document = Document::default();
    let dir = TempDir::new("darkroom-document-extension");
    let wrong = dir.join("wrong.json");
    assert!(matches!(
        save(&document, &wrong, SavedPaths::Relative).unwrap_err(),
        DocumentSaveError::InvalidExtension { path } if path == wrong
    ));
    assert!(
        matches!(
            load(&wrong).unwrap_err(),
            DocumentLoadError::InvalidExtension { path } if path == wrong
        ),
        "load reports the exact rejected path"
    );

    let uppercase = dir.join("uppercase.DARKROOM");
    save(&document, &uppercase, SavedPaths::Relative).expect("uppercase extension is valid");
    assert_eq!(load(&uppercase).unwrap(), document);
}

#[test]
fn load_rejects_invalid_archives_and_malformed_documents() {
    let dir = TempDir::new("darkroom-document-rejects");
    let corrupt = dir.join("corrupt.darkroom");
    fs::write(&corrupt, b"not a zip archive").unwrap();
    assert!(
        matches!(
            load(&corrupt).unwrap_err(),
            DocumentLoadError::InvalidArchive { path, .. } if path == corrupt
        ),
        "corrupt ZIP reports the exact archive path"
    );

    let missing = dir.join("missing.darkroom");
    write_test_archive(&missing, "other.ron", b"()");
    assert!(
        matches!(
            load(&missing).unwrap_err(),
            DocumentLoadError::DocumentEntryCount { path, count: 0 } if path == missing
        ),
        "missing document entry reports its archive and exact count"
    );

    let malformed = dir.join("malformed.darkroom");
    write_test_archive(&malformed, DOCUMENT_ENTRY, b"{");
    assert!(
        matches!(
            load(&malformed).unwrap_err(),
            DocumentLoadError::DeserializeDocument { path, .. } if path == malformed
        ),
        "malformed RON reports its archive"
    );
}

/// The one size predicate a load and a save share admits the limit itself and
/// refuses the first byte over it.
#[test]
fn document_size_limit_rejects_the_first_byte_over_the_boundary() {
    assert!(fits(MAX_DOCUMENT_BYTES));
    assert!(!fits(MAX_DOCUMENT_BYTES + 1));
}

fn write_test_archive(path: &Path, name: &str, contents: &[u8]) {
    let file = fs::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(name, SimpleFileOptions::default())
        .unwrap();
    archive.write_all(contents).unwrap();
    archive.finish().unwrap();
}

/// A document of path inputs: a Load Image node at `one`, a Stack Lights node at `many`, an
/// unset Load Image node, and a stub whose record binds `stub`. Its node ids, in that order.
fn paths_document(one: &str, many: &[&str], stub: &str) -> (Document, [NodeId; 4]) {
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let func = |name: &str| library.cur().by_name(name).unwrap().clone();
    let mut fixture = DocumentFixture::default();
    let load = fixture.add(&func("Load Image"));
    let stack = fixture.add(&func("Stack Lights"));
    let unset = fixture.add(&func("Load Image"));
    let stubbed = fixture.stub_at(Vec2::ZERO);
    let mut document = fixture.document;
    let bind = |document: &mut Document, node_id, value| {
        document
            .graph
            .bindings
            .insert(InputPort::new(node_id, 0), Binding::Const(value));
    };
    bind(&mut document, load, ConstValue::FsPath(one.to_string()));
    bind(
        &mut document,
        stack,
        ConstValue::FsPaths(many.iter().map(ToString::to_string).collect()),
    );
    bind(&mut document, unset, ConstValue::FsPath(String::new()));
    let record = document.stubs.remove(stubbed).unwrap();
    document.stubs.insert(
        DetachedNode::new(DetachedNodeParts {
            node_id: stubbed,
            node: record.node().clone(),
            bindings: vec![BindingEntry {
                port: InputPort::new(stubbed, 0),
                binding: Binding::Const(ConstValue::FsPath(stub.to_string())),
            }],
            subscriptions: Vec::new(),
        })
        .unwrap(),
    );
    (document, [load, stack, unset, stubbed])
}

/// Every path `document` holds, the stub's last, in the order [`paths_document`] binds them.
fn paths_of(document: &Document, nodes: [NodeId; 4]) -> Vec<String> {
    let mut paths = Vec::new();
    for node_id in &nodes[..3] {
        let Binding::Const(value) = &document.graph.bindings[&InputPort::new(*node_id, 0)] else {
            panic!("a const binding");
        };
        paths.extend(value.as_fs_paths().unwrap().iter().cloned());
    }
    let stub = document.stubs.get(nodes[3]).unwrap();
    let Binding::Const(value) = &stub.bindings()[0].binding else {
        panic!("a const binding");
    };
    paths.extend(value.as_fs_paths().unwrap().iter().cloned());
    paths
}

/// The paths the archive at `path` stores, as its RON text holds them.
fn stored(path: &Path, nodes: [NodeId; 4]) -> Vec<String> {
    let mut archive = ZipArchive::new(fs::open(path).unwrap()).unwrap();
    let mut encoded = String::new();
    archive
        .by_name(DOCUMENT_ENTRY)
        .unwrap()
        .read_to_string(&mut encoded)
        .unwrap();
    paths_of(&ron::from_str(&encoded).unwrap(), nodes)
}

/// A save writes each path inside the document's folder relative to it, `/` between its parts,
/// and the folder itself as `.`; a path outside, and an unset one, stay as they are, and a stub's
/// path is written as the graph's are. A load reads them against the folder of the file it
/// reads: a copy in another folder names the copy's files. Saved absolute, the file holds the
/// document's own paths; and a save to a folder below leaves the old folder's paths absolute.
#[test]
fn paths_inside_the_folder_are_saved_relative_and_move_with_it() {
    let dir = TempDir::new("darkroom-document-paths");
    let (first, second) = (dir.join("first"), dir.join("second"));
    fs::create_dir_all(first.join("below")).unwrap();
    fs::create_dir_all(&second).unwrap();
    let inside = |folder: &Path, name: &str| folder.join("inputs").join(name).display().to_string();
    let outside = dir.join("elsewhere").join("c.fit").display().to_string();
    let folder = first.display().to_string();
    let (document, nodes) = paths_document(
        &inside(&first, "a.tiff"),
        &[&inside(&first, "b.fit"), &outside, &folder],
        &inside(&first, "s.tiff"),
    );

    let saved = first.join("scene.darkroom");
    save(&document, &saved, SavedPaths::Relative).unwrap();
    assert_eq!(
        stored(&saved, nodes),
        [
            "inputs/a.tiff",
            "inputs/b.fit",
            outside.as_str(),
            ".",
            "",
            "inputs/s.tiff"
        ]
    );
    assert_eq!(
        load(&saved).unwrap(),
        document,
        "read back where it was written"
    );

    let copy = second.join("scene.darkroom");
    fs::copy(&saved, &copy).unwrap();
    assert_eq!(
        paths_of(&load(&copy).unwrap(), nodes),
        [
            inside(&second, "a.tiff"),
            inside(&second, "b.fit"),
            outside,
            second.display().to_string(),
            String::new(),
            inside(&second, "s.tiff"),
        ]
    );

    let absolute = first.join("absolute.darkroom");
    save(&document, &absolute, SavedPaths::Absolute).unwrap();
    assert_eq!(stored(&absolute, nodes), paths_of(&document, nodes));

    let below = first.join("below").join("scene.darkroom");
    save(&document, &below, SavedPaths::Relative).unwrap();
    assert_eq!(
        stored(&below, nodes),
        paths_of(&document, nodes),
        "none inside `below`"
    );
}

/// A relative path that climbs out of the folder is listed, as `../x.tiff` and `a/../../y.tiff`
/// do; one that climbs back in before leaving, `a/../b.tiff`, is not, nor is an absolute one.
#[test]
fn relative_paths_that_leave_the_folder_are_listed() {
    let dir = TempDir::new("darkroom-document-leaving");
    let absolute = dir.join("z.tiff").display().to_string();
    let (document, _) = paths_document(
        "../x.tiff",
        &["a/../b.tiff", "a/../../y.tiff", &absolute],
        "inputs/s.tiff",
    );
    let path = dir.join("scene.darkroom");
    internals::write_unchecked(&document, &path);
    assert_eq!(
        paths_leaving_folder(&path).unwrap(),
        ["../x.tiff", "a/../../y.tiff"]
    );
}
