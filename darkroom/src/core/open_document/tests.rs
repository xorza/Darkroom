use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::TempDir;
use glam::Vec2;

use crate::core::document::Document;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::load_report::DocumentRepair;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::edit::relayout::Relayout;
use crate::core::io::document;
use crate::core::io::document::document_format::DocumentFormat;
use crate::core::io::document::error::DocumentLoadError;
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::io::preferences::Preferences;
use crate::core::open_document::{LaunchSource, Loaded, OpenDocument};
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

#[test]
fn preferred_document_reopens_and_a_failed_load_forgets_the_path() {
    let dir = TempDir::new("darkroom-open-document");
    let path = dir.join("preferred.darkroom");
    document::save(
        &Document::default(),
        &path,
        SavedPaths::Relative,
        DocumentFormat::Ron,
    )
    .unwrap();
    let mut preferences = Preferences {
        document_path: Some(path.clone()),
        ..Preferences::default()
    };
    let mut status = StatusLog::default();

    // A remembered path with reopening on comes back as the open document.
    let open = OpenDocument::open_at_launch(
        LaunchSource {
            argument: None,
            preferences: &mut preferences,
        },
        &mut status,
        &Library::default(),
    );
    assert_eq!(open.path, Some(path.clone()));

    // Reopening off: empty document, but the path stays remembered.
    preferences.load_last_document = false;
    let open = OpenDocument::open_at_launch(
        LaunchSource {
            argument: None,
            preferences: &mut preferences,
        },
        &mut status,
        &Library::default(),
    );
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, Some(path));
    assert_eq!(status.lines().count(), 0, "neither path reports a failure");

    // An unloadable path degrades to an empty document and is forgotten.
    preferences.load_last_document = true;
    preferences.document_path = Some("invalid.json".into());
    let open = OpenDocument::open_at_launch(
        LaunchSource {
            argument: None,
            preferences: &mut preferences,
        },
        &mut status,
        &Library::default(),
    );
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, None);
    assert_eq!(
        status.lines().collect::<Vec<_>>(),
        ["load failed: invalid.json must use the .darkroom extension"]
    );
}

#[test]
fn a_command_line_document_outranks_the_remembered_one_and_leaves_it_alone() {
    let dir = TempDir::new("darkroom-open-document-argument");
    let named = dir.join("named.darkroom");
    document::save(
        &Document::default(),
        &named,
        SavedPaths::Relative,
        DocumentFormat::Ron,
    )
    .unwrap();
    let remembered = PathBuf::from("remembered.darkroom");
    let mut preferences = Preferences {
        document_path: Some(remembered.clone()),
        // Off, and pointed at an unloadable file: a named document wins over
        // both, and the preferences are untouched on the way through.
        load_last_document: false,
        ..Preferences::default()
    };
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
    let open = OpenDocument::open_at_launch(
        LaunchSource {
            argument: Some(argument),
            preferences: &mut preferences,
        },
        &mut status,
        &Library::default(),
    );
    assert_eq!(
        open.path.as_deref().map(Path::as_os_str),
        Some(named.as_os_str())
    );
    assert_eq!(preferences.document_path, Some(remembered.clone()));
    assert_eq!(status.lines().count(), 0);

    // An unloadable argument degrades to an empty document rather than to the
    // remembered one, and still leaves the remembered path standing — it is
    // not what failed.
    let missing = dir.join("missing.darkroom");
    let open = OpenDocument::open_at_launch(
        LaunchSource {
            argument: Some(missing.clone()),
            preferences: &mut preferences,
        },
        &mut status,
        &Library::default(),
    );
    assert!(open.path.is_none());
    assert_eq!(preferences.document_path, Some(remembered));
    // The tail is the OS's own wording for a missing file, so only the part
    // we compose is pinned.
    let lines = status.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one failure, not two: {lines:?}");
    let prefix = format!("load failed: {}: ", missing.display());
    assert!(lines[0].starts_with(&prefix), "got {:?}", lines[0]);
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
    document::save(
        &Document::from(graph),
        &first,
        SavedPaths::Relative,
        DocumentFormat::Ron,
    )
    .unwrap();

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
    loaded
        .open
        .save_to(&stubbed, SavedPaths::Relative, DocumentFormat::Ron)
        .unwrap();

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

/// A drained batch whose one step is a frame of a held gesture folds into
/// that gesture's entry. A frame that shares its batch with another edit is
/// recorded with it as a plain entry, which seals the gesture before it.
#[test]
fn only_a_one_step_batch_is_a_gesture_frame() {
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
            Relayout::NotNeeded,
            "moving and selecting remeasure nothing"
        );
    };

    drain(&mut open, vec![frame(5.0)]);
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
    assert_eq!(open.undo(), Relayout::NotNeeded);
    assert_eq!(
        pos(&open),
        Vec2::new(12.0, 0.0),
        "the frame after the batch"
    );
    assert!(open.can_undo());
    assert_eq!(open.undo(), Relayout::NotNeeded);
    assert_eq!(
        pos(&open),
        Vec2::new(9.0, 0.0),
        "the shared batch, as one entry"
    );
    assert!(open.document.main_view.selected.is_empty());
    assert!(open.can_undo());
    assert_eq!(open.undo(), Relayout::NotNeeded);
    assert_eq!(
        pos(&open),
        Vec2::ZERO,
        "the two frames before it, as one entry"
    );
    assert!(!open.can_undo());
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
        document::save(
            &Document::from(graph),
            &path,
            SavedPaths::Relative,
            DocumentFormat::Ron,
        )
        .unwrap();
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
    assert_eq!(open.format, DocumentFormat::Ron);
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
