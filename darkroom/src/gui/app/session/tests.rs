use glam::Vec2;
use palantir::{DockOperation, Key, Modifiers};
use scenarium::NodeId;
use std::sync::Arc;

use crate::alloc_audit;
use crate::core::document::TabRef;
use crate::core::document::internals::DocFixture;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::preview::preview_func;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::file::FileCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::app::session::internals::SessionHarness;
use crate::gui::pane::graph::node::preview_row::preview_image_wid;
use crate::gui::pane::graph::toolbar::internals::run_chip_wid;
use crate::gui::pane::viewer::ImageViewer;
use crate::gui::state::preview_store::StoredContent;
use crate::gui::state::preview_store::internals::opaque_image_value;

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
/// nodes, a preview card holding an image, and the status bar's memory
/// readout, which recomputes its whole line every frame.
///
/// Each frame is audited on its own rather than summed, so a
/// grow-on-the-Nth-frame allocation — a `Vec` doubling, a map rehash —
/// fails on the frame that performed it.
#[test]
fn a_settled_frame_records_without_allocating() {
    let mut fixture = DocFixture::probes(6);
    let node = fixture.add(&preview_func(Arc::default()));
    let mut test = SessionHarness::new(fixture);
    test.run_state
        .previews
        .ingest_preview(test.ui.ui(), node, opaque_image_value());
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
/// — a click makes the frame record twice — and a viewer uploads its
/// full-resolution texture as it draws, so the image is on screen in the
/// same frame the click opened it.
#[test]
fn a_viewer_opened_by_click_shows_its_image_in_that_same_frame() {
    let mut fixture = DocFixture::default();
    let node = fixture.add(&preview_func(Arc::default()));
    let mut test = SessionHarness::new(fixture);
    test.run_state
        .previews
        .ingest_preview(test.ui.ui(), node, opaque_image_value());
    test.prime(2);

    let resident = |test: &SessionHarness| {
        let Some(StoredContent::Image(image)) = test.run_state.previews.entries.get(&node) else {
            panic!("the ingested image is the node's stored content");
        };
        image.is_full_resident()
    };
    assert!(
        !resident(&test),
        "a card-only preview never uploads its full-resolution source"
    );

    test.ui.click_on(preview_image_wid(node));
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
        resident(&test),
        "and the pass that drew the new tab uploaded its texture — no \
         placeholder, and no waiting for the next frame"
    );
}

/// A widget cannot legitimately build a malformed intent — it reads
/// every identity it emits out of the live document — so one is our own
/// bug and fails loudly in every build: no caller is there to take a
/// refusal back.
#[test]
#[should_panic(expected = "a widget built a malformed intent")]
fn a_widget_built_malformed_intent_is_a_bug_not_a_refusal() {
    let mut test = SessionHarness::new(DocFixture::default());
    test.apply(GraphIntent::add_node(
        Vec2::ZERO,
        NodeId::nil(),
        DocFixture::stub_node(),
    ));
}

/// The exit prompt's signal: content edits flip `dirty`, navigation
/// doesn't.
#[test]
fn dirty_flag_tracks_content_edits_not_navigation() {
    let mut test = SessionHarness::new(DocFixture::default());
    let node_id = NodeId::unique();

    test.apply(GraphIntent::add_node(
        Vec2::ZERO,
        node_id,
        DocFixture::stub_node(),
    ));
    assert!(test.session.open.dirty, "adding a node is savable work");

    test.session.open.dirty = false;
    test.apply(GraphIntent::SetSelection {
        to: [node_id].into_iter().collect(),
    });
    assert!(!test.session.open.dirty, "selecting is navigation");
}

/// Pane arrangement is navigation: the op lands on the layout but
/// records no undo step and doesn't flip the unsaved flag, so Ctrl+Z
/// walks straight past it to the last graph edit and quitting after a
/// rearrangement doesn't prompt.
#[test]
fn dock_ops_apply_without_entering_the_undo_history_or_dirtying() {
    let mut test = SessionHarness::new(DocFixture::default());
    let node_id = NodeId::unique();
    test.apply(GraphIntent::add_node(
        Vec2::ZERO,
        node_id,
        DocFixture::stub_node(),
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

    // One undo takes back the *node*, not the tab.
    assert!(test.undo(), "the node add is the only entry");
    assert_eq!(
        test.session.open.document.graph.len(),
        0,
        "the node came back out"
    );
    assert!(
        test.session
            .open
            .document
            .layout
            .all_tabs()
            .any(|t| t == tab),
        "undo leaves the layout alone"
    );
    assert!(!test.undo(), "the dock op recorded nothing of its own");
}

/// Two surfaces answering the same frame both reach `App`, in the order
/// they answered. A queue that kept only the first claim would drop a
/// Ctrl+S that landed on the frame a run chip was clicked.
#[test]
fn a_chord_and_a_click_on_one_frame_both_reach_the_app() {
    let mut test = SessionHarness::new(DocFixture::probes(1));
    // Two frames so the toolbar chip has a rect to aim at, and so the
    // Ctrl+S chord is subscribed for palantir's keyboard wake-gate.
    test.prime(2);

    test.ui.set_modifiers(Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    });
    test.ui.key(Key::Char('S'));
    test.ui.click_on(run_chip_wid());

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
    // A node the graph actually holds: a viewer tab names the preview node
    // whose value it shows, and the drain below prunes a tab whose node is
    // gone — as it must, since that is what closes a viewer when its node
    // is deleted.
    let fixture = DocFixture::probes(1);
    let node_id = fixture.node(0);
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
