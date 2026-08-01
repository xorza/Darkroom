use std::collections::BTreeSet;

use glam::{UVec2, Vec2};
use palantir::WidgetId;
use scenarium::{Binding, InputPort, NodeId};

use crate::core::document::harness::DocFixture;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::harness::CanvasHarness;

/// Room for three nodes in a row *and* the opened menu below them.
const SURFACE: UVec2 = UVec2::new(1600, 900);

/// How far down from the anchor [`menu_rows`] sweeps, how far in from its
/// left edge, and how fine its step. The sweep has to clear the whole menu,
/// and the step has to be finer than its thinnest band (a separator) or a row
/// would read as two.
const PROBE_DEPTH: f32 = 320.0;
const PROBE_INSET: f32 = 20.0;
const PROBE_STEP: f32 = 2.0;

/// The open node menu's clickable rows, top to bottom.
///
/// [`MenuItem`](palantir::MenuItem) takes no caller-supplied id, so a test
/// finds the rows the way a pointer does: sweep a vertical line down the open
/// popup and keep every widget it lands on. The popup's own panel shows
/// through the padding above the first row, the gaps between rows, and every
/// separator, so it is the one widget the sweep meets repeatedly — and its
/// first and last sightings bracket exactly the rows.
fn menu_rows(h: &CanvasHarness, anchor: Vec2) -> Vec<WidgetId> {
    let mut runs: Vec<WidgetId> = Vec::new();
    let mut y = anchor.y;
    while y < anchor.y + PROBE_DEPTH {
        if let Some(id) = h.ui.hit_at(Vec2::new(anchor.x + PROBE_INSET, y))
            && runs.last() != Some(&id)
        {
            runs.push(id);
        }
        y += PROBE_STEP;
    }
    let sightings = |id: &WidgetId| runs.iter().filter(|other| *other == id).count();
    let panel = *runs
        .iter()
        .max_by_key(|id| sightings(id))
        .expect("the sweep crossed the open menu");
    let first = runs.iter().position(|id| *id == panel).unwrap();
    let last = runs.iter().rposition(|id| *id == panel).unwrap();
    runs[first..last]
        .iter()
        .copied()
        .filter(|id| *id != panel)
        .collect()
}

/// Every row the menu offers over a runnable node, in layout order.
const RUN: usize = 0;
const DUPLICATE: usize = 1;
const DUPLICATE_WITH_INCOMING: usize = 2;
const REMOVE: usize = 3;
const ROW_COUNT: usize = 4;

/// Open the menu on `on`, click its `row`th item, and return the intents the
/// picking frame raised. The open and the pick are separate frames because
/// the menu has to record before an item can be hit — the same order the real
/// app sees them in, and what lets the pick read back the selection the open
/// committed.
fn pick(h: &mut CanvasHarness, on: NodeId, row: usize) -> Vec<GraphIntent> {
    // Two frames so every node body has recorded and carries a hit-testable
    // rect to aim the right-click at, then two more so the opened menu has.
    h.prime(2);
    let anchor = h.node_center(on);
    h.ui.right_click_at(anchor);
    h.prime(2);
    let rows = menu_rows(h, anchor);
    assert_eq!(
        rows.len(),
        ROW_COUNT,
        "the sweep found rows the menu does not offer, or missed some"
    );
    h.ui.click_on(rows[row]);
    h.frame()
}

/// A node-body right-click selects the node it landed on before the menu
/// opens, so whatever the user picks next acts on a coherent set.
///
/// It comes out of the closure the shared trigger scan takes — "which of this
/// node's widgets opens the menu" — so it's checked through a real click
/// rather than by calling it.
#[test]
fn a_node_body_right_click_selects_the_node_it_landed_on() {
    // Two nodes, so the assertion that exactly one ends up selected has
    // something to exclude, on a surface wide enough for the opened menu.
    let mut h = CanvasHarness::sized(DocFixture::probes(2), SURFACE);
    let func = h.node(0);
    // Two frames so every node body has recorded and carries a hit-testable
    // rect for the click below.
    h.prime(2);

    let on_func = h.node_center(func);
    h.ui.right_click_at(on_func);
    let intents = h.frame();
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }] if to.len() == 1 && to.contains(&func),
        ),
        "the right-click selects exactly the node it opened on: {intents:?}"
    );
}

/// "Remove" acts on the whole selection, not only the node the menu opened
/// on — one intent per member, which the drain batches into a single undo
/// entry. Opening on a node already in the selection leaves that selection
/// alone, so both members survive to the pick.
#[test]
fn remove_pick_removes_every_selected_node() {
    let mut h = CanvasHarness::shaping_text(DocFixture::probes(3), SURFACE);
    let (a, b) = (h.node(0), h.node(1));
    h.doc_mut().main_view.selected = [a, b].into_iter().collect();

    let intents = pick(&mut h, a, REMOVE);
    let removed: BTreeSet<NodeId> = intents
        .iter()
        .filter_map(|intent| match intent {
            GraphIntent::RemoveNode { node_id } => Some(*node_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        removed,
        BTreeSet::from([a, b]),
        "both selected nodes go, the third stays: {intents:?}"
    );
    assert_eq!(intents.len(), 2, "and nothing rides along: {intents:?}");
}

/// The two Duplicate rows differ in exactly one thing: whether a clone keeps
/// its wire to a producer outside the selection. With `c -> b` crossing that
/// boundary, the plain pick drops the wire and the "with incoming" pick keeps
/// it pointed at the original `c`.
#[test]
fn duplicate_picks_differ_by_whether_incoming_wires_survive() {
    // `(binding count, the one binding)` the `row`th pick's clone carries.
    let duplicate_via = |row: usize| {
        let mut h = CanvasHarness::shaping_text(DocFixture::probes(3), SURFACE);
        let (a, b, c) = (h.node(0), h.node(1), h.node(2));
        // The one wire crossing the selection boundary: b reads c, and only
        // {a, b} are selected.
        h.doc_mut()
            .graph
            .set_input_binding(InputPort::new(b, 0), Binding::bind(c, 0));
        h.doc_mut().main_view.selected = [a, b].into_iter().collect();

        let intents = pick(&mut h, a, row);
        let [
            GraphIntent::DuplicateNodes {
                nodes, bindings, ..
            },
        ] = &intents[..]
        else {
            panic!("expected exactly one DuplicateNodes intent, got {intents:?}");
        };
        assert_eq!(nodes.len(), 2, "the two selected nodes are cloned");
        (bindings.len(), bindings.first().map(|(_, b)| b.clone()), c)
    };

    let (dropped, _, _) = duplicate_via(DUPLICATE);
    let (kept, binding, c) = duplicate_via(DUPLICATE_WITH_INCOMING);
    assert_eq!(dropped, 0, "the external wire is dropped");
    assert_eq!(kept, 1, "the external wire is kept");
    match binding {
        Some(Binding::Bind(src)) => {
            assert_eq!(src.node_id, c, "still fed by the original producer");
            assert_eq!(src.port_idx, 0);
        }
        other => panic!("expected a Bind to c, got {other:?}"),
    }
}

/// "Run to this node" is the one pick that names the node the menu opened on
/// rather than the selection, and the one that leaves the graph alone — it
/// surfaces as a command instead of an intent.
#[test]
fn run_pick_raises_no_intent() {
    let mut h = CanvasHarness::shaping_text(DocFixture::probes(2), SURFACE);
    let a = h.node(0);
    h.doc_mut().main_view.selected = [a].into_iter().collect();

    let intents = pick(&mut h, a, RUN);
    assert!(
        intents.is_empty(),
        "running a node edits nothing: {intents:?}"
    );
}
