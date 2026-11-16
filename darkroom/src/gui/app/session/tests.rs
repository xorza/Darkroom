use glam::{UVec2, Vec2};
use palantir::Harness;
use palantir::{DockOperation, Key, Modifiers};
use scenarium::{Binding, GraphId, InputPort, NodeId, OutputPort, PortId, ProgramId};
use std::iter;
use std::sync::Arc;

use common::internals::alloc_audit;

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::TieAngle;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::open_document::OpenDocument;
use crate::core::preview_sink;
use crate::gui::app::App;
use crate::gui::app::session::internals::SessionHarness;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::FileCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::frame::app_command::ViewCommand;
use crate::gui::frame::shortcuts;
use crate::gui::pane::graph_ui::toolbar;
use crate::gui::pane::graph_ui::wid;
use crate::gui::pane::image_viewer::ImageViewer;
use crate::gui::state::preview_store;
use crate::gui::state::preview_store::StoredContent;

/// Frames to settle the scene before the window opens. The caches that
/// grow once — text shaping, palantir's widget tables, the record store's
/// arenas — do it inside these, so what the window sees is steady state.
const SETTLE_FRAMES: u32 = 32;
/// Long enough that a once-every-N-frames allocation lands inside the
/// window rather than after it.
const AUDITED_FRAMES: usize = 64;

/// A whole frame of the app performs no heap operation once the editor has settled: its sweeps
/// and every record pass, as the window runs them.
///
/// Strict zero, because every allocation on this path would be the editor's own: a `format!`
/// for a label palantir copies into its text arena anyway, or a `collect()` for a list that
/// could refill a buffer the editor owns. One frame's worth is too small to show in a profile;
/// the cost is the tail it puts on the frames that trip the allocator.
///
/// The scene is the one the record path is widest over: a graph pane of
/// nodes with const editors, wires run through two ties, one selected and pinned, a preview card
/// holding an image, and the status bar's memory readout, which recomputes its whole line every
/// frame.
///
/// Each frame is audited on its own rather than summed, so a grow-on-the-Nth-frame allocation,
/// a `Vec` doubling or a map rehash, fails on the frame that performed it.
#[test]
fn a_settled_frame_records_without_allocating() {
    let mut ui = Harness::new(UVec2::new(1200, 800));
    let mut app = App::offscreen(ui.ui_mut());
    let library = app.runtime.library.cur();
    let func = library.by_name("Brightness / Contrast").unwrap();
    let mut fixture = DocumentFixture::default();
    let nodes: [NodeId; 6] = std::array::from_fn(|_| fixture.add(func));
    let wires = [(0, 1), (1, 2), (0, 3)].map(|(from, to)| {
        fixture.wire(
            OutputPort::new(nodes[from], PortId::new(1)),
            InputPort::new(nodes[to], PortId::new(1)),
        )
    });
    let tie = fixture.tie(Vec2::new(300.0, 120.0), wires);
    fixture.tie(Vec2::new(380.0, 140.0), [wires[0]]);
    let view = &mut fixture.document.main_view;
    view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(30.0);
    view.selected.insert(ItemId::Tie(tie));
    let preview = fixture.add(&preview_sink::func(Arc::default()));
    fixture.document.validate().unwrap();
    app.adopt_document(OpenDocument::over(fixture.document));
    app.run_state.previews_mut().publish(
        ProgramId::from(preview),
        preview_store::internals::opaque_image_value(),
    );
    app.run_state.previews_mut().settle(ui.ui());
    // Each frame asked, as a wake asks one: a frame nothing asked for paints only and records
    // nothing, which would audit nothing.
    let full_frame = |ui: &mut Harness, app: &mut App| {
        ui.ui_mut().request_repaint();
        ui.frame_app(app)
    };
    for _ in 0..SETTLE_FRAMES {
        full_frame(&mut ui, &mut app);
    }

    for frame in 0..AUDITED_FRAMES {
        let allocations = alloc_audit::allocations(|| {
            full_frame(&mut ui, &mut app);
        });
        assert_eq!(
            allocations, 0,
            "settled frame {frame} performed {allocations} heap operations"
        );
    }
}

/// A frame reads every menu chord, also past the one that fired, so a chord that comes next still
/// wakes the window: with nothing focused, only a watched press asks for a frame.
#[test]
fn a_menu_chord_leaves_the_others_watched() {
    let mut test = SessionHarness::new(DocumentFixture::default());
    test.prime(2);
    test.ui.key(shortcuts::NEW);
    assert!(matches!(
        test.frame()[..],
        [AppCommand::File(FileCommand::New)]
    ));
    assert!(test.ui.key_down(shortcuts::QUIT).repaint_requested);
}

/// Opening a viewer by clicking a preview card lands its tab *inside* the
/// record that read the click. The tab is drawn by the pass after that one
/// — a click makes the frame record twice — and a viewer asks for its
/// full-resolution conversion as it draws, so the frame the click opened it
/// raises the one ask, with the thumbnail standing in until the texture
/// lands.
#[test]
fn a_viewer_opened_by_click_queues_its_full_image_in_that_same_frame() {
    let mut fixture = DocumentFixture::default();
    let node = ProgramId::from(fixture.add(&preview_sink::func(Arc::default())));
    let mut test = SessionHarness::new(fixture);
    test.run_state
        .previews_mut()
        .publish(node, preview_store::internals::opaque_image_value());
    test.run_state.previews_mut().settle(test.ui.ui());
    test.prime(2);

    let resident = |test: &SessionHarness| {
        let Some(StoredContent::Image(image)) = test.run_state.previews().get(node) else {
            panic!("the landed image is the node's stored content");
        };
        image.is_full_resident()
    };
    assert!(
        !resident(&test),
        "a card-only preview never uploads its full-resolution source"
    );

    test.ui
        .click(wid::preview_image(GraphId::Main, node.node_id()));
    let commands = test.frame();
    assert!(
        test.session
            .open
            .document()
            .layout
            .all_tabs()
            .any(|t| t == TabRef::ImageViewer(node)),
        "the click opened the viewer tab within that same frame"
    );
    assert!(
        !resident(&test),
        "the conversion runs off the frame, so the click frame draws the \
         thumbnail in its place"
    );
    let [
        AppCommand::View(ViewCommand::ConvertFull {
            program_id: asked,
            max_dim,
        }),
    ] = commands[..]
    else {
        panic!("the click frame raised {commands:?}, not the one ask");
    };
    assert_eq!(asked, node);
    // As `App::run_command` applies it; nothing else asks before the settle,
    // so a conversion that lands is the one that frame asked for.
    test.run_state.previews_mut().convert_full(asked, max_dim);
    test.run_state.previews_mut().settle(test.ui.ui());
    assert!(
        resident(&test),
        "the pass that drew the new tab queued its conversion"
    );
    assert!(
        test.frame().is_empty(),
        "a viewer whose texture is resident asks for nothing"
    );
}

/// A widget cannot legitimately build a malformed intent — it reads
/// every identity it emits out of the live document — so one is our own
/// bug and fails loudly in every build: no caller is there to take a
/// refusal back.
#[test]
#[should_panic(expected = "a widget built a malformed intent")]
fn a_widget_built_malformed_intent_is_a_bug_not_a_refusal() {
    let mut test = SessionHarness::new(DocumentFixture::default());
    test.apply(GraphIntent::add_node(
        Vec2::ZERO,
        NodeId::nil(),
        DocumentFixture::missing_node(),
    ));
}

/// The exit prompt's signal: content edits flip `dirty`, navigation
/// doesn't.
#[test]
fn dirty_flag_tracks_content_edits_not_navigation() {
    let mut fixture = DocumentFixture::default();
    let node = fixture.plain_node();
    let mut test = SessionHarness::new(fixture);
    let node_id = NodeId::unique();

    test.apply(GraphIntent::add_node(Vec2::ZERO, node_id, node));
    assert!(
        test.session.open.is_dirty(),
        "adding a node is savable work"
    );

    test.session.open.set_dirty(false);
    test.apply(GraphIntent::SetSelection {
        to: [ItemId::Node(node_id)].into_iter().collect(),
    });
    assert!(!test.session.open.is_dirty(), "selecting is navigation");
}

/// Pane arrangement is navigation: the op lands on the layout but
/// records no undo step and doesn't flip the unsaved flag, so Ctrl+Z
/// walks straight past it to the last graph edit and quitting after a
/// rearrangement doesn't prompt.
#[test]
fn dock_ops_apply_without_entering_the_undo_history_or_dirtying() {
    let mut fixture = DocumentFixture::default();
    let preview = preview_sink::func(Arc::default());
    fixture.library.add(preview.clone());
    let mut test = SessionHarness::new(fixture);
    let node_id = NodeId::unique();
    test.apply(GraphIntent::add_node(
        Vec2::ZERO,
        node_id,
        scenarium::Node::from(&preview),
    ));

    let tab = TabRef::ImageViewer(ProgramId::from(node_id));
    test.session.open.set_dirty(false);
    test.requests.push_view(DockOperation::OpenTab { tab });
    test.drain();
    assert!(
        test.session
            .open
            .document()
            .layout
            .all_tabs()
            .any(|t| t == tab),
        "the viewer tab opened"
    );
    assert!(
        !test.session.open.is_dirty(),
        "arranging panes is navigation, not savable work"
    );

    // One undo takes back the node, and its viewer tab goes with it.
    assert!(test.undo(), "the node add is the only entry");
    assert_eq!(
        test.session.open.document().graphs.main().len(),
        0,
        "the node came back out"
    );
    assert!(
        !test
            .session
            .open
            .document()
            .layout
            .all_tabs()
            .any(|t| t == tab),
        "a removed node's viewer closes"
    );
    assert!(!test.undo(), "the dock op recorded nothing of its own");
}

/// Two surfaces answering the same frame both reach `App`, in the order
/// they answered. A queue that kept only the first claim would drop a
/// Ctrl+S that landed on the frame a run chip was clicked.
#[test]
fn a_chord_and_a_click_on_one_frame_both_reach_the_app() {
    let mut test = SessionHarness::new(DocumentFixture::probes(1));
    // Two frames so the toolbar chip has a rect to aim at, and so the
    // Ctrl+S chord is subscribed for palantir's keyboard wake-gate.
    test.prime(2);

    test.ui.hold_modifiers(Modifiers::CTRL);
    test.ui.key(Key::Char('S'));
    test.ui
        .click(toolbar::internals::run_chip_wid(GraphId::Main));

    let commands = test.frame();
    assert!(
        matches!(
            commands[..],
            [
                AppCommand::File(FileCommand::Save),
                AppCommand::Run(RunCommand::Once)
            ]
        ),
        "the chord is raised before the record, the chip during it: {commands:?}"
    );
}

/// Viewer tabs dedupe per node, and their navigation state is dropped
/// once the tab closes.
#[test]
fn image_viewer_tabs_dedupe_per_node_and_prune_state_on_close() {
    // A preview node the graph holds: a viewer tab names the preview node
    // whose value it shows, and a document holds no viewer on any other.
    let mut fixture = DocumentFixture::default();
    let node_id = fixture.add(&preview_sink::func(Arc::default()));
    let mut test = SessionHarness::new(fixture);
    let tab = TabRef::ImageViewer(ProgramId::from(node_id));

    test.requests.push_view(DockOperation::OpenTab { tab });
    test.requests.push_view(DockOperation::OpenTab { tab });
    test.drain();
    assert_eq!(
        test.session
            .open
            .document()
            .layout
            .all_tabs()
            .filter(|t| *t == tab)
            .count(),
        1,
        "one tab per node"
    );

    test.session.main_window.image_viewers.insert(
        ProgramId::from(node_id),
        ImageViewer::new(ProgramId::from(node_id)),
    );
    test.session
        .open
        .document_mut()
        .layout
        .apply(DockOperation::CloseTab { tab });
    test.session.reconcile_caches();
    assert!(
        test.session.main_window.image_viewers.is_empty(),
        "closing the tab drops its navigation state"
    );
}

/// Every route an edit takes owes what it strands, and spending clears it: a rename strands the
/// geometry through the queue, through one edit, through its undo and through its redo, and a
/// selection strands nothing on any of them.
#[test]
fn each_edit_route_owes_what_it_strands() {
    let mut test = SessionHarness::new(DocumentFixture::probes(1));
    let node_id = test
        .session
        .open
        .document()
        .graphs
        .main()
        .iter()
        .next()
        .unwrap()
        .node_id;
    let rename = |to: &str| GraphIntent::RenameNode {
        node_id,
        to: to.to_owned(),
    };
    let select = || GraphIntent::SetSelection {
        to: [ItemId::Node(node_id)].into_iter().collect(),
    };
    let owed_then_clear = |test: &mut SessionHarness| {
        let owed = test.requests.owes_relayout();
        test.ui.frame(|ui| test.requests.spend_relayout(ui));
        owed
    };

    test.apply(select());
    assert!(!owed_then_clear(&mut test), "one edit, a selection");
    test.requests.push_graph(GraphId::Main, select());
    test.drain();
    assert!(!owed_then_clear(&mut test), "the queue, a selection");

    test.apply(rename("one"));
    assert!(owed_then_clear(&mut test), "one edit, a rename");
    test.requests.push_graph(GraphId::Main, rename("two"));
    test.drain();
    assert!(owed_then_clear(&mut test), "the queue, a rename");
    assert!(test.undo());
    assert!(owed_then_clear(&mut test), "an undo of a rename");
    let SessionHarness {
        session, requests, ..
    } = &mut test;
    session.redo(requests);
    assert!(owed_then_clear(&mut test), "a redo of a rename");
}

/// Two panes side by side on a subgraph and its copy, which holds the same node under the same
/// id: a click on the node in one pane selects it in that graph alone, and makes it active.
#[test]
fn a_click_in_one_pane_selects_nothing_in_a_copy_beside_it() {
    use palantir::{DockDrop, SplitSide};
    use scenarium::{Interface, Subgraph, SubgraphId};

    use crate::core::document::internals::insert_subgraph;
    use crate::core::document::item_id::ItemId;

    let mut fixture = DocumentFixture::default();
    let [left, right] = [1, 2].map(SubgraphId::from_u128);
    let shared = NodeId::from_u128(40);
    for id in [left, right] {
        let mut subgraph = Subgraph::new("copy", Interface::default());
        subgraph.graph.insert(shared, fixture.plain_node());
        insert_subgraph(&mut fixture.document, id, subgraph);
    }
    let layout = &mut fixture.document.layout;
    let group = layout.primary().id();
    let [left_tab, right_tab] = [left, right].map(|id| TabRef::Graph(GraphId::Subgraph(id)));
    layout.apply(DockOperation::OpenTab { tab: left_tab });
    layout.apply(DockOperation::OpenTab { tab: right_tab });
    layout.apply(DockOperation::MoveTab {
        tab: right_tab,
        to: DockDrop::Split {
            group,
            side: SplitSide::Right,
        },
    });
    let mut test = SessionHarness::new(fixture);
    test.prime(2);
    let [left_graph, right_graph] = [left, right].map(GraphId::Subgraph);
    for graph in [left_graph, right_graph] {
        assert!(
            test.ui.rect(wid::body(graph, shared)).is_some(),
            "premise: {graph:?}'s pane shows the node"
        );
    }

    test.ui.click(wid::body(left_graph, shared));
    test.frame();
    test.drain();
    let document = test.session.open.document();
    assert_eq!(
        document.view(left_graph).selected,
        [ItemId::Node(shared)].into_iter().collect()
    );
    assert!(document.view(right_graph).selected.is_empty());
    assert_eq!(document.active, left_graph);
}

/// A wire from a node's output dropped on the `Outputs` node of its subgraph's pane gives the
/// interface an output of that port's name and type, wired from it, as one history entry.
#[test]
fn a_wire_dropped_on_the_outputs_node_adds_an_interface_output() {
    use palantir::PointerButton;
    use scenarium::{Boundary, InputPort, Interface, Subgraph, SubgraphId};

    use crate::core::document::internals::insert_subgraph;
    use crate::gui::port_ref::PortRef;

    let mut fixture = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    let mut subgraph = Subgraph::new("inner", Interface::default());
    let inside = subgraph.graph.add(fixture.plain_node());
    let outputs = subgraph.boundary_node(Boundary::Outputs);
    insert_subgraph(&mut fixture.document, id, subgraph);
    let graph = GraphId::Subgraph(id);
    let fixture = fixture.with_tab(TabRef::Graph(graph));
    let mut test = SessionHarness::new(fixture);
    test.prime(2);
    let from = test
        .ui
        .rect(wid::port_circle(
            graph,
            PortRef::output(inside, PortId::new(1)),
        ))
        .expect("the output shows")
        .center();
    let onto = test
        .ui
        .rect(wid::body(graph, outputs))
        .expect("the Outputs node shows")
        .center();

    test.ui.press(from, PointerButton::Left);
    test.frame();
    test.ui.drag_to(onto);
    test.frame();
    test.ui.release(PointerButton::Left);
    test.frame();
    test.drain();
    let document = test.session.open.document();
    let interface = &document.graphs.subgraph(id).unwrap().interface;
    assert_eq!(interface.outputs().len(), 1);
    let port = interface.outputs()[0].id;
    assert_eq!(
        document
            .graphs
            .graph(graph)
            .bindings
            .get(&InputPort::new(outputs, port)),
        Some(&Binding::Wire(OutputPort::new(inside, PortId::new(1))))
    );
    assert!(test.undo());
    assert!(
        test.session
            .open
            .document()
            .graphs
            .subgraph(id)
            .unwrap()
            .interface
            .outputs()
            .is_empty()
    );
}

/// An edit the document's rules refuse reaches the shell as the status line naming why: here a
/// boundary node's removal, which writes nothing.
#[test]
fn a_refused_edit_raises_its_status_line() {
    use scenarium::{Boundary, Interface, Subgraph, SubgraphId};

    use crate::core::document::internals::insert_subgraph;

    let mut fixture = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    let subgraph = Subgraph::new("inner", Interface::default());
    let inputs = subgraph.boundary_node(Boundary::Inputs);
    insert_subgraph(&mut fixture.document, id, subgraph);
    let mut test = SessionHarness::new(fixture);
    test.requests.push_graph(
        GraphId::Subgraph(id),
        GraphIntent::RemoveNode { node_id: inputs },
    );
    test.drain();
    let commands: Vec<AppCommand> = iter::from_fn(|| test.requests.pop_app()).collect();
    assert!(
        matches!(
            commands.as_slice(),
            [AppCommand::Warn(line)] if line == "a subgraph keeps its Inputs and Outputs nodes"
        ),
        "{commands:?}"
    );
    assert!(!test.session.open.can_undo(), "nothing was written");
}

/// Ctrl+C over a selected instance puts a clip on the clipboard, and Ctrl+V pastes another
/// instance of the same definition, which the paste reuses, as one history entry.
#[test]
fn copy_and_paste_go_through_the_clipboard() {
    use scenarium::{Interface, Subgraph, SubgraphId};

    use crate::core::clip::HEADER;
    use crate::core::document::internals::insert_subgraph;
    use crate::core::document::item_id::ItemId;

    let mut fixture = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    insert_subgraph(
        &mut fixture.document,
        id,
        Subgraph::new("inner", Interface::default()),
    );
    let instance = fixture.add_instance(id);
    let mut test = SessionHarness::new(fixture);
    test.apply(GraphIntent::SetSelection {
        to: [ItemId::Node(instance)].into(),
    });
    test.prime(2);
    test.ui.key(shortcuts::COPY);
    test.frame();
    assert!(
        test.ui
            .clipboard_text()
            .is_some_and(|text| text.starts_with(HEADER)),
        "the clipboard holds a clip"
    );
    test.ui.key(shortcuts::PASTE);
    test.frame();
    test.drain();
    let document = test.session.open.document();
    let instances = document
        .graphs
        .main()
        .iter()
        .filter(|node| node.subgraph() == Some(id))
        .count();
    assert_eq!((instances, document.graphs.subgraphs().len()), (2, 1));
    assert!(test.undo());
    assert_eq!(test.session.open.document().graphs.main().len(), 1);
}

/// A double click on a subgraph's tab opens an editor of its name in the chip, and Enter renames
/// the subgraph, one undo entry; the tab's menu opens the same editor, and Escape keeps the name.
#[test]
fn a_subgraphs_tab_renames_it_in_place() {
    use scenarium::{Interface, Subgraph, SubgraphId};

    use crate::core::document::internals::insert_subgraph;
    use crate::gui::main_window::dock_panes::rename_wid;

    let mut fixture = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    insert_subgraph(
        &mut fixture.document,
        id,
        Subgraph::new("Denoise", Interface::default()),
    );
    fixture.document.layout.apply(DockOperation::OpenTab {
        tab: TabRef::Graph(GraphId::Subgraph(id)),
    });
    let mut test = SessionHarness::new(fixture);
    let name = |test: &SessionHarness| {
        test.session
            .open
            .document()
            .graphs
            .subgraph(id)
            .unwrap()
            .name
            .clone()
    };
    test.prime(2);

    test.ui.double_click("Denoise");
    test.prime(2);
    assert!(
        palantir::EditableLabel::is_editing(test.ui.ui(), rename_wid(id)),
        "the double click opens the editor"
    );
    test.ui.type_text("Stars");
    test.ui.key(Key::Enter);
    test.prime(2);
    assert_eq!(name(&test), "Stars");
    assert!(test.undo());
    assert_eq!(name(&test), "Denoise");

    test.prime(2);
    test.ui.right_click("Denoise");
    test.prime(2);
    test.ui.click("Rename");
    test.prime(2);
    let editing =
        |test: &SessionHarness| palantir::EditableLabel::is_editing(test.ui.ui(), rename_wid(id));
    assert!(editing(&test), "the menu's Rename opens the editor");
    test.ui.type_text("Gone");
    test.ui.key(Key::Escape);
    test.prime(2);
    assert!(!editing(&test));
    assert_eq!(name(&test), "Denoise", "Escape keeps the name");
}
