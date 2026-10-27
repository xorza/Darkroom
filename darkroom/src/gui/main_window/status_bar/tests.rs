use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::app::session::internals::SessionHarness;
use crate::gui::theme::Theme;

/// The rendered line, or `None` where there is no readout at all.
fn rendered(process: u64, cache: RamUsage) -> Option<String> {
    MemoryLabel::resolve(process, cache).map(|label| label.to_string())
}

#[test]
fn memory_label_leads_with_the_process_then_sums_both_cache_pools() {
    const MEM: u64 = 3 * 1024 * 1024;
    // An empty cache leaves the process footprint standing alone.
    assert_eq!(
        rendered(MEM, RamUsage::default()).as_deref(),
        Some("MEM 3.0 MiB")
    );
    // Either pool alone raises the clause on its own.
    assert_eq!(
        rendered(MEM, RamUsage { cpu: 1024, gpu: 0 }).as_deref(),
        Some("MEM 3.0 MiB · Cache 1.0 KiB")
    );
    assert_eq!(
        rendered(MEM, RamUsage { cpu: 0, gpu: 2048 }).as_deref(),
        Some("MEM 3.0 MiB · Cache 2.0 KiB")
    );
    // Both present → one clause carrying the sum: 1024 + 2048 = 3072 B,
    // which is exactly 3.0 KiB.
    assert_eq!(
        rendered(
            MEM,
            RamUsage {
                cpu: 1024,
                gpu: 2048
            }
        )
        .as_deref(),
        Some("MEM 3.0 MiB · Cache 3.0 KiB")
    );
    // No footprint → no readout, even with a populated cache.
    assert_eq!(
        rendered(
            0,
            RamUsage {
                cpu: 1024,
                gpu: 2048
            }
        ),
        None
    );
}

/// The process footprint gives the bar something to say on every frame,
/// so it is recorded on an untouched document — and stays recorded when no
/// reading is available, rather than reappearing as the figure lands.
#[test]
fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
    let mut h = SessionHarness::new(DocumentFixture::default());
    h.prime(2);
    let without =
        h.ui.rect(id())
            .expect("status bar records with no reading and an empty cache");

    h.process_memory = 3 * 1024 * 1024;
    h.prime(2);
    let with = h.ui.rect(id()).expect("status bar records");

    // Padding alone without a reading, a row either way: a collapsed one would arrange to zero
    // height and read as "no bar". A reading adds one body line, 13 px at the theme's factor,
    // which the shaper measures up to a whole pixel: 15.59 to 16.
    let theme = Theme::default();
    let line = theme.palantir.text.line_height_for(theme.text.body).ceil();
    let pad = theme.space.xs;
    assert_eq!(without.size.h, 2.0 * pad, "no reading");
    assert_eq!(with.size.h, 2.0 * pad + line, "a 3 MiB reading");
    for (rect, what) in [(without, "no reading"), (with, "3 MiB reading")] {
        assert_eq!(rect.size.w, 1200.0, "{what}: the bar spans the window");
    }
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
    h.ui.click_at(h.ui.center_of(link));
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
