use std::io::Write as _;
use std::sync::Arc;

use common::TempDir;
use glam::Vec2;
use lens::MlModelPaths;
use palantir::{DockDrop, DockOperation, SplitSide};
use scenarium::{
    Binding, Compound, CompoundId, ConstValue, EventPort, InputPort, Interface, Node, NodeId,
    OutputPort, PortId, ProgramId,
};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;
use crate::core::document::internals::{DocumentFixture, insert_compound};
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::TieAngle;
use crate::core::io::document::document_format::DOCUMENT_ENTRY;
use crate::core::preview_sink;
use crate::core::runtime_library::RuntimeLibrary;

/// A document round-trips in each format its extension names, and a load reads the form the
/// bytes show, so a misnamed file still opens. The plain file is the document's pretty RON text;
/// the zipped one holds that text as its one deflated entry. The document holds every kind of view state: placements keyed by `NodeId`, a wire
/// and a subscription through two ties in route order, a pinned angle, a selection of a node
/// and a tie, and a dock split in two groups.
#[test]
fn document_round_trips_in_either_format() {
    let dir = TempDir::new("darkroom-document-roundtrip");
    let mut fixture = DocumentFixture::sample();
    let [a, b] = [0.0, 220.0].map(|x| fixture.node_at(Vec2::new(x, 300.0)));
    let data = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(b, PortId::new(1)),
    );
    let event = fixture.subscribe(EventPort::new(a, PortId::new(1)), b);
    let first = fixture.tie(Vec2::new(80.0, 310.0), [data, event]);
    let second = fixture.tie(Vec2::new(140.0, 310.0), [data]);
    let viewer = fixture.add(&preview_sink::func(Arc::default()));
    let mut document = fixture.document;
    document.main_view.ties.get_mut(&second).unwrap().angle = TieAngle::Pinned(45.0);
    document.main_view.selected = BTreeSet::from([ItemId::Node(a), ItemId::Tie(first)]);
    let primary = document.layout.primary().id();
    for tab in [
        TabRef::Preferences,
        TabRef::ImageViewer(ProgramId::from(viewer)),
    ] {
        document.layout.apply(DockOperation::OpenTab { tab });
    }
    document.layout.apply(DockOperation::MoveTab {
        tab: TabRef::ImageViewer(ProgramId::from(viewer)),
        to: DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    });
    document.validate().unwrap();
    assert_eq!(
        document.main_view.routes[&data],
        Route::new([first, second]),
        "the data wire runs through both ties, in order"
    );

    let plain = dir.join("plain.darkroom");
    let zipped = dir.join("zipped.darkroomz");
    for path in [&plain, &zipped] {
        save(&document, &Library::default(), path, SavedPaths::Relative).expect("save document");
        assert_eq!(load(path).expect("load document").document, document);
    }
    // A load reads the form the bytes show, so a zipped file named as plain RON still opens.
    let misnamed = dir.join("misnamed.darkroom");
    fs::copy(&zipped, &misnamed).unwrap();
    assert_eq!(load(&misnamed).unwrap().document, document);

    let text = fs::read_to_string(&plain).unwrap();
    let written: FileOwned = ron::from_str(&text).unwrap();
    assert_eq!(written.document, document);
    assert!(
        written.manifest.is_empty(),
        "the fixture's library holds no func of its nodes"
    );
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
        save(&document, &Library::default(), &wrong, SavedPaths::Relative).unwrap_err(),
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
        &Library::default(),
        &uppercase,
        SavedPaths::Relative,
    )
    .expect("uppercase extension is valid");
    assert_eq!(load(&uppercase).unwrap().document, document);
    let uppercase_zipped = dir.join("uppercase.DARKROOMZ");
    save(
        &document,
        &Library::default(),
        &uppercase_zipped,
        SavedPaths::Relative,
    )
    .unwrap();
    assert!(
        fs::read(&uppercase_zipped).unwrap().starts_with(b"PK"),
        "the extension names the format without regard to case"
    );
    assert!(
        !fs::read(&uppercase).unwrap().starts_with(b"PK"),
        "a .darkroom file is plain RON"
    );
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

    let corrupt = dir.join("corrupt.darkroomz");
    fs::write(&corrupt, b"PK not a zip archive").unwrap();
    assert!(matches!(
        load(&corrupt).unwrap_err(),
        DocumentLoadError::InvalidArchive { path, .. } if path == corrupt
    ));

    let other = dir.join("other.darkroomz");
    write_test_archive(&other, "other.ron", b"()");
    assert!(matches!(
        load(&other).unwrap_err(),
        DocumentLoadError::DocumentEntryCount { path, count: 0 } if path == other
    ));

    let zipped_malformed = dir.join("zipped-malformed.darkroomz");
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
/// unset Load Image node, a ghost node whose record binds `ghost node`, and a compound whose Load Image node
/// is at `inner`. Its main graph's node ids, in that order.
fn paths_document(one: &str, many: &[&str], ghost: &str, inner: &str) -> (Document, [NodeId; 4]) {
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let func = |name: &str| library.cur().by_name(name).unwrap().clone();
    let mut fixture = DocumentFixture::default();
    let load = fixture.add(&func("Load Image"));
    let stack = fixture.add(&func("Stack Lights"));
    let unset = fixture.add(&func("Load Image"));
    let ghosted = fixture.ghost_node_at(Vec2::ZERO);
    fixture.bind_ghost_node(ghosted, ConstValue::FsPath(ghost.to_string()));
    let mut document = fixture.document;
    let bind = |document: &mut Document, node_id, value| {
        document.graphs.main_mut().bindings.insert(
            InputPort::new(node_id, PortId::new(1)),
            Binding::Const(value),
        );
    };
    bind(&mut document, load, ConstValue::FsPath(one.to_string()));
    bind(
        &mut document,
        stack,
        ConstValue::FsPaths(many.iter().map(ToString::to_string).collect()),
    );
    bind(&mut document, unset, ConstValue::FsPath(String::new()));
    let mut compound = Compound::new("inner", Interface::default());
    let nested = NodeId::unique();
    compound
        .graph
        .insert(nested, Node::from(&func("Load Image")));
    compound.graph.bindings.insert(
        InputPort::new(nested, PortId::new(1)),
        Binding::Const(ConstValue::FsPath(inner.to_string())),
    );
    insert_compound(&mut document, CompoundId::unique(), compound);
    (document, [load, stack, unset, ghosted])
}

/// Every path `document` holds, the ghost node's and then the compound's last, in the order
/// [`paths_document`] binds them.
fn paths_of(document: &Document, nodes: [NodeId; 4]) -> Vec<String> {
    let mut paths = Vec::new();
    for node_id in &nodes[..3] {
        let Binding::Const(value) =
            &document.graphs.main().bindings[&InputPort::new(*node_id, PortId::new(1))]
        else {
            panic!("a const binding");
        };
        paths.extend(value.as_fs_paths().unwrap().iter().cloned());
    }
    let kept = document
        .graphs
        .main()
        .ghost_wiring()
        .binding(InputPort::new(nodes[3], PortId::new(1)))
        .unwrap();
    let Binding::Const(value) = &kept.binding() else {
        panic!("a const binding");
    };
    paths.extend(value.as_fs_paths().unwrap().iter().cloned());
    let (_, compound) = document.graphs.compounds().next().unwrap();
    let Some(Binding::Const(value)) = compound.graph.bindings.values().next() else {
        panic!("a const binding");
    };
    paths.extend(value.as_fs_paths().unwrap().iter().cloned());
    paths
}

/// The paths the file at `path` stores, as its RON text holds them.
fn stored(path: &Path, nodes: [NodeId; 4]) -> Vec<String> {
    let file: FileOwned = ron::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    paths_of(&file.document, nodes)
}

/// A save writes each path inside the document's folder relative to it, `/` between its parts,
/// and the folder itself as `.`; a path outside, one reached through `..`, and an unset one stay
/// as they are, and a ghost node's path and a compound's are written as the main graph's are. A load reads them against the
/// folder of the file it reads: a copy in another folder names the copy's files. Saved absolute, the file holds the
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
    // Through `..`, one out of the folder and one back into it: both stay absolute.
    let climbs_out = first.join("..").join("x.tiff").display().to_string();
    let climbs_back = first
        .join("below")
        .join("..")
        .join("y.tiff")
        .display()
        .to_string();
    let (document, nodes) = paths_document(
        &inside(&first, "a.tiff"),
        &[
            &inside(&first, "b.fit"),
            &outside,
            &folder,
            &climbs_out,
            &climbs_back,
        ],
        &inside(&first, "s.tiff"),
        &inside(&first, "i.tiff"),
    );

    let saved = first.join("scene.darkroom");
    save(&document, &Library::default(), &saved, SavedPaths::Relative).unwrap();
    assert_eq!(
        stored(&saved, nodes),
        [
            "inputs/a.tiff",
            "inputs/b.fit",
            outside.as_str(),
            ".",
            climbs_out.as_str(),
            climbs_back.as_str(),
            "",
            "inputs/s.tiff",
            "inputs/i.tiff"
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
            climbs_out,
            climbs_back,
            String::new(),
            inside(&second, "s.tiff"),
            inside(&second, "i.tiff"),
        ]
    );

    let absolute = first.join("absolute.darkroom");
    save(
        &document,
        &Library::default(),
        &absolute,
        SavedPaths::Absolute,
    )
    .unwrap();
    assert_eq!(stored(&absolute, nodes), paths_of(&document, nodes));

    let below = first.join("below").join("scene.darkroom");
    save(&document, &Library::default(), &below, SavedPaths::Relative).unwrap();
    assert_eq!(
        stored(&below, nodes),
        paths_of(&document, nodes),
        "none inside `below`"
    );
}

/// A path in either system's absolute form stays byte for byte through a save and a load and
/// another save, on either system: a join would make `/data/a.fit` into `C:\data\a.fit` on Windows,
/// and `C:\data\b.fit` into one file name under the folder on Unix.
#[test]
fn paths_of_another_system_stay_as_they_are() {
    let dir = TempDir::new("darkroom-document-foreign");
    let (document, nodes) = paths_document(
        "/data/a.fit",
        &[r"C:\data\b.fit", "C:/data/c.fit"],
        r"\\server\share\d.fit",
        "/data/e.fit",
    );
    let want = [
        "/data/a.fit",
        r"C:\data\b.fit",
        "C:/data/c.fit",
        "",
        r"\\server\share\d.fit",
        "/data/e.fit",
    ];

    let first = dir.join("first.darkroom");
    save(&document, &Library::default(), &first, SavedPaths::Relative).unwrap();
    assert_eq!(stored(&first, nodes), want);

    let loaded = load(&first).unwrap().document;
    assert_eq!(paths_of(&loaded, nodes), want);

    let second = dir.join("second.darkroom");
    save(&loaded, &Library::default(), &second, SavedPaths::Relative).unwrap();
    assert_eq!(stored(&second, nodes), want);
}

/// A relative path that climbs out of the folder is listed, sorted, as `../x.tiff`,
/// `a/../../y.tiff` and the compound's `../i.tiff` do; one that climbs back in before leaving,
/// `a/../b.tiff`, is not, nor is an absolute one.
#[test]
fn relative_paths_that_leave_the_folder_are_listed() {
    let dir = TempDir::new("darkroom-document-leaving");
    let absolute = dir.join("z.tiff").display().to_string();
    let (document, _) = paths_document(
        "../x.tiff",
        &["a/../b.tiff", "a/../../y.tiff", &absolute],
        "inputs/s.tiff",
        "../i.tiff",
    );
    let path = dir.join("scene.darkroom");
    internals::write_unchecked(&document, &path);
    assert_eq!(
        paths_leaving_folder(&path).unwrap(),
        ["../i.tiff", "../x.tiff", "a/../../y.tiff"]
    );
}
