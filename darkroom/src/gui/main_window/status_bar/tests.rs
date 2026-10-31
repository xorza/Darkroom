use glam::UVec2;
use palantir::Harness;

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::app::session::internals::SessionHarness;

/// The memory clause leads with the process and sums both cache pools; no footprint leaves no
/// clause, even with a populated cache.
#[test]
fn memory_label_leads_with_the_process_then_sums_both_cache_pools() {
    const MEM: u64 = 3 * 1024 * 1024;
    let fields = |process, cache| {
        MemoryLabel::resolve(process, cache).map(|label| (label.process, label.cached))
    };
    assert_eq!(fields(MEM, RamUsage::default()), Some((MEM, 0)));
    assert_eq!(
        fields(MEM, RamUsage { cpu: 1024, gpu: 0 }),
        Some((MEM, 1024))
    );
    assert_eq!(
        fields(MEM, RamUsage { cpu: 0, gpu: 2048 }),
        Some((MEM, 2048))
    );
    // Both pools → one clause carrying the sum: 1024 + 2048 = 3072 B.
    assert_eq!(
        fields(
            MEM,
            RamUsage {
                cpu: 1024,
                gpu: 2048
            }
        ),
        Some((MEM, 3072))
    );
    assert_eq!(
        fields(
            0,
            RamUsage {
                cpu: 1024,
                gpu: 2048
            }
        ),
        None
    );
}

/// The readout reserves each figure's widest value, so the line keeps its width as the zoom and
/// the memory change their digit counts: 10% against 500%, and 1.0 KiB against 1023.9 MiB.
#[test]
fn the_readout_holds_its_width_as_its_figures_change() {
    let theme = Theme::default();
    let mut h = Harness::new(UVec2::new(1600, 100));
    let root = WidgetId::from_hash("readout_root");
    let mut width = |zoom: f32, process: u64, cached: u64| {
        h.frame(|ui| {
            Panel::hstack()
                .id(root)
                .size((Sizing::HUG, Sizing::HUG))
                .show(ui, |ui| {
                    Readout {
                        counts: CountsClause {
                            node_count: 19,
                            selected_count: 1,
                        },
                        zoom,
                        ram: Some(MemoryLabel { process, cached }),
                    }
                    .show(ui, &theme);
                });
        });
        h.rect(root).expect("the readout arranged").size.w
    };
    let short = width(0.1, 1024, 1024);
    let long = width(5.0, 1_073_636_966, 1_073_636_966);
    assert_eq!(short, long);
}

/// The graph's line gives the bar something to say on every frame, so it is recorded on an
/// untouched document, one row tall with or without a memory reading: a bar that grew as the
/// figure landed would move the dock.
#[test]
fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
    let mut h = SessionHarness::new(DocumentFixture::default());
    h.prime(2);
    let without =
        h.ui.rect(wid())
            .expect("status bar records with no reading and an empty cache");

    h.process_memory = 3 * 1024 * 1024;
    h.prime(2);
    let with = h.ui.rect(wid()).expect("status bar records");

    // A row, two body sizes: 2 × 14 = 28 px, on the 4 px grid.
    let theme = Theme::default();
    assert_eq!(theme.row_height(), 28.0);
    assert_eq!(without.size.h, theme.row_height(), "no reading");
    assert_eq!(with.size.h, theme.row_height(), "a 3 MiB reading");
    for (rect, what) in [(without, "no reading"), (with, "3 MiB reading")] {
        assert_eq!(rect.size.w, 1200.0, "{what}: the bar spans the window");
    }
}

/// The graph's counts name its nodes, one noun singular, and its selection.
#[test]
fn the_counts_clause_counts_nodes_and_the_selection() {
    let line = |node_count, selected_count| {
        CountsClause {
            node_count,
            selected_count,
        }
        .to_string()
    };
    assert_eq!(line(19, 1), "19 nodes \u{b7} 1 selected");
    assert_eq!(line(1, 0), "1 node \u{b7} 0 selected");
    assert_eq!(line(0, 0), "0 nodes \u{b7} 0 selected");
}

/// The run line leads with the worst of the run and counts the rest, one noun singular.
#[test]
fn the_run_line_leads_with_the_worst_of_the_run() {
    let line = |outcome| RunLine(outcome).to_string();
    let ran = RunOutcome {
        ran_count: 8,
        ..RunOutcome::default()
    };
    assert_eq!(line(ran), "Ran 8 nodes.");
    assert_eq!(line(RunOutcome::default()), "Nothing ran.");
    assert_eq!(
        line(RunOutcome {
            ran_count: 1,
            cached_count: 3,
            ..RunOutcome::default()
        }),
        "Ran 1 node, 3 cached."
    );
    assert_eq!(
        line(RunOutcome {
            failed_count: 1,
            skipped_count: 11,
            ..ran
        }),
        "Run failed: 1 error, 11 skipped."
    );
    assert_eq!(
        line(RunOutcome {
            failed_count: 2,
            ..RunOutcome::default()
        }),
        "Run failed: 2 errors."
    );
    assert_eq!(
        line(RunOutcome {
            missing_count: 1,
            ..ran
        }),
        "Run incomplete: 1 node missing an input."
    );
    assert_eq!(
        line(RunOutcome {
            failed_count: 1,
            cancelled: true,
            ..ran
        }),
        "Run cancelled."
    );
}

/// The run line's link selects the node that failed and asks the canvas to frame it, and the
/// canvas's next prepass moves the camera onto it.
#[test]
fn the_run_lines_link_selects_and_frames_the_cause() {
    use scenarium::{InvokeError, NodeExecStatus, NodeStatus, RunError, RunSummary};
    use std::io;
    use std::sync::Arc;

    use crate::core::document::item_id::ItemId;

    let mut h = SessionHarness::new(DocumentFixture::probes(2));
    let ids: Vec<_> = h
        .session
        .open
        .document
        .graph
        .iter()
        .map(|node| node.node_id)
        .collect();
    let cause = ids[1];
    h.run_state.complete(RunSummary {
        nodes: vec![
            NodeStatus {
                node_id: ids[0],
                status: Some(NodeExecStatus::Executed { elapsed_secs: 0.5 }),
                ram: RamUsage::default(),
            },
            NodeStatus {
                node_id: cause,
                status: Some(NodeExecStatus::Errored {
                    error: RunError::Invoke(Arc::new(InvokeError::external(io::Error::other(
                        "boom",
                    )))),
                }),
                ram: RamUsage::default(),
            },
        ],
        ..RunSummary::default()
    });
    h.prime(2);
    let link = cause_link_wid();
    h.ui.click(h.ui.rect(link).expect("the widget shows").center());
    let commands = h.frame();
    assert!(
        matches!(
            commands[..],
            [AppCommand::View(ViewCommand::Frame(Framing::ShowSelected))]
        ),
        "{commands:?}"
    );
    let _relayout = h.drain();
    assert_eq!(
        h.session.open.document.main_view.selected,
        BTreeSet::from([ItemId::Node(cause)])
    );

    let before = h.session.open.document.main_view.viewport;
    h.session.frame_graph(Framing::ShowSelected);
    h.frame();
    let _relayout = h.drain();
    assert_ne!(
        h.session.open.document.main_view.viewport, before,
        "the next prepass framed the selection"
    );
}
