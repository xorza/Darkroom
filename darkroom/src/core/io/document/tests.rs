use common::TempDir;
use glam::Vec2;
use lens::MlModelPaths;
use scenarium::{
    Binding, BindingEntry, ConstValue, DetachedNode, DetachedNodeParts, InputPort, NodeId,
};

use std::io::Write as _;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::core::io::document::document_format::DOCUMENT_ENTRY;
use crate::core::runtime_library::RuntimeLibrary;

/// A document round-trips in each format, and a load reports the format it found. The plain
/// file is the document's pretty RON text; the zipped one holds that text as its one deflated
/// entry.
#[test]
fn document_round_trips_in_either_format() {
    let dir = TempDir::new("darkroom-document-roundtrip");
    // Populated, not `Document::default()`: an empty document has an empty
    // placement map, which would never exercise a `NodeId` as a key.
    let document = DocumentFixture::sample().document;
    assert!(
        !document.main_view.item_placements.is_empty(),
        "the fixture must place nodes for this to prove anything"
    );

    let plain = dir.join("plain.darkroom");
    let zipped = dir.join("zipped.darkroom");
    for (path, format) in [
        (&plain, DocumentFormat::Ron),
        (&zipped, DocumentFormat::ZippedRon),
    ] {
        save(&document, path, SavedPaths::Relative, format).expect("save document");
        let loaded = load(path).expect("load document");
        assert_eq!((loaded.document, loaded.format), (document.clone(), format));
    }

    let text = fs::read_to_string(&plain).unwrap();
    assert_eq!(ron::from_str::<Document>(&text).unwrap(), document);
    assert!(
        text.lines().count() > 1,
        "pretty, so a diff shows the line that changed"
    );

    let mut archive = ZipArchive::new(fs::open(&zipped).unwrap()).unwrap();
    assert_eq!(
        archive.len(),
        1,
        "the archive holds only the document entry"
    );
    let mut entry = archive.by_name(DOCUMENT_ENTRY).unwrap();
    assert_eq!(entry.compression(), CompressionMethod::Deflated);
    let mut zipped_text = String::new();
    entry.read_to_string(&mut zipped_text).unwrap();
    assert_eq!(zipped_text, text, "the entry is the plain file's text");
}

#[test]
fn document_extension_is_required_case_insensitively() {
    let document = Document::default();
    let dir = TempDir::new("darkroom-document-extension");
    let wrong = dir.join("wrong.json");
    assert!(matches!(
        save(&document, &wrong, SavedPaths::Relative, DocumentFormat::Ron).unwrap_err(),
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
    save(
        &document,
        &uppercase,
        SavedPaths::Relative,
        DocumentFormat::Ron,
    )
    .expect("uppercase extension is valid");
    assert_eq!(load(&uppercase).unwrap().document, document);
}

/// A file that is not a document's RON, and one that is missing, report their exact path, as do
/// the archives a zipped save never writes: one that is no ZIP past its signature, one with no
/// document entry, and one whose entry is not a document's RON.
#[test]
fn load_rejects_malformed_documents_and_archives() {
    let dir = TempDir::new("darkroom-document-rejects");
    let malformed = dir.join("malformed.darkroom");
    fs::write(&malformed, b"{").unwrap();
    assert!(
        matches!(
            load(&malformed).unwrap_err(),
            DocumentLoadError::Deserialize { path, .. } if path == malformed
        ),
        "malformed RON reports its file"
    );

    let missing = dir.join("missing.darkroom");
    assert!(matches!(
        load(&missing).unwrap_err(),
        DocumentLoadError::Open { path, .. } if path == missing
    ));

    let corrupt = dir.join("corrupt.darkroom");
    fs::write(&corrupt, b"PK not a zip archive").unwrap();
    assert!(matches!(
        load(&corrupt).unwrap_err(),
        DocumentLoadError::InvalidArchive { path, .. } if path == corrupt
    ));

    let other = dir.join("other.darkroom");
    write_test_archive(&other, "other.ron", b"()");
    assert!(matches!(
        load(&other).unwrap_err(),
        DocumentLoadError::DocumentEntryCount { path, count: 0 } if path == other
    ));

    let zipped_malformed = dir.join("zipped-malformed.darkroom");
    write_test_archive(&zipped_malformed, DOCUMENT_ENTRY, b"{");
    assert!(matches!(
        load(&zipped_malformed).unwrap_err(),
        DocumentLoadError::Deserialize { path, .. } if path == zipped_malformed
    ));
}

fn write_test_archive(path: &Path, name: &str, contents: &[u8]) {
    let mut archive = ZipWriter::new(fs::create(path).unwrap());
    archive
        .start_file(name, SimpleFileOptions::default())
        .unwrap();
    archive.write_all(contents).unwrap();
    archive.finish().unwrap();
}

/// The one size predicate a load and a save share admits the limit itself and
/// refuses the first byte over it.
#[test]
fn document_size_limit_rejects_the_first_byte_over_the_boundary() {
    assert!(fits(MAX_DOCUMENT_BYTES));
    assert!(!fits(MAX_DOCUMENT_BYTES + 1));
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

/// The paths the file at `path` stores, as its RON text holds them.
fn stored(path: &Path, nodes: [NodeId; 4]) -> Vec<String> {
    paths_of(
        &ron::from_str(&fs::read_to_string(path).unwrap()).unwrap(),
        nodes,
    )
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
    save(&document, &saved, SavedPaths::Relative, DocumentFormat::Ron).unwrap();
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
        load(&saved).unwrap().document,
        document,
        "read back where it was written"
    );

    let copy = second.join("scene.darkroom");
    fs::copy(&saved, &copy).unwrap();
    assert_eq!(
        paths_of(&load(&copy).unwrap().document, nodes),
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
    save(
        &document,
        &absolute,
        SavedPaths::Absolute,
        DocumentFormat::Ron,
    )
    .unwrap();
    assert_eq!(stored(&absolute, nodes), paths_of(&document, nodes));

    let below = first.join("below").join("scene.darkroom");
    save(&document, &below, SavedPaths::Relative, DocumentFormat::Ron).unwrap();
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
