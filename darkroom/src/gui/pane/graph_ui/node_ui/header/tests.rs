use glam::{UVec2, Vec2};
use scenarium::{CacheMode, NodeId};
use std::mem;

use crate::core::document::internals::DocumentFixture;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::node_property::NodeProperty;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::node_ui::header::cache_button_wid;
use crate::gui::pane::graph_ui::wid;

/// Room for the node and the cache menu opened below its header.
const SURFACE: UVec2 = UVec2::new(1200, 800);

/// The header is one row, left to right: run, the title, then the cache and inspect buttons
/// pinned to the right edge, each centered on the same line.
#[test]
fn the_header_is_one_row_of_run_title_cache_and_inspect() {
    let mut h = CanvasHarness::sized(DocumentFixture::probes(1), SURFACE);
    let node_id = h.node(0);
    h.prime(2);
    let rect = |h: &CanvasHarness, id| h.ui.rect(id).expect("recorded");
    let row = [
        wid::of_node("run_button", node_id),
        wid::rename(node_id),
        cache_button_wid(node_id),
        wid::inspect_button(node_id),
    ]
    .map(|id| rect(&h, id));
    for pair in row.windows(2) {
        assert!(pair[0].max().x <= pair[1].min.x, "{pair:?}");
        let center = |r: palantir::Rect| r.min.y + r.size.h * 0.5;
        assert!((center(pair[0]) - center(pair[1])).abs() < 1.0, "{pair:?}");
    }
    let body = h.node_rect(node_id);
    assert_eq!(
        row[3].max().x + 12.0,
        body.max().x - 2.0,
        "the inspect button ends the row"
    );
}

/// What one pick in the cache menu raised, in the two tiers it can reach.
#[derive(Debug)]
struct Pick {
    node_id: NodeId,
    intents: Vec<GraphIntent>,
    commands: Vec<AppCommand>,
}

/// The cache menu's rows, in order.
const RAM: usize = 0;
const DISK: usize = 1;
const CLEAR: usize = 2;

/// Open the cache menu of the fixture's only node, sitting at `from`, and pick its `row`.
fn pick(from: CacheMode, row: usize) -> Pick {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(1), SURFACE);
    let node_id = h.node(0);
    h.document_mut().graph.find_mut(node_id).unwrap().cache = from;
    h.prime(2);
    let button =
        h.ui.rect(cache_button_wid(node_id))
            .expect("a cacheable node's button");
    h.ui.click_at(h.ui.center_of(cache_button_wid(node_id)));
    h.frame();
    h.prime(2);
    let rows = h.menu_rows(Vec2::new(button.min.x, button.max().y));
    assert_eq!(rows.len(), 3, "RAM, disk and clear: {rows:?}");
    mem::take(&mut h.commands);
    h.ui.click_on(rows[row]);
    let intents = h.frame();
    Pick {
        node_id,
        intents,
        commands: mem::take(&mut h.commands),
    }
}

fn cache_edits(pick: &Pick) -> Vec<CacheMode> {
    pick.intents
        .iter()
        .filter_map(|intent| match intent {
            GraphIntent::SetNodeProperty {
                node_id,
                to: NodeProperty::RuntimeCache(mode),
            } if *node_id == pick.node_id => Some(*mode),
            _ => None,
        })
        .collect()
}

fn flushed(pick: &Pick) -> bool {
    pick.commands.iter().any(|command| {
        matches!(command, AppCommand::Run(RunCommand::FlushCache(id)) if *id == pick.node_id)
    })
}

/// Each row flips its own bit and keeps the other. Turning the disk bit **on** also flushes
/// what the node holds in RAM to disk, without which the value would reach disk only when
/// the node next recomputed; turning it off, or flipping RAM, flushes nothing.
#[test]
fn each_cache_row_flips_its_bit_and_the_disk_row_flushes_when_it_turns_on() {
    let cases = [
        (CacheMode::None, RAM, CacheMode::Ram, false),
        (CacheMode::Both, RAM, CacheMode::Disk, false),
        (CacheMode::None, DISK, CacheMode::Disk, true),
        (CacheMode::Ram, DISK, CacheMode::Both, true),
        (CacheMode::Disk, DISK, CacheMode::None, false),
        (CacheMode::Both, DISK, CacheMode::Ram, false),
    ];
    for (from, row, to, flushes) in cases {
        let pick = pick(from, row);
        assert_eq!(cache_edits(&pick), [to], "{from:?}, row {row}");
        assert_eq!(
            flushed(&pick),
            flushes,
            "{from:?}, row {row}: {:?}",
            pick.commands
        );
    }
}

/// Clear cache evicts the node's caches and edits nothing.
#[test]
fn clear_cache_evicts_and_edits_nothing() {
    let pick = pick(CacheMode::Ram, CLEAR);
    assert!(cache_edits(&pick).is_empty(), "{:?}", pick.intents);
    assert!(
        matches!(pick.commands[..], [AppCommand::Run(RunCommand::EvictCache(id))] if id == pick.node_id),
        "{:?}",
        pick.commands
    );
}

/// The inspect button selects its node alone and shows the inspector that follows the
/// selection: one tab, whichever node's button opened it.
#[test]
fn the_inspect_button_selects_its_node_and_opens_one_inspector() {
    use std::collections::BTreeSet;

    use crate::core::document::inspect_target::InspectTarget;
    use crate::core::document::item_id::ItemId;
    use crate::core::document::tab_ref::TabRef;
    use crate::gui::app::session::internals::SessionHarness;

    let mut h = SessionHarness::new(DocumentFixture::probes(2));
    let ids: Vec<NodeId> = h
        .session
        .open
        .document
        .graph
        .iter()
        .map(|node| node.node_id)
        .collect();
    let inspectors = |h: &SessionHarness| {
        h.session
            .open
            .document
            .layout
            .all_tabs()
            .filter(|tab| *tab == TabRef::Inspector(InspectTarget::Selection))
            .count()
    };
    for node_id in [ids[0], ids[1]] {
        h.prime(2);
        h.ui.click_at(h.ui.center_of(wid::inspect_button(node_id)));
        h.frame();
        let _relayout = h.drain();
        assert_eq!(
            h.session.open.document.main_view.selected,
            BTreeSet::from([ItemId::Node(node_id)])
        );
        assert_eq!(inspectors(&h), 1, "one inspector tab");
    }
}
