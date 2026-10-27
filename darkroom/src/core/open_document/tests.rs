use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::TempDir;
use glam::Vec2;

use crate::core::document::Document;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::load_report::DocumentRepair;
use crate::core::document::viewport::Viewport;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::io::document;
use crate::core::io::document::error::DocumentLoadError;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::open_document::{Launched, Loaded, OpenDocument};
use crate::core::status_log::StatusLog;
use scenarium::{
    Binding, BindingEntry, BindingRejection, Compiler, ConstValue, DataType, EventPort, FuncId,
    FuncInput, FuncOutput, Graph, InputPort, Library, Node, NodeId, Repair, Subscription,
    SubscriptionRejection, Unresolved, internals,
};

#[test]
fn load_returns_the_document_error() {
    let path = PathBuf::from("not-a-document.json");

    let error = OpenDocument::load(path.clone(), &Library::default()).unwrap_err();

    assert!(matches!(
        error,
        DocumentLoadError::InvalidExtension { path: error_path } if error_path == path
    ));
}

#[test]
fn empty_document_has_the_main_graph_tab() {
    let open = OpenDocument::default();

    assert!(open.path.is_none());
    assert_eq!(open.document.layout.all_tabs().count(), 1);
}

/// A remembered document comes back as the open one; none remembered is an
/// empty document; an unloadable one degrades to an empty document, is
/// reported, and is forgotten.
#[test]
fn preferred_document_reopens_and_a_failed_load_forgets_the_path() {
    let dir = TempDir::new("darkroom-open-document");
    let path = dir.join("preferred.darkroom");
    document::save(&Document::default(), &path, SavedPaths::Relative).unwrap();
    let mut status = StatusLog::default();
    let launch = |remembered: Option<&Path>, status: &mut StatusLog| {
        let Launched {
            open,
            forgot_remembered,
        } = OpenDocument::open_at_launch(None, remembered, status, &Library::default());
        (open.path, forgot_remembered)
    };

    assert_eq!(
        launch(Some(&path), &mut status),
        (Some(path.clone()), false)
    );
    assert_eq!(launch(None, &mut status), (None, false));
    assert_eq!(status.lines().count(), 0, "neither reports a failure");

    assert_eq!(
        launch(Some(Path::new("invalid.json")), &mut status),
        (None, true)
    );
    assert_eq!(
        status.lines().collect::<Vec<_>>(),
        ["load failed: invalid.json must use the .darkroom or .darkroomz extension"]
    );
}

/// A command-line document outranks the remembered one, which stands; an
/// unloadable argument degrades to an empty document rather than to the
/// remembered one, and still leaves the remembered one standing — it is not
/// what failed.
#[test]
fn a_command_line_document_outranks_the_remembered_one_and_leaves_it_alone() {
    let dir = TempDir::new("darkroom-open-document-argument");
    let named = dir.join("named.darkroom");
    document::save(&Document::default(), &named, SavedPaths::Relative).unwrap();
    // An unloadable remembered file: a named document wins over it untried.
    let remembered = Path::new("remembered.darkroom");
    let mut status = StatusLog::default();

    // The `.` component is what proves the path went through
    // `std::path::absolute`: it is the one thing that strips it, and a
    // relative argument is made absolute by that same call. Compared as
    // `OsStr`, since `Path`'s own `==` skips `.` components and would pass
    // either way.
    let argument = dir.join(".").join("named.darkroom");
    assert_ne!(
        argument.as_os_str(),
        named.as_os_str(),
        "the fixture has to differ before the load"
    );
    let Launched {
        open,
        forgot_remembered,
    } = OpenDocument::open_at_launch(
        Some(argument),
        Some(remembered),
        &mut status,
        &Library::default(),
    );
    assert_eq!(
        open.path.as_deref().map(Path::as_os_str),
        Some(named.as_os_str())
    );
    assert!(!forgot_remembered);
    assert_eq!(status.lines().count(), 0);

    let missing = dir.join("missing.darkroom");
    let Launched {
        open,
        forgot_remembered,
    } = OpenDocument::open_at_launch(
        Some(missing.clone()),
        Some(remembered),
        &mut status,
        &Library::default(),
    );
    assert!(open.path.is_none());
    assert!(!forgot_remembered, "the remembered one is not what failed");
    // The tail is the OS's own wording for a missing file, so only the part
    // we compose is pinned.
    let lines = status.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one failure, not two: {lines:?}");
    let prefix = format!("load failed: {}: ", missing.display());
    assert!(lines[0].starts_with(&prefix), "got {:?}", lines[0]);

    // An empty argument has no absolute form, so it does not load, and says so.
    let mut status = StatusLog::default();
    let Launched {
        open,
        forgot_remembered,
    } = OpenDocument::open_at_launch(
        Some(PathBuf::new()),
        Some(remembered),
        &mut status,
        &Library::default(),
    );
    assert!(open.path.is_none() && !forgot_remembered);
    let lines = status.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("load failed: : "),
        "got {:?}",
        lines[0]
    );
}

/// A node keeps the ports it was authored against. Loaded against a library whose func moved its
/// ports, it leaves the graph as a stub with its wiring, the document is dirty, and the graph
/// compiles without it. Saved like that and loaded against the first library, it attaches again
/// with its wiring — except an edge whose input the graph bound while it was a stub.
#[test]
fn a_node_whose_ports_moved_loads_as_a_stub_and_attaches_again() {
    let dir = TempDir::new("darkroom-open-document-signature");
    let (first, stubbed) = (dir.join("first.darkroom"), dir.join("stubbed.darkroom"));
    let int_func = |name: &str| internals::stub_func(FuncId::unique(), name).pure();
    let src = int_func("src")
        .output(FuncOutput::new("out", DataType::Int))
        .build();
    let mid = int_func("mid")
        .input(FuncInput::required("in", DataType::Int))
        .output(FuncOutput::new("out", DataType::Int))
        .build();
    let dst = internals::stub_func(FuncId::unique(), "dst")
        .sink()
        .input(FuncInput::required("in", DataType::Int))
        .build();
    let mut library = Library::default();
    let mut changed = Library::default();
    let mut moved = mid.clone().into_builder();
    moved.inputs[0].data_type = DataType::String;
    for func in [&src, &dst] {
        library.add(func.clone());
        changed.add(func.clone());
    }
    library.add(mid.clone());
    changed.add(moved.build());

    // Fixed ids, so the record's bindings sort `mid` before `dst`.
    let [src_id, mid_id, dst_id] = [1, 2, 3].map(NodeId::from_u128);
    let mut graph = Graph::default();
    graph.insert(src_id, Node::from(&src));
    graph.insert(mid_id, Node::from(&mid));
    graph.insert(dst_id, Node::from(&dst));
    let into_mid = InputPort::new(mid_id, 0);
    let into_dst = InputPort::new(dst_id, 0);
    graph.set_input_binding(into_mid, Binding::wire(src_id, 0));
    graph.set_input_binding(into_dst, Binding::wire(mid_id, 0));
    document::save(&Document::from(graph), &first, SavedPaths::Relative).unwrap();

    let same = OpenDocument::load(first.clone(), &library).unwrap();
    assert!(same.report.is_empty());
    assert!(!same.open.dirty);

    let mut loaded = OpenDocument::load(first, &changed).unwrap();
    assert_eq!(
        loaded.report.repairs,
        [DocumentRepair::Stubbed {
            node_id: mid_id,
            reason: Unresolved::ChangedPorts,
        }]
    );
    assert!(loaded.open.dirty, "a save accepts the change");
    let document = &loaded.open.document;
    assert!(document.graph.find(mid_id).is_none());
    assert_eq!(
        document.stubs.get(mid_id).unwrap().bindings(),
        [
            BindingEntry {
                port: into_mid,
                binding: Binding::wire(src_id, 0),
            },
            BindingEntry {
                port: into_dst,
                binding: Binding::wire(mid_id, 0),
            },
        ],
        "the stub keeps every edge that touched it"
    );
    let compiled = Compiler::default().compile(&document.graph, &changed);
    assert!(compiled.contains(src_id) && compiled.contains(dst_id));

    // While `mid` is a stub, `dst` is wired to `src` instead.
    loaded
        .open
        .document
        .graph
        .set_input_binding(into_dst, Binding::wire(src_id, 0));
    loaded.open.save_to(&stubbed, SavedPaths::Relative).unwrap();

    let reattached = OpenDocument::load(stubbed, &library).unwrap();
    assert_eq!(
        reattached.report.repairs,
        [
            DocumentRepair::Reattached { node_id: mid_id },
            DocumentRepair::StubBindingDropped {
                node_id: mid_id,
                entry: BindingEntry {
                    port: into_dst,
                    binding: Binding::wire(mid_id, 0),
                },
            },
        ]
    );
    let document = &reattached.open.document;
    assert!(document.stubs.get(mid_id).is_none());
    assert_eq!(
        document.graph.bindings.get(&into_mid),
        Some(&Binding::wire(src_id, 0))
    );
    assert_eq!(
        document.graph.bindings.get(&into_dst),
        Some(&Binding::wire(src_id, 0)),
        "the input bound while `mid` was a stub keeps its binding"
    );
}

/// A drained batch whose every step joins a held gesture folds into that
/// gesture's entry. A frame that shares its batch with another edit is
/// recorded with it as a plain entry, which seals the gesture before it; a
/// camera move between two frames records nothing and seals nothing.
#[test]
fn only_a_batch_wholly_in_a_gesture_folds_into_it() {
    let fixture = DocumentFixture::sample();
    let (node, other) = (fixture.node(0), fixture.node(1));
    let DocumentFixture { document, library } = fixture;
    let mut open = OpenDocument::over(document);
    let start = open.document.main_view.node_placements[&node].pos;
    let mut queue = DocumentQueue::default();
    let gesture = queue.open_gesture();
    let members: Arc<[DragStart]> = Arc::from([DragStart {
        item: ItemId::Node(node),
        pos: start,
    }]);
    let frame = |x: f32| GraphIntent::MoveSelection {
        gesture,
        members: Arc::clone(&members),
        offset: Vec2::new(x, 0.0),
    };
    let mut drain = |open: &mut OpenDocument, intents: Vec<GraphIntent>| {
        queue.extend_graph(intents);
        assert_eq!(
            open.drain_requests(&mut queue, &library),
            EditFacts::NONE,
            "moving and selecting remeasure nothing"
        );
    };

    drain(&mut open, vec![frame(5.0)]);
    let zoomed = Viewport {
        pan: Vec2::new(30.0, 0.0),
        zoom: 2.0,
    };
    drain(&mut open, vec![GraphIntent::SetViewport { to: zoomed }]);
    drain(&mut open, vec![frame(9.0)]);
    drain(
        &mut open,
        vec![
            frame(12.0),
            GraphIntent::SetSelection {
                to: BTreeSet::from([ItemId::Node(other)]),
            },
        ],
    );
    drain(&mut open, vec![frame(20.0)]);

    let pos = |open: &OpenDocument| open.document.main_view.node_placements[&node].pos - start;
    assert_eq!(pos(&open), Vec2::new(20.0, 0.0));
    assert!(open.can_undo());
    assert_eq!(open.undo(), EditFacts::NONE);
    assert_eq!(
        pos(&open),
        Vec2::new(12.0, 0.0),
        "the frame after the batch"
    );
    assert!(open.can_undo());
    assert_eq!(open.undo(), EditFacts::NONE);
    assert_eq!(
        pos(&open),
        Vec2::new(9.0, 0.0),
        "the shared batch, as one entry"
    );
    assert!(open.document.main_view.selected.is_empty());
    assert!(open.can_undo());
    assert_eq!(open.undo(), EditFacts::NONE);
    assert_eq!(
        pos(&open),
        Vec2::ZERO,
        "the two frames before it, as one entry, the zoom between them none"
    );
    assert!(!open.can_undo());
    assert_eq!(
        open.document.main_view.viewport, zoomed,
        "no undo moves the camera"
    );
}

/// The graph revision moves on a change of the node set alone, which the
/// canvas sweeps its caches on, not on a move or a selection; an edit, its
/// undo and its redo each leave the document dirty.
#[test]
fn the_revision_follows_the_node_set_and_every_edit_dirties() {
    let fixture = DocumentFixture::sample();
    let node = fixture.node(0);
    let DocumentFixture { document, library } = fixture;
    let mut open = OpenDocument::over(document);
    let mut queue = DocumentQueue::default();
    let mut drain = |open: &mut OpenDocument, intent: GraphIntent| {
        queue.push_graph(intent);
        let _ = open.drain_requests(&mut queue, &library);
    };
    let start = open.graph_revision();

    let gesture = DocumentQueue::default().open_gesture();
    let placed = open.document.main_view.node_placements[&node].pos;
    drain(
        &mut open,
        GraphIntent::MoveSelection {
            gesture,
            members: Arc::from([DragStart {
                item: ItemId::Node(node),
                pos: placed,
            }]),
            offset: Vec2::new(5.0, 0.0),
        },
    );
    drain(
        &mut open,
        GraphIntent::SetSelection {
            to: BTreeSet::from([ItemId::Node(node)]),
        },
    );
    assert_eq!(
        open.graph_revision(),
        start,
        "a move and a selection keep it"
    );
    assert!(open.dirty, "a move is an edit");

    drain(&mut open, GraphIntent::RemoveNode { node_id: node });
    let removed = open.graph_revision();
    assert_ne!(removed, start, "a removal changes the node set");
    open.dirty = false;
    let _ = open.undo();
    assert!(open.dirty, "undo dirties");
    let restored = open.graph_revision();
    assert_ne!(restored, removed, "undo brings the node back");

    open.dirty = false;
    let _ = open.redo();
    assert!(open.dirty, "redo dirties");
    assert_ne!(open.graph_revision(), restored, "redo takes it out again");
}

/// Esc on a held gesture takes its whole entry back, an edit that joined it
/// too: a node spawned in a drag and moved by it is gone, and nothing is left
/// to undo. A cancel of a gesture an edit since sealed falls back on its move.
#[test]
fn a_cancel_takes_the_gesture_back_whole() {
    let fixture = DocumentFixture::sample();
    let node = fixture.node(0);
    let DocumentFixture { document, library } = fixture;
    let mut open = OpenDocument::over(document);
    let before = open.document.clone();
    let mut queue = DocumentQueue::default();
    let gesture = queue.open_gesture();
    let spawned = NodeId::unique();
    let spawn = GraphIntent::add_node(
        Vec2::new(40.0, 40.0),
        spawned,
        open.document.graph.find(node).unwrap().clone(),
    );
    let placed: Arc<[DragStart]> = Arc::from([DragStart {
        item: ItemId::Node(spawned),
        pos: Vec2::new(40.0, 40.0),
    }]);
    let frame = |x: f32| GraphIntent::MoveSelection {
        gesture,
        members: Arc::clone(&placed),
        offset: Vec2::new(x, 0.0),
    };
    let drain = |open: &mut OpenDocument, queue: &mut DocumentQueue| {
        let _ = open.drain_requests(queue, &library);
    };

    queue.extend_graph_in(gesture, [spawn]);
    drain(&mut open, &mut queue);
    queue.push_graph(frame(10.0));
    drain(&mut open, &mut queue);
    assert!(open.document.graph.find(spawned).is_some());
    queue.push_cancel(gesture, frame(0.0));
    drain(&mut open, &mut queue);
    assert_eq!(
        open.document, before,
        "the spawn and its move are both gone"
    );
    assert!(!open.can_undo(), "and no entry is left");

    // Sealed by an edit in between, the gesture is a packed entry the cancel
    // leaves alone: the fallback move puts the member back as its own entry.
    let start = open.document.main_view.node_placements[&node].pos;
    let gesture = queue.open_gesture();
    let members: Arc<[DragStart]> = Arc::from([DragStart {
        item: ItemId::Node(node),
        pos: start,
    }]);
    let moved = |x: f32| GraphIntent::MoveSelection {
        gesture,
        members: Arc::clone(&members),
        offset: Vec2::new(x, 0.0),
    };
    queue.push_graph(moved(10.0));
    drain(&mut open, &mut queue);
    queue.push_graph(GraphIntent::SetSelection {
        to: BTreeSet::from([ItemId::Node(node)]),
    });
    drain(&mut open, &mut queue);
    queue.push_cancel(gesture, moved(0.0));
    drain(&mut open, &mut queue);
    assert_eq!(open.document.main_view.node_placements[&node].pos, start);
    assert!(
        open.can_undo(),
        "the drag and the selection stay in the history"
    );
}

/// A file can carry wiring on a port or event its func does not declare — no
/// save or graph check refuses it — so the load holds every binding and
/// subscription to the library, the same rule the edit gate applies, and
/// removes what it refuses. Index 0 of each loads untouched; index 1, the
/// first past the probe's one input, output and event, is removed, reported,
/// and leaves the document dirty.
#[test]
fn wiring_the_library_refuses_is_removed_at_load() {
    let dir = TempDir::new("darkroom-open-document-declaration");
    let path = dir.join("wired.darkroom");
    let probe = internals::stub_func(FuncId::unique(), "probe")
        .input(FuncInput::optional("a", DataType::Int))
        .output(FuncOutput::new("out", DataType::Int))
        .event("tick", internals::stub_event())
        .build();
    let mut library = Library::default();
    library.add(probe.clone());
    let (producer, consumer) = (NodeId::unique(), NodeId::unique());
    let load = |wire: &dyn Fn(&mut Graph)| {
        let mut graph = Graph::default();
        graph.insert(producer, Node::from(&probe));
        graph.insert(consumer, Node::from(&probe));
        wire(&mut graph);
        document::save(&Document::from(graph), &path, SavedPaths::Relative).unwrap();
        OpenDocument::load(path.clone(), &library).unwrap()
    };

    let declared = load(&|graph| {
        graph.set_input_binding(InputPort::new(consumer, 0), Binding::wire(producer, 0));
        graph.subscribe(EventPort::new(producer, 0), consumer);
    });
    assert!(declared.report.is_empty());
    assert!(!declared.open.dirty);

    let refused = |loaded: &Loaded| {
        assert!(loaded.open.dirty);
        assert!(loaded.open.document.graph.bindings.is_empty());
        assert_eq!(loaded.open.document.graph.subscriptions().len(), 0);
        let [DocumentRepair::Graph(repair)] = &*loaded.report.repairs else {
            panic!("one graph repair, got {:?}", loaded.report.repairs);
        };
        repair.clone()
    };
    let constant = Binding::Const(ConstValue::from(1i64));
    let loaded =
        load(&|graph| graph.set_input_binding(InputPort::new(consumer, 1), constant.clone()));
    assert_eq!(
        refused(&loaded),
        Repair::RejectedBinding {
            port: InputPort::new(consumer, 1),
            binding: constant,
            rejection: BindingRejection::UndeclaredInput,
        }
    );
    let loaded = load(&|graph| {
        graph.set_input_binding(InputPort::new(consumer, 0), Binding::wire(producer, 1));
    });
    assert_eq!(
        refused(&loaded),
        Repair::RejectedBinding {
            port: InputPort::new(consumer, 0),
            binding: Binding::wire(producer, 1),
            rejection: BindingRejection::UndeclaredOutput,
        }
    );
    let event = EventPort::new(producer, 1);
    let loaded = load(&|graph| graph.subscribe(event, consumer));
    assert_eq!(
        refused(&loaded),
        Repair::RejectedSubscription {
            subscription: Subscription::new(event, consumer),
            rejection: SubscriptionRejection::UndeclaredEvent,
        }
    );
}

/// The sample project opens on the editor's own library with nothing to repair, as plain RON, its
/// paths naming the inputs beside it.
#[test]
fn the_sample_project_opens_clean() {
    use lens::MlModelPaths;

    use crate::core::runtime_library::RuntimeLibrary;

    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample/sample.darkroom");
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let Loaded { open, report } = OpenDocument::load(sample.clone(), library.cur()).unwrap();
    assert!(report.is_empty(), "{report:?}");
    let mut paths = Vec::new();
    open.document
        .for_each_path(|_, path| paths.push(PathBuf::from(path)));
    paths.sort();
    let inputs = sample.parent().unwrap().join("inputs");
    assert_eq!(
        paths,
        [inputs.join("full_example.fits"), inputs.join("lena.tiff")]
    );
}
