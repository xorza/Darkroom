use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use common::TempDir;
use glam::Vec2;
use lens::MlModelPaths;

use crate::core::document::Document;
use crate::core::document::graph_revision::GraphRevision;
use crate::core::document::graph_view::NodePlacement;
use crate::core::document::internals::{DocumentFixture, insert_subgraph};
use crate::core::document::item_id::ItemId;
use crate::core::document::load_report::{DocumentRepair, ForeignPath};
use crate::core::document::viewport::Viewport;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::io::document;
use crate::core::io::document::error::DocumentLoadError;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::open_document::{Launched, Loaded, OpenDocument};
use crate::core::runtime_library::RuntimeLibrary;
use crate::core::status_log::StatusLog;
use scenarium::{
    Binding, BindingRejection, Compiler, ConstValue, CustomTypeId, DataType, DetachReason, EnumId,
    EventPort, Func, FuncId, GhostBinding, GhostSource, Graph, GraphId, GraphSet, InputDecl,
    InputPort, Interface, InterfacePort, Library, Node, NodeAddr, NodeDecl, NodeId, Numbered,
    OutputDecl, OutputPort, PortId, PortList, Ports, ProgramId, Repair, SavedPort, Subgraph,
    SubgraphId, Subscription, SubscriptionRejection, UnknownType, Unresolved, internals,
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
    document::save(
        &Document::default(),
        &Library::default(),
        &path,
        SavedPaths::Relative,
    )
    .unwrap();
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
    document::save(
        &Document::default(),
        &Library::default(),
        &named,
        SavedPaths::Relative,
    )
    .unwrap();
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

/// A node names its func by id and its wiring names ports by id, so a func whose ports changed
/// still resolves: the node stays in the graph, the load holds each binding to the ports as they
/// are, and keeps as a ghost only what they refuse — here a wire into an input retyped to
/// `String`, and a wire from an output renumbered from 1 to 2.
#[test]
fn a_node_whose_ports_changed_keeps_the_wiring_they_admit() {
    let dir = TempDir::new("darkroom-open-document-changed-ports");
    let path = dir.join("first.darkroom");
    let chain = Chain::new();
    let mut changed = Library::default();
    let mut moved = chain.mid.clone().into_builder();
    moved.ports.inputs[0].data_type = DataType::String;
    moved.ports.outputs[0].id = PortId::new(2);
    changed.add(chain.src.clone());
    changed.add(moved.build());
    changed.add(chain.dst.clone());
    document::save(
        &Document::from(GraphSet::from(chain.graph.clone())),
        &chain.library(),
        &path,
        SavedPaths::Relative,
    )
    .unwrap();

    let same = OpenDocument::load(path.clone(), &chain.library()).unwrap();
    assert!(same.report.is_empty());
    assert!(!same.open.dirty);

    let loaded = OpenDocument::load(path, &changed).unwrap();
    assert_eq!(
        loaded.report.repairs,
        [
            DocumentRepair::Graph {
                graph: GraphId::Main,
                repair: Repair::GhostedBinding {
                    port: chain.into_mid,
                    binding: Binding::wire(chain.src_id, PortId::new(1)),
                    rejection: BindingRejection::WireMismatch.into(),
                },
            },
            DocumentRepair::Graph {
                graph: GraphId::Main,
                repair: Repair::GhostedBinding {
                    port: chain.into_dst,
                    binding: Binding::wire(chain.mid_id, PortId::new(1)),
                    rejection: BindingRejection::UndeclaredOutput.into(),
                },
            },
        ]
    );
    assert!(loaded.open.dirty, "a save accepts the change");
    let document = &loaded.open.document;
    assert!(document.graphs.main().find(chain.mid_id).is_some());
    assert!(document.graphs.main().ghost_node(chain.mid_id).is_none());
    assert!(document.graphs.main().bindings.is_empty());
    assert_eq!(
        document
            .graphs
            .main()
            .ghost_wiring()
            .bindings()
            .map(|(port, _)| port)
            .collect::<Vec<_>>(),
        BTreeSet::from([chain.into_mid, chain.into_dst])
            .into_iter()
            .collect::<Vec<_>>(),
        "each refused wire is kept as a ghost"
    );
    assert_eq!(
        document
            .graphs
            .main()
            .ghost_wiring()
            .binding(chain.into_dst),
        Some(&GhostBinding {
            input: None,
            source: GhostSource::Wire {
                output: OutputPort::new(chain.mid_id, PortId::new(1)),
                saved: Some(SavedPort {
                    name: "out".to_owned(),
                    data_type: DataType::Int,
                }),
            },
        }),
        "the manifest of the save names the output that left"
    );
}

/// A node whose func the library lacks leaves the graph as a ghost node, its wiring as ghosts,
/// the document is dirty, and the graph compiles without it. Saved like that and loaded against
/// a library that holds the func, it attaches again with its wiring — except an edge whose input
/// was bound while it was a ghost, which took the ghost's place.
#[test]
fn a_node_whose_func_is_missing_loads_as_a_ghost_node_and_attaches_again() {
    let dir = TempDir::new("darkroom-open-document-ghost");
    let (first, ghosted) = (dir.join("first.darkroom"), dir.join("ghosted.darkroom"));
    let chain = Chain::new();
    let Chain {
        src_id,
        mid_id,
        dst_id,
        into_mid,
        into_dst,
        ..
    } = chain;
    let mut lacking = Library::default();
    lacking.add(chain.src.clone());
    lacking.add(chain.dst.clone());
    document::save(
        &Document::from(GraphSet::from(chain.graph.clone())),
        &chain.library(),
        &first,
        SavedPaths::Relative,
    )
    .unwrap();

    let mut loaded = OpenDocument::load(first, &lacking).unwrap();
    assert_eq!(
        loaded.report.repairs,
        [DocumentRepair::Graph {
            graph: GraphId::Main,
            repair: Repair::GhostedNode {
                node_id: mid_id,
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            },
        }]
    );
    assert!(loaded.open.dirty, "a save accepts the change");
    let document = &loaded.open.document;
    assert!(document.graphs.main().find(mid_id).is_none());
    assert_eq!(
        document
            .graphs
            .main()
            .ghost_node(mid_id)
            .and_then(|ghost| ghost.saved.clone()),
        Some(NodeDecl::Func(&chain.mid).saved()),
        "the ghost node holds what the save's manifest says `mid` declared"
    );
    let mut kept: Vec<(InputPort, Binding)> = vec![
        (into_mid, Binding::wire(src_id, PortId::new(1))),
        (into_dst, Binding::wire(mid_id, PortId::new(1))),
    ];
    kept.sort_unstable_by_key(|(port, _)| *port);
    assert_eq!(
        document
            .graphs
            .main()
            .ghost_wiring()
            .bindings()
            .map(|(port, ghost)| (port, ghost.binding()))
            .collect::<Vec<_>>(),
        kept,
        "every edge that touched it is kept as a ghost"
    );
    let compiled = Compiler::default().compile(&document.graphs, &lacking);
    assert!(
        compiled.contains(ProgramId::from(src_id)) && compiled.contains(ProgramId::from(dst_id))
    );

    // While `mid` is a ghost node, `dst` is wired to `src` instead, in place of the ghost wire.
    loaded
        .open
        .document
        .graphs
        .main_mut()
        .set_input_binding(into_dst, Binding::wire(src_id, PortId::new(1)));
    loaded
        .open
        .save_to(&lacking, &ghosted, SavedPaths::Relative)
        .unwrap();
    let again = OpenDocument::load(ghosted.clone(), &lacking).unwrap();
    assert!(
        again.report.is_empty() && !again.open.dirty,
        "a ghost that stays one is no repair: {:?}",
        again.report
    );
    assert_eq!(again.open.document, loaded.open.document);

    let reattached = OpenDocument::load(ghosted, &chain.library()).unwrap();
    assert_eq!(
        reattached.report.repairs,
        [
            DocumentRepair::Graph {
                graph: GraphId::Main,
                repair: Repair::Reattached { node_id: mid_id },
            },
            DocumentRepair::Graph {
                graph: GraphId::Main,
                repair: Repair::RestoredBinding {
                    port: into_mid,
                    binding: Binding::wire(src_id, PortId::new(1)),
                },
            },
        ]
    );
    let document = &reattached.open.document;
    assert!(document.graphs.main().ghost_node(mid_id).is_none());
    assert!(document.graphs.main().ghost_wiring().is_empty());
    assert_eq!(
        document.graphs.main().bindings.get(&into_mid),
        Some(&Binding::wire(src_id, PortId::new(1)))
    );
    assert_eq!(
        document.graphs.main().bindings.get(&into_dst),
        Some(&Binding::wire(src_id, PortId::new(1))),
        "the input bound while `mid` was a ghost node keeps its binding"
    );
}

/// `src → mid → dst`, each carrying an `Int` through port 1, under fixed node ids so the
/// bindings sort `mid` before `dst`.
#[derive(Debug)]
struct Chain {
    src: Func,
    mid: Func,
    dst: Func,
    graph: Graph,
    src_id: NodeId,
    mid_id: NodeId,
    dst_id: NodeId,
    into_mid: InputPort,
    into_dst: InputPort,
}

impl Chain {
    fn new() -> Self {
        let int_func = |name: &str| internals::stub_func(FuncId::unique(), name).pure();
        let src = int_func("src")
            .output(1, OutputDecl::new("out", DataType::Int))
            .build();
        let mid = int_func("mid")
            .input(1, InputDecl::required("in", DataType::Int))
            .output(1, OutputDecl::new("out", DataType::Int))
            .build();
        let dst = internals::stub_func(FuncId::unique(), "dst")
            .sink()
            .input(1, InputDecl::required("in", DataType::Int))
            .build();
        let [src_id, mid_id, dst_id] = [1, 2, 3].map(NodeId::from_u128);
        let mut graph = Graph::default();
        graph.insert(src_id, Node::from(&src));
        graph.insert(mid_id, Node::from(&mid));
        graph.insert(dst_id, Node::from(&dst));
        let into_mid = InputPort::new(mid_id, PortId::new(1));
        let into_dst = InputPort::new(dst_id, PortId::new(1));
        graph.set_input_binding(into_mid, Binding::wire(src_id, PortId::new(1)));
        graph.set_input_binding(into_dst, Binding::wire(mid_id, PortId::new(1)));
        Self {
            src,
            mid,
            dst,
            graph,
            src_id,
            mid_id,
            dst_id,
            into_mid,
            into_dst,
        }
    }

    /// A library holding all three funcs.
    fn library(&self) -> Library {
        let mut library = Library::default();
        for func in [&self.src, &self.mid, &self.dst] {
            library.add(func.clone());
        }
        library
    }
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
    let members: Rc<[DragStart]> = Rc::from([DragStart {
        item: ItemId::Node(node),
        pos: start,
    }]);
    let frame = |x: f32| GraphIntent::MoveSelection {
        gesture,
        members: Rc::clone(&members),
        offset: Vec2::new(x, 0.0),
    };
    // Moving and selecting remeasure nothing; each step touches the main graph, and a camera
    // move, which records no step, touches none.
    let main = EditFacts {
        touched: Touched::Graph(GraphId::Main),
        ..EditFacts::NONE
    };
    let mut drain = |open: &mut OpenDocument, intents: Vec<GraphIntent>, facts: EditFacts| {
        queue.extend_graph(GraphId::Main, intents);
        assert_eq!(open.drain_requests(&mut queue, &library), facts);
    };

    drain(&mut open, vec![frame(5.0)], main);
    let zoomed = Viewport {
        pan: Vec2::new(30.0, 0.0),
        zoom: 2.0,
    };
    drain(
        &mut open,
        vec![GraphIntent::SetViewport { to: zoomed }],
        EditFacts::NONE,
    );
    drain(&mut open, vec![frame(9.0)], main);
    drain(
        &mut open,
        vec![
            frame(12.0),
            GraphIntent::SetSelection {
                to: BTreeSet::from([ItemId::Node(other)]),
            },
        ],
        main,
    );
    drain(&mut open, vec![frame(20.0)], main);

    let pos = |open: &OpenDocument| open.document.main_view.node_placements[&node].pos - start;
    assert_eq!(pos(&open), Vec2::new(20.0, 0.0));
    assert!(open.can_undo());
    assert_eq!(open.undo(), main);
    assert_eq!(
        pos(&open),
        Vec2::new(12.0, 0.0),
        "the frame after the batch"
    );
    assert!(open.can_undo());
    assert_eq!(open.undo(), main);
    assert_eq!(
        pos(&open),
        Vec2::new(9.0, 0.0),
        "the shared batch, as one entry"
    );
    assert!(open.document.main_view.selected.is_empty());
    assert!(open.can_undo());
    assert_eq!(open.undo(), main);
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

/// A graph's revision moves on a change of its node set alone, which its
/// canvas sweeps its caches on, not on a move or a selection, nor on an edit of
/// another graph; an edit, its undo and its redo each leave the document dirty.
#[test]
fn the_revision_follows_the_node_set_and_every_edit_dirties() {
    let fixture = DocumentFixture::sample();
    let node = fixture.node(0);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let other = GraphId::Subgraph(SubgraphId::from_u128(5));
    insert_subgraph(
        &mut document,
        SubgraphId::from_u128(5),
        Subgraph::new("other", Interface::default()),
    );
    let mut open = OpenDocument::over(document);
    let mut queue = DocumentQueue::default();
    let mut drain = |open: &mut OpenDocument, intent: GraphIntent| {
        queue.push_graph(GraphId::Main, intent);
        let _ = open.drain_requests(&mut queue, &library);
    };
    let start = open.graph_revision(GraphId::Main);

    let gesture = DocumentQueue::default().open_gesture();
    let placed = open.document.main_view.node_placements[&node].pos;
    drain(
        &mut open,
        GraphIntent::MoveSelection {
            gesture,
            members: Rc::from([DragStart {
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
        open.graph_revision(GraphId::Main),
        start,
        "a move and a selection keep it"
    );
    assert!(open.dirty, "a move is an edit");

    drain(&mut open, GraphIntent::RemoveNode { node_id: node });
    let removed = open.graph_revision(GraphId::Main);
    assert_ne!(removed, start, "a removal changes the node set");
    assert_eq!(
        open.graph_revision(other),
        GraphRevision::default(),
        "an edit of the main graph leaves another graph's revision"
    );
    open.dirty = false;
    let _ = open.undo();
    assert!(open.dirty, "undo dirties");
    let restored = open.graph_revision(GraphId::Main);
    assert_ne!(restored, removed, "undo brings the node back");

    open.dirty = false;
    let _ = open.redo();
    assert!(open.dirty, "redo dirties");
    assert_ne!(
        open.graph_revision(GraphId::Main),
        restored,
        "redo takes it out again"
    );
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
        open.document.graphs.main().find(node).unwrap().clone(),
    );
    let placed: Rc<[DragStart]> = Rc::from([DragStart {
        item: ItemId::Node(spawned),
        pos: Vec2::new(40.0, 40.0),
    }]);
    let frame = |x: f32| GraphIntent::MoveSelection {
        gesture,
        members: Rc::clone(&placed),
        offset: Vec2::new(x, 0.0),
    };
    let drain = |open: &mut OpenDocument, queue: &mut DocumentQueue| {
        let _ = open.drain_requests(queue, &library);
    };

    queue.extend_graph_in(gesture, GraphId::Main, [spawn]);
    drain(&mut open, &mut queue);
    queue.push_graph(GraphId::Main, frame(10.0));
    drain(&mut open, &mut queue);
    assert!(open.document.graphs.main().find(spawned).is_some());
    queue.push_cancel(gesture, GraphId::Main, frame(0.0));
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
    let members: Rc<[DragStart]> = Rc::from([DragStart {
        item: ItemId::Node(node),
        pos: start,
    }]);
    let moved = |x: f32| GraphIntent::MoveSelection {
        gesture,
        members: Rc::clone(&members),
        offset: Vec2::new(x, 0.0),
    };
    queue.push_graph(GraphId::Main, moved(10.0));
    drain(&mut open, &mut queue);
    queue.push_graph(
        GraphId::Main,
        GraphIntent::SetSelection {
            to: BTreeSet::from([ItemId::Node(node)]),
        },
    );
    drain(&mut open, &mut queue);
    queue.push_cancel(gesture, GraphId::Main, moved(0.0));
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
/// removes what it refuses. Port 1 of each loads untouched; port 2, which the
/// probe's one input, output and event leave undeclared, is removed, reported,
/// and leaves the document dirty.
#[test]
fn wiring_the_library_refuses_is_removed_at_load() {
    let dir = TempDir::new("darkroom-open-document-declaration");
    let path = dir.join("wired.darkroom");
    let probe = internals::stub_func(FuncId::unique(), "probe")
        .input(1, InputDecl::optional("a", DataType::Int))
        .output(1, OutputDecl::new("out", DataType::Int))
        .event(1, "tick", internals::stub_event())
        .build();
    let mut library = Library::default();
    library.add(probe.clone());
    let (producer, consumer) = (NodeId::unique(), NodeId::unique());
    let load = |wire: &dyn Fn(&mut Graph)| {
        let mut graph = Graph::default();
        graph.insert(producer, Node::from(&probe));
        graph.insert(consumer, Node::from(&probe));
        wire(&mut graph);
        document::save(
            &Document::from(GraphSet::from(graph)),
            &Library::default(),
            &path,
            SavedPaths::Relative,
        )
        .unwrap();
        OpenDocument::load(path.clone(), &library).unwrap()
    };

    let declared = load(&|graph| {
        graph.set_input_binding(
            InputPort::new(consumer, PortId::new(1)),
            Binding::wire(producer, PortId::new(1)),
        );
        graph.subscribe(EventPort::new(producer, PortId::new(1)), consumer);
    });
    assert!(declared.report.is_empty());
    assert!(!declared.open.dirty);

    let refused = |loaded: &Loaded| {
        assert!(loaded.open.dirty);
        assert!(loaded.open.document.graphs.main().bindings.is_empty());
        assert_eq!(loaded.open.document.graphs.main().subscriptions().len(), 0);
        let [
            DocumentRepair::Graph {
                graph: GraphId::Main,
                repair,
            },
        ] = &*loaded.report.repairs
        else {
            panic!("one graph repair, got {:?}", loaded.report.repairs);
        };
        repair.clone()
    };
    let constant = Binding::Const(ConstValue::from(1i64));
    let loaded = load(&|graph| {
        graph.set_input_binding(InputPort::new(consumer, PortId::new(2)), constant.clone());
    });
    assert_eq!(
        refused(&loaded),
        Repair::GhostedBinding {
            port: InputPort::new(consumer, PortId::new(2)),
            binding: constant,
            rejection: BindingRejection::UndeclaredInput.into(),
        }
    );
    let loaded = load(&|graph| {
        graph.set_input_binding(
            InputPort::new(consumer, PortId::new(1)),
            Binding::wire(producer, PortId::new(2)),
        );
    });
    assert_eq!(
        refused(&loaded),
        Repair::GhostedBinding {
            port: InputPort::new(consumer, PortId::new(1)),
            binding: Binding::wire(producer, PortId::new(2)),
            rejection: BindingRejection::UndeclaredOutput.into(),
        }
    );
    let event = EventPort::new(producer, PortId::new(2));
    let loaded = load(&|graph| graph.subscribe(event, consumer));
    assert_eq!(
        refused(&loaded),
        Repair::GhostedSubscription {
            subscription: Subscription::new(event, consumer),
            rejection: SubscriptionRejection::UndeclaredEvent.into(),
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

/// A load reports the path that is absolute on the other system alone, and it leaves the document
/// clean, as it changed nothing: each of these two paths is foreign on exactly one system.
#[test]
fn a_path_of_another_system_is_reported_and_leaves_the_document_clean() {
    let dir = TempDir::new("darkroom-open-foreign");
    let path = dir.join("scene.darkroom");
    let library = RuntimeLibrary::new(&MlModelPaths::default());
    let load_image = library.cur().by_name("Load Image").unwrap().clone();
    let mut fixture = DocumentFixture::default();
    let bound = [
        (fixture.add(&load_image), "/data/a.fit"),
        (fixture.add(&load_image), r"C:\data\a.fit"),
    ];
    let mut document = fixture.document;
    for (node_id, value) in bound {
        document.graphs.main_mut().bindings.insert(
            InputPort::new(node_id, PortId::new(1)),
            Binding::Const(ConstValue::FsPath(value.to_string())),
        );
    }
    document::save(&document, &Library::default(), &path, SavedPaths::Relative).unwrap();

    let loaded = OpenDocument::load(path, library.cur()).unwrap();

    let foreign: Vec<_> = bound
        .into_iter()
        .filter(|(_, value)| !Path::new(value).is_absolute())
        .map(|(node_id, value)| ForeignPath {
            node: NodeAddr::new(GraphId::Main, node_id),
            path: value.to_string(),
        })
        .collect();
    assert_eq!(
        foreign.len(),
        1,
        "one is absolute here and the other is not"
    );
    assert_eq!(loaded.report.foreign_paths, foreign);
    assert!(loaded.report.repairs.is_empty());
    assert!(!loaded.open.dirty);
    assert_eq!(
        loaded.report.to_string(),
        "the document names 1 path(s) this system cannot read as written"
    );
}

/// A file whose two subgraphs place each other opens with the loop broken at one instance, kept
/// as a ghost node whose reason is `Recursive`, and opens the same way each time.
#[test]
fn a_loop_between_two_subgraphs_opens_with_one_recursive_ghost_node_every_time() {
    let dir = TempDir::new("darkroom-open-loop");
    let path = dir.join("scene.darkroom");
    let [a, b] = [1, 2].map(SubgraphId::from_u128);
    let mut fixture = DocumentFixture::default();
    for id in [a, b] {
        insert_subgraph(
            &mut fixture.document,
            id,
            Subgraph::new(format!("{id:?}"), Interface::default()),
        );
    }
    // Each places the other, past the edit gate that refuses the second placement.
    for (graph, placed, node_id) in [(a, b, 10), (b, a, 20)] {
        let node = Node::instance(placed);
        let slot = fixture.document.slot_mut(GraphId::Subgraph(graph));
        slot.graph.insert(NodeId::from_u128(node_id), node);
        slot.view
            .node_placements
            .insert(NodeId::from_u128(node_id), NodePlacement::default());
    }
    fixture.add_instance(a);
    document::save(
        &fixture.document,
        &Library::default(),
        &path,
        SavedPaths::Relative,
    )
    .unwrap();

    let first = OpenDocument::load(path.clone(), &Library::default()).unwrap();
    let ghosted: Vec<(GraphId, NodeId, DetachReason)> = first
        .report
        .repairs
        .iter()
        .filter_map(|repair| match repair {
            DocumentRepair::Graph {
                graph,
                repair: Repair::GhostedNode { node_id, reason },
            } => Some((*graph, *node_id, *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(ghosted.len(), 1, "{:?}", first.report.repairs);
    assert_eq!(ghosted[0].2, DetachReason::Recursive);
    let second = OpenDocument::load(path, &Library::default()).unwrap();
    assert_eq!(second.report.repairs, first.report.repairs);
    assert_eq!(second.open.document(), first.open.document());
}

/// A subgraph whose interface names a type this build does not register loads as written, an
/// enum's default and an instance's constant of it included: the load reports each type and each
/// port that names it, and leaves the document clean. A repair inside a subgraph names the
/// subgraph it was made in.
#[test]
fn a_subgraphs_unknown_type_is_reported_and_its_repairs_name_the_subgraph() {
    let dir = TempDir::new("darkroom-open-unknown-type");
    let path = dir.join("scene.darkroom");
    let unknown = DataType::Custom(CustomTypeId::from_u128(77));
    let unknown_enum = DataType::Enum(EnumId::from_u128(78));
    let [port, choice] = [1, 2].map(PortId::new);
    let interface = Interface::new(Ports {
        inputs: vec![
            Numbered::new(port, InputDecl::optional("mystery", unknown)),
            Numbered::new(
                choice,
                InputDecl::optional("filter", unknown_enum)
                    .default(ConstValue::Enum("Lanczos".to_string())),
            ),
        ],
        outputs: vec![],
    });
    let id = SubgraphId::from_u128(5);
    let mut fixture = DocumentFixture::default();
    insert_subgraph(
        &mut fixture.document,
        id,
        Subgraph::new("holder", interface),
    );
    let instance = fixture.add_instance(id);
    let mut document = fixture.document;
    document.graphs.main_mut().bindings.insert(
        InputPort::new(instance, choice),
        Binding::Const(ConstValue::Enum("Bicubic".to_string())),
    );
    document::save(&document, &Library::default(), &path, SavedPaths::Relative).unwrap();

    let loaded = OpenDocument::load(path.clone(), &Library::default()).unwrap();
    let named = |data_type, port| UnknownType {
        data_type,
        ports: vec![InterfacePort {
            subgraph: id,
            list: PortList::Inputs,
            port,
        }],
    };
    assert_eq!(
        loaded.report.unknown_types,
        [named(unknown, port), named(unknown_enum, choice)]
    );
    assert_eq!(loaded.open.document(), &document, "kept as written");
    assert!(loaded.report.repairs.is_empty());
    assert!(!loaded.open.dirty, "a type kept as written is no change");

    let ghost = NodeId::from_u128(9);
    let slot = document.slot_mut(GraphId::Subgraph(id));
    slot.graph
        .insert(ghost, internals::unresolved_node(FuncId::from_u128(99)));
    slot.view
        .node_placements
        .insert(ghost, NodePlacement::default());
    document::save(&document, &Library::default(), &path, SavedPaths::Relative).unwrap();
    let loaded = OpenDocument::load(path, &Library::default()).unwrap();
    assert_eq!(
        loaded.report.repairs,
        [DocumentRepair::Graph {
            graph: GraphId::Subgraph(id),
            repair: Repair::GhostedNode {
                node_id: ghost,
                reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            },
        }]
    );
    assert_eq!(
        loaded.report.repairs[0].to_string(),
        format!(
            "in subgraph {id:?}, kept {ghost:?} as a ghost node: the library holds no func under \
             its id"
        )
    );
    assert!(loaded.open.dirty);
}
