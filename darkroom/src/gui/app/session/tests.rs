use glam::Vec2;
use palantir::{DockOperation, Key, Modifiers};
use scenarium::{InputPort, NodeId, OutputPort};
use std::sync::Arc;

use crate::alloc_audit;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::TieAngle;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::preview_sink;
use crate::gui::app::session::internals::SessionHarness;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::FileCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::pane::graph_ui::node_ui::preview_row;
use crate::gui::pane::graph_ui::toolbar;
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

/// The record path performs no heap operation once the editor has settled.
///
/// Strict zero, because every allocation on this path would be the
/// editor's own: a `format!` for a label palantir was going to copy into
/// its text arena anyway, or a `collect()` for a list that could have
/// refilled a buffer the editor already owns. Both have appeared here
/// before, and neither shows up in a profile — one frame's worth is far
/// too small to see, and the cost is the tail it puts on the frames that
/// happen to trip the allocator.
///
/// The scene is the one the record path is widest over: a graph pane of
/// nodes, wires run through two ties, one selected and pinned, a preview
/// card holding an image, and the status bar's memory readout, which
/// recomputes its whole line every frame.
///
/// Each frame is audited on its own rather than summed, so a
/// grow-on-the-Nth-frame allocation — a `Vec` doubling, a map rehash —
/// fails on the frame that performed it.
#[test]
fn a_settled_frame_records_without_allocating() {
    let mut fixture = DocumentFixture::probes(6);
    let probes: [NodeId; 6] = std::array::from_fn(|i| fixture.node(i));
    let wires = [(0, 1), (1, 2), (0, 3)].map(|(from, to)| {
        fixture.wire(
            OutputPort::new(probes[from], 0),
            InputPort::new(probes[to], 0),
        )
    });
    let tie = fixture.tie(Vec2::new(300.0, 120.0), wires);
    fixture.tie(Vec2::new(380.0, 140.0), [wires[0]]);
    let view = &mut fixture.document.main_view;
    view.ties.get_mut(&tie).unwrap().angle = TieAngle::Pinned(30.0);
    view.selected.insert(ItemId::Tie(tie));
    let node = fixture.add(&preview_sink::func(Arc::default()));
    let mut test = SessionHarness::new(fixture);
    test.run_state
        .previews
        .publish(node, preview_store::internals::opaque_image_value());
    test.run_state.previews.settle(test.ui.ui());
    // A reading, so the memory readout renders its longest form rather
    // than the absent-figure path.
    test.process_memory = 3 * 1024 * 1024;
    test.prime(SETTLE_FRAMES);

    for frame in 0..AUDITED_FRAMES {
        let allocations = alloc_audit::allocations(|| {
            let _ = test.frame();
        });
        assert_eq!(
            allocations, 0,
            "settled frame {frame} performed {allocations} heap operations"
        );
    }
}

/// Opening a viewer by clicking a preview card lands its tab *inside* the
/// record that read the click. The tab is drawn by the pass after that one
/// — a click makes the frame record twice — and a viewer queues its
/// full-resolution conversion as it draws, so the conversion starts in the
/// same frame the click opened it, with the thumbnail standing in until it
/// lands.
#[test]
fn a_viewer_opened_by_click_queues_its_full_image_in_that_same_frame() {
    let mut fixture = DocumentFixture::default();
    let node = fixture.add(&preview_sink::func(Arc::default()));
    let mut test = SessionHarness::new(fixture);
    test.run_state
        .previews
        .publish(node, preview_store::internals::opaque_image_value());
    test.run_state.previews.settle(test.ui.ui());
    test.prime(2);

    let resident = |test: &SessionHarness| {
        let Some(StoredContent::Image(image)) = test.run_state.previews.get(node) else {
            panic!("the landed image is the node's stored content");
        };
        image.is_full_resident()
    };
    assert!(
        !resident(&test),
        "a card-only preview never uploads its full-resolution source"
    );

    test.ui.click_on(preview_row::preview_image_wid(node));
    let _ = test.frame();
    assert!(
        test.session
            .open
            .document
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
    // Nothing between the click frame and the settle asks again, so a
    // conversion that lands is one that frame queued.
    test.run_state.previews.settle(test.ui.ui());
    assert!(
        resident(&test),
        "the pass that drew the new tab queued its conversion"
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
        DocumentFixture::stub_node(),
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
    assert!(test.session.open.dirty, "adding a node is savable work");

    test.session.open.dirty = false;
    test.apply(GraphIntent::SetSelection {
        to: [ItemId::Node(node_id)].into_iter().collect(),
    });
    assert!(!test.session.open.dirty, "selecting is navigation");
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

    let tab = TabRef::ImageViewer(node_id);
    test.session.open.dirty = false;
    test.requests.push_view(DockOperation::OpenTab { tab });
    test.drain();
    assert!(
        test.session
            .open
            .document
            .layout
            .all_tabs()
            .any(|t| t == tab),
        "the viewer tab opened"
    );
    assert!(
        !test.session.open.dirty,
        "arranging panes is navigation, not savable work"
    );

    // One undo takes back the node, and its viewer tab goes with it.
    assert!(test.undo(), "the node add is the only entry");
    assert_eq!(
        test.session.open.document.graph.len(),
        0,
        "the node came back out"
    );
    assert!(
        !test
            .session
            .open
            .document
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

    test.ui.set_modifiers(Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    });
    test.ui.key(Key::Char('S'));
    test.ui.click_on(toolbar::internals::run_chip_wid());

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
    let tab = TabRef::ImageViewer(node_id);

    test.requests.push_view(DockOperation::OpenTab { tab });
    test.requests.push_view(DockOperation::OpenTab { tab });
    test.drain();
    assert_eq!(
        test.session
            .open
            .document
            .layout
            .all_tabs()
            .filter(|t| *t == tab)
            .count(),
        1,
        "one tab per node"
    );

    test.session
        .main_window
        .image_viewers
        .insert(node_id, ImageViewer::new(node_id));
    test.session
        .open
        .document
        .layout
        .apply(DockOperation::CloseTab { tab });
    test.session.reconcile_caches();
    assert!(
        test.session.main_window.image_viewers.is_empty(),
        "closing the tab drops its navigation state"
    );
}
