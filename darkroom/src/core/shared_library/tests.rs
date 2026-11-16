use common::{TempDir, fs};
use scenarium::{
    Binding, ConstValue, DataType, GhostBinding, GhostSource, GraphSet, InputDecl, InputPort,
    Interface, Library, Node, Numbered, PortId, Ports, SavedPort, Subgraph, SubgraphDigest,
    SubgraphId,
};

use crate::core::document::Document;
use crate::core::document::graph_view::NodePlacement;
use crate::core::document::internals::{DocumentFixture, insert_subgraph};
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::subgraph_intent::{SubgraphIntent, Update};
use crate::core::io::document;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::open_document::OpenDocument;
use crate::core::shared_library::{SyncState, published, scan};

/// A document sharing `subgraph` under `id`, or holding it unshared.
fn library_file(id: SubgraphId, subgraph: Subgraph, shared: bool) -> Document {
    let mut fixture = DocumentFixture::default();
    insert_subgraph(&mut fixture.document, id, subgraph);
    fixture.document.sides.get_mut(&id).unwrap().shared = shared;
    fixture.document
}

/// A scan lists each subgraph a file shares and not one it holds unshared; an id two files share
/// with equal digests is one entry, and with differing ones one per file; a file that does not
/// load is listed with its error, and the others still load.
#[test]
fn a_scan_lists_each_shared_definition_once_per_digest() {
    let dir = TempDir::new("darkroom-library-scan");
    let [id, hidden] = [1, 2].map(SubgraphId::from_u128);
    let denoise = Subgraph::new("Denoise", Interface::default());
    let mut changed = denoise.clone();
    changed.name = "Denoise, changed".to_string();
    let files = [
        ("a.darkroom", library_file(id, denoise.clone(), true)),
        ("b.darkroom", library_file(id, denoise, true)),
        ("c.darkroomz", library_file(id, changed, true)),
        (
            "d.darkroom",
            library_file(hidden, Subgraph::new("Hidden", Interface::default()), false),
        ),
    ];
    for (name, held) in &files {
        document::save(
            held,
            &Library::default(),
            &dir.join(name),
            SavedPaths::Relative,
        )
        .unwrap();
    }
    fs::write(dir.join("broken.darkroom"), b"not a document").unwrap();
    fs::write(dir.join("notes.txt"), b"ignored").unwrap();

    let library = scan(dir.path());
    let listed: Vec<(SubgraphId, &str, String)> = library
        .entries()
        .iter()
        .map(|entry| {
            let file = library
                .path(entry)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            (entry.id, entry.name.as_str(), file)
        })
        .collect();
    assert_eq!(
        listed,
        [
            (id, "Denoise", "a.darkroom".to_string()),
            (id, "Denoise, changed", "c.darkroomz".to_string()),
        ]
    );
    assert!(
        library.entry(id).is_none(),
        "two digests under one id name no one entry"
    );
    let failures: Vec<_> = library
        .failures()
        .iter()
        .map(|failure| {
            failure
                .path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(failures, ["broken.darkroom"]);
    let carried = library.carried(&library.entries()[0]);
    assert_eq!(carried.len(), 1);
    assert_eq!(
        carried[0].synced,
        Some(files[0].1.graphs.digest(id)),
        "a place remembers the digest it copied"
    );
}

/// The standing of a copy, from each arrangement of the three digests.
#[test]
fn a_copys_standing_reads_off_three_digests() {
    let digest = |name: &str| {
        let mut graphs = GraphSet::default();
        graphs.insert_subgraph(
            SubgraphId::from_u128(1),
            Subgraph::new(name, Interface::default()),
        );
        graphs.digest(SubgraphId::from_u128(1))
    };
    let [old, ours, theirs]: [SubgraphDigest; 3] = ["old", "ours", "theirs"].map(digest);
    let cases = [
        (Some(old), old, old, SyncState::InSync),
        (Some(old), old, theirs, SyncState::UpdateAvailable),
        (Some(old), ours, old, SyncState::ChangedHere),
        (Some(old), ours, theirs, SyncState::ChangedInBoth),
        (Some(old), ours, ours, SyncState::InSync),
        (None, ours, theirs, SyncState::ChangedInBoth),
        (None, ours, ours, SyncState::InSync),
    ];
    for (synced, local, library, state) in cases {
        assert_eq!(
            SyncState::of(synced, local, library),
            state,
            "{synced:?} {local:?} {library:?}"
        );
    }
}

fn inputs(ids: &[u32]) -> Interface {
    Interface::new(Ports {
        inputs: ids
            .iter()
            .map(|&id| {
                Numbered::new(
                    PortId::new(id),
                    InputDecl::optional(format!("in {id}"), DataType::Int),
                )
            })
            .collect(),
        outputs: vec![],
    })
}

/// A local copy that last shared `original`, with an instance holding a constant on input 1, and
/// a library file sharing a changed definition under the same id, input 1 gone: an update puts
/// the library's definition in place, keeps the constant the new interface does not hold as a ghost,
/// records the library's digest as shared, and one undo takes it all back. A publish of the
/// local copy writes it over the file's, marked shared, and the file's own instance loses what
/// the local interface does not hold.
#[test]
fn an_update_takes_the_librarys_definition_and_a_publish_gives_ours() {
    let dir = TempDir::new("darkroom-library-update");
    let id = SubgraphId::from_u128(1);
    let original = Subgraph::new("Denoise", inputs(&[1, 2]));
    let mut theirs = original.clone();
    theirs.name = "Denoise, theirs".to_string();
    theirs.interface = inputs(&[2]);
    let file = library_file(id, theirs.clone(), true);
    document::save(
        &file,
        &Library::default(),
        &dir.join("lib.darkroom"),
        SavedPaths::Relative,
    )
    .unwrap();
    let shared = scan(dir.path());
    let entry = shared.entry(id).unwrap().clone();

    let mut fixture = DocumentFixture::default();
    insert_subgraph(&mut fixture.document, id, original);
    let instance = fixture.add_instance(id);
    let port = InputPort::new(instance, PortId::new(1));
    fixture
        .document
        .graphs
        .main_mut()
        .bindings
        .insert(port, Binding::Const(ConstValue::Int(5)));
    let synced = fixture.document.graphs.digest(id);
    fixture.document.sides.get_mut(&id).unwrap().synced = Some(synced);
    let DocumentFixture { document, library } = fixture;
    let before = document.clone();
    let mut open = OpenDocument::over(document);
    let local_state = SyncState::of(
        Some(synced),
        open.document().graphs.digest(id),
        entry.digest,
    );
    assert_eq!(local_state, SyncState::UpdateAvailable);

    let mut queue = DocumentQueue::default();
    queue.push_subgraph(SubgraphIntent::Update(Box::new(Update {
        id,
        carried: shared.carried(&entry),
        manifest: shared.manifest(&entry).clone(),
    })));
    let facts = open.drain_requests(&mut queue, &library);
    assert_eq!((facts.dropped, facts.ghosted), (0, 1));
    let updated = open.document();
    assert_eq!(updated.graphs.subgraph(id), Some(&theirs));
    assert!(!updated.graphs.main().bindings.contains_key(&port));
    assert_eq!(
        updated.graphs.main().ghost_wiring().binding(port),
        Some(&GhostBinding {
            input: Some(SavedPort {
                name: "in 1".to_owned(),
                data_type: DataType::Int,
            }),
            source: GhostSource::Const(ConstValue::Int(5)),
        }),
        "the update keeps what you set on the port it removed, under the port's name"
    );
    assert_eq!(updated.sides[&id].synced, Some(entry.digest));
    updated.validate().unwrap();
    let _ = open.undo();
    assert_eq!(open.document(), &before, "one entry takes the update back");

    let mut file_with_instance = shared.document(&entry).clone();
    let theirs_instance = Node::instance(id);
    let placed = file_with_instance.graphs.main_mut().add(theirs_instance);
    file_with_instance
        .main_view
        .node_placements
        .insert(placed, NodePlacement::default());
    let held_port = InputPort::new(placed, PortId::new(2));
    file_with_instance
        .graphs
        .main_mut()
        .bindings
        .insert(held_port, Binding::Const(ConstValue::Int(9)));
    let mut ours = before;
    let mut changed = ours.graphs.subgraph(id).unwrap().clone();
    changed.name = "Denoise, ours".to_string();
    changed.interface = inputs(&[1]);
    ours.graphs.put_subgraph(id, changed.clone());
    let written = published(
        &file_with_instance,
        shared.manifest(&entry),
        &ours,
        id,
        &library,
    );
    assert_eq!(written.graphs.subgraph(id), Some(&changed));
    assert!(written.sides[&id].shared);
    assert_eq!(written.sides[&id].synced, Some(ours.graphs.digest(id)));
    assert!(
        !written.graphs.main().bindings.contains_key(&held_port),
        "the file's instance loses the port our interface does not hold"
    );
    written.validate().unwrap();
}

/// An update whose definition holds a node of a func this build lacks: the definition is parsed
/// against this library as it comes in, with its file's manifest, so the node is a ghost node
/// that holds what its func declared and the document stays one a compile takes.
#[test]
fn an_update_keeps_a_node_this_library_lacks_as_a_ghost_node() {
    use scenarium::{DetachReason, FuncId, OutputDecl, Unresolved, internals};

    let dir = TempDir::new("darkroom-library-update-lacking");
    let id = SubgraphId::from_u128(1);
    let original = Subgraph::new("Denoise", Interface::default());
    let theirs_only = internals::stub_func(FuncId::unique(), "sharpen")
        .output(1, OutputDecl::new("out", DataType::Int))
        .build();
    let mut their_library = Library::default();
    their_library.add(theirs_only.clone());
    let mut theirs = original.clone();
    let inside = theirs.graph.add(Node::from(&theirs_only));
    let file = library_file(id, theirs, true);
    document::save(
        &file,
        &their_library,
        &dir.join("lib.darkroom"),
        SavedPaths::Relative,
    )
    .unwrap();
    let shared = scan(dir.path());
    let entry = shared.entry(id).unwrap().clone();
    assert_eq!(
        shared
            .manifest(&entry)
            .func(theirs_only.id())
            .map(|saved| saved.name.as_str()),
        Some("sharpen")
    );

    let mut fixture = DocumentFixture::default();
    insert_subgraph(&mut fixture.document, id, original);
    let DocumentFixture { document, library } = fixture;
    let mut open = OpenDocument::over(document);
    let mut queue = DocumentQueue::default();
    queue.push_subgraph(SubgraphIntent::Update(Box::new(Update {
        id,
        carried: shared.carried(&entry),
        manifest: shared.manifest(&entry).clone(),
    })));
    let _ = open.drain_requests(&mut queue, &library);
    let updated = open.document();
    let held = &updated.graphs.subgraph(id).unwrap().graph;
    assert!(held.find(inside).is_none());
    let ghost = held.ghost_node(inside).expect("the node is kept");
    assert_eq!(
        ghost.reason,
        DetachReason::Unresolved(Unresolved::MissingFunc)
    );
    assert_eq!(
        ghost.saved.as_ref().map(|saved| saved.name.as_str()),
        Some("sharpen")
    );
    updated.validate().unwrap();
    updated.graphs.validate_with(&library).unwrap();
}

/// A library file is read as written, with no reconcile: a shared subgraph placing one that has
/// no side lists the shared one alone, and placing it copies both in, the sideless one laid out
/// in a row.
#[test]
fn a_subgraph_with_no_side_in_a_library_file_is_carried_in_a_row() {
    use crate::core::document::graph_view::GraphView;

    let dir = TempDir::new("darkroom-library-sideless");
    let [outer, inner] = [1, 2].map(SubgraphId::from_u128);
    let mut file = library_file(inner, Subgraph::new("Inner", Interface::default()), false);
    let mut enclosing = Subgraph::new("Outer", Interface::default());
    enclosing.graph.add(Node::instance(inner));
    insert_subgraph(&mut file, outer, enclosing);
    file.sides.get_mut(&outer).unwrap().shared = true;
    file.sides.remove(&inner);
    // Written by hand, as a save refuses a document with a subgraph and no side.
    document::internals::write_unchecked(&file, &dir.join("a.darkroom"));

    let library = scan(dir.path());
    let ids: Vec<SubgraphId> = library.entries().iter().map(|entry| entry.id).collect();
    assert_eq!(ids, [outer]);
    let carried = library.carried(&library.entries()[0]);
    let order: Vec<SubgraphId> = carried.iter().map(|definition| definition.id).collect();
    assert_eq!(order, [inner, outer], "each after those it places");
    assert_eq!(
        carried[0].view,
        GraphView::new(&carried[0].subgraph.graph),
        "a subgraph with no side shows in a row"
    );
}
