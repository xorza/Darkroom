use glam::{UVec2, Vec2};
use palantir::{EditableLabel, PointerButton};
use scenarium::{CacheMode, Keep, NodeAddr, NodeId, PortId, TriggerPort};
use scenarium::{GraphId, internals::test_graph::TestGraph};
use std::mem;
use std::sync::Arc;

use crate::core::document::internals::DocumentFixture;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::node_property::NodeProperty;
use crate::core::preview_sink;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::node_ui::header::cache_button_wid;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortRef;

/// Room for the node and the cache menu opened below its header.
const SURFACE: UVec2 = UVec2::new(1200, 800);

/// The header is one row, left to right: run, the title, then the cache and inspect buttons
/// pinned to the right edge, each centered on the same line. A button is its 16 px icon with 6 px
/// a side, 16 + 2 · 6 = 28 square at zoom 1.
#[test]
fn the_header_is_one_row_of_run_title_cache_and_inspect() {
    let mut h = CanvasHarness::sized(DocumentFixture::probes(1), SURFACE);
    let node_id = h.node(0);
    h.prime(2);
    let rect = |h: &CanvasHarness, id| h.ui.rect(id).expect("recorded");
    let row = [
        wid::of_node("run_button", GraphId::Main, node_id),
        wid::rename(GraphId::Main, node_id),
        cache_button_wid(GraphId::Main, node_id),
        wid::inspect_button(GraphId::Main, node_id),
    ]
    .map(|id| rect(&h, id));
    for pair in row.windows(2) {
        assert!(pair[0].max().x <= pair[1].min.x, "{pair:?}");
        let center = |r: palantir::Rect| r.min.y + r.size.h * 0.5;
        assert!((center(pair[0]) - center(pair[1])).abs() < 1.0, "{pair:?}");
    }
    for button in &row[2..] {
        assert_eq!((button.size.w, button.size.h), (28.0, 28.0), "{button:?}");
    }
    let body = h.node_rect(node_id);
    assert_eq!(
        row[3].max().x + 12.0,
        body.max().x - 2.0,
        "the inspect button ends the row"
    );
}

/// A node's own trigger hangs on its left edge at the header's middle, as an input port's circle
/// hangs on its row: its center on the body's left edge, as the input's, level with the title, and the run button
/// after it keeps the place it has on a node with no trigger.
#[test]
fn a_nodes_own_trigger_hangs_on_its_left_edge_level_with_the_title() {
    // A glyph hung on the edge lands 1/320 px past it, the input's circle as the trigger, so the
    // edge, the title's middle and the run button are held to 0.01 px, far under what a screen
    // shows.
    const SUBPIXEL: f32 = 0.01;
    let mut fixture = DocumentFixture::probes(1);
    let sink = fixture.node_at(Vec2::new(40.0, 300.0));
    let mut h = CanvasHarness::sized(fixture, SURFACE);
    let probe = h.node(0);
    h.prime(2);
    let rect = |h: &CanvasHarness, id| h.ui.rect(id).expect("recorded");
    let middle = |r: palantir::Rect| r.min.y + r.size.h * 0.5;
    let trigger = rect(
        &h,
        wid::trigger_glyph(GraphId::Main, TriggerPort::from(sink)),
    );
    let title = rect(&h, wid::rename(GraphId::Main, sink));
    let body = h.node_rect(sink);
    let input = rect(
        &h,
        wid::port_circle(GraphId::Main, PortRef::input(sink, PortId::new(1))),
    );
    assert_eq!(
        trigger.center().x,
        input.center().x,
        "as the input's circle hangs"
    );
    assert!(
        (trigger.center().x - body.min.x).abs() < SUBPIXEL,
        "{trigger:?} on {body:?}"
    );
    assert!(
        (middle(trigger) - middle(title)).abs() < SUBPIXEL,
        "{trigger:?} level with {title:?}"
    );
    let run_offset = |h: &CanvasHarness, node_id| {
        rect(h, wid::of_node("run_button", GraphId::Main, node_id))
            .min
            .x
            - h.node_rect(node_id).min.x
    };
    assert!(
        (run_offset(&h, sink) - run_offset(&h, probe)).abs() < SUBPIXEL,
        "the trigger takes no room in the row"
    );
}

/// A left drag on a node's title moves the node, and with its rename field open it selects text and
/// moves nothing.
#[test]
fn a_drag_on_a_title_moves_the_node_unless_its_field_is_open() {
    let drag_latches = |open: bool| {
        let mut h = CanvasHarness::sized(DocumentFixture::probes(1), SURFACE);
        let title = wid::rename(GraphId::Main, h.node(0));
        h.prime(2);
        let at = h.ui.rect(title).expect("recorded").center();
        if open {
            for _ in 0..2 {
                h.ui.click(at);
                h.frame();
            }
            h.ui.advance_past_double_click();
        }
        assert_eq!(EditableLabel::is_editing(h.ui.ui(), title), open);
        h.ui.press(at, PointerButton::Left);
        h.frame();
        h.ui.drag_to(at + Vec2::new(40.0, 0.0));
        let intents = h.frame();
        let moved = intents
            .iter()
            .any(|intent| matches!(intent, GraphIntent::MoveSelection { .. }));
        assert_eq!(moved, h.graph_ui.gestures.group_drag.in_flight());
        moved
    };
    assert!(drag_latches(false));
    assert!(!drag_latches(true));
}

/// What one pick in the cache menu raised, in the two tiers it can reach.
#[derive(Debug)]
struct Pick {
    node_id: NodeId,
    intents: Vec<GraphIntent>,
    commands: Vec<AppCommand>,
}

/// The cache menu's rows, in order.
/// The cache menu's rows, top to bottom: how long RAM keeps the output, then the disk bit, then
/// Clear cache.
const AUTO: usize = 0;
const ALWAYS: usize = 1;
const NEVER: usize = 2;
const DISK: usize = 3;
const CLEAR: usize = 4;

/// Open the cache menu of the fixture's only node, sitting at `from`, and pick its `row`.
fn pick(from: CacheMode, row: usize) -> Pick {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(1), SURFACE);
    let node_id = h.node(0);
    *h.document_mut()
        .graphs
        .main_mut()
        .find_mut(node_id)
        .unwrap()
        .cache_mut()
        .unwrap() = from;
    h.prime(2);
    let button =
        h.ui.rect(cache_button_wid(GraphId::Main, node_id))
            .expect("a cacheable node's button");
    h.ui.click(
        h.ui.rect(cache_button_wid(GraphId::Main, node_id))
            .expect("the widget shows")
            .center(),
    );
    h.frame();
    h.prime(2);
    let rows = h.menu_rows(Vec2::new(button.min.x, button.max().y));
    assert_eq!(rows.len(), 5, "three RAM rows, disk and clear: {rows:?}");
    mem::take(&mut h.commands);
    h.ui.click(rows[row]);
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
        matches!(command, AppCommand::Run(RunCommand::FlushCache(addr)) if *addr == NodeAddr::new(GraphId::Main, pick.node_id))
    })
}

/// A RAM row sets how long RAM keeps the output and keeps the disk bit, and the row already
/// picked edits nothing. The disk row flips its bit and keeps the RAM choice; turning it on also
/// flushes what the node holds in RAM to disk, and turning it off or picking RAM flushes nothing.
#[test]
fn each_cache_row_sets_its_choice_and_the_disk_row_flushes_when_it_turns_on() {
    let mode = |ram, disk| CacheMode { ram, disk };
    let cases = [
        (CacheMode::AUTO, ALWAYS, Some(CacheMode::ALWAYS), false),
        (CacheMode::NEVER, AUTO, Some(CacheMode::AUTO), false),
        (
            mode(Keep::Always, true),
            NEVER,
            Some(mode(Keep::Never, true)),
            false,
        ),
        (CacheMode::ALWAYS, ALWAYS, None, false),
        (CacheMode::AUTO, DISK, Some(mode(Keep::Auto, true)), true),
        (
            mode(Keep::Always, true),
            DISK,
            Some(CacheMode::ALWAYS),
            false,
        ),
    ];
    for (from, row, to, flushes) in cases {
        let pick = pick(from, row);
        assert_eq!(
            cache_edits(&pick),
            Vec::from_iter(to),
            "{from:?}, row {row}"
        );
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
    let pick = pick(CacheMode::ALWAYS, CLEAR);
    assert!(cache_edits(&pick).is_empty(), "{:?}", pick.intents);
    assert!(
        matches!(pick.commands[..], [AppCommand::Run(RunCommand::EvictCache(addr))] if addr == NodeAddr::new(GraphId::Main, pick.node_id)),
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
        .document()
        .graphs
        .main()
        .iter()
        .map(|node| node.node_id)
        .collect();
    let inspectors = |h: &SessionHarness| {
        h.session
            .open
            .document()
            .layout
            .all_tabs()
            .filter(|tab| *tab == TabRef::Inspector(InspectTarget::Selection))
            .count()
    };
    for node_id in [ids[0], ids[1]] {
        h.prime(2);
        h.ui.click(
            h.ui.rect(wid::inspect_button(GraphId::Main, node_id))
                .expect("the widget shows")
                .center(),
        );
        h.frame();
        h.drain();
        assert_eq!(
            h.session.open.document().main_view.selected,
            BTreeSet::from([ItemId::Node(node_id)])
        );
        assert_eq!(inspectors(&h), 1, "one inspector tab");
    }
}

/// A preview node is a sink, and says so with one kind mark between its title and the controls
/// pinned right, not a mark for each kind.
#[test]
fn a_preview_node_shows_one_kind_mark_after_its_title() {
    let mut fixture = DocumentFixture::default();
    let preview = fixture.add(&preview_sink::func(Arc::default()));
    let mut h = CanvasHarness::sized(fixture, SURFACE);
    h.prime(2);
    let title =
        h.ui.rect(wid::rename(GraphId::Main, preview))
            .expect("recorded");
    let mark =
        h.ui.rect(wid::of_node("kind_mark", GraphId::Main, preview))
            .expect("a preview has a kind mark");
    let disable =
        h.ui.rect(wid::of_node("disable_button", GraphId::Main, preview))
            .expect("a sink can be disabled");
    assert!(title.max().x <= mark.min.x && mark.max().x <= disable.min.x);
    for gone in ["sink_mark", "impure_mark"] {
        assert_eq!(
            h.ui.rect(wid::of_node(gone, GraphId::Main, preview)),
            None,
            "{gone}"
        );
    }
}

/// A node with no measured size is laid out unseen, its trigger too, so neither takes a
/// pointer, and it owes a relayout, whose pass shows both. Removed and restored by an undo, a sink
/// has lost its size, as a node first recorded has none.
#[test]
fn a_node_and_its_pin_show_once_measured() {
    use crate::core::edit::document_request::DocumentRequest;

    let mut h = CanvasHarness::new(TestGraph::source_sink_loose());
    h.prime(2);
    let sink = h
        .ctx
        .graph_ctx()
        .nodes()
        .find(|node| node.sink())
        .expect("the fixture holds a sink")
        .node_id;
    let parts = [
        wid::body(GraphId::Main, sink),
        wid::trigger_glyph(GraphId::Main, TriggerPort::from(sink)),
    ];
    let listed = |h: &CanvasHarness| {
        parts.map(|part| h.ui.widgets(None).iter().any(|widget| widget.id == part))
    };
    assert_eq!(listed(&h), [true, true], "a measured node shows");

    let _removed = h.ctx.open.apply_edits(
        [DocumentRequest::Graph {
            graph: GraphId::Main,
            intent: GraphIntent::RemoveNode { node_id: sink },
        }],
        &h.ctx.library,
    );
    h.frame();
    h.graph_ui.retain_nodes(h.ctx.open.document().graphs.main());
    let _restored = h.ctx.open.undo();
    h.frame();
    assert_eq!(
        listed(&h),
        [true, true],
        "the first pass laid it out unseen, and the relayout it owed shows it, measured",
    );
}

/// An instance draws its own trigger while its interface declares one, as a sink does; one whose
/// interface declares none draws none, and a ghost trigger where a ghost subscription ends on it.
#[test]
fn an_instance_has_a_trigger_while_its_interface_declares_one() {
    use scenarium::{
        Compound, CompoundId, EventDecl, EventPort, EventPorts, GhostSubscription, Interface,
        PortId, Subscription, SubscriptionState,
    };

    use crate::core::document::internals::insert_compound;

    let mut fixture = DocumentFixture::probes(0);
    let emitter = fixture.node_at(Vec2::new(40.0, 40.0));
    let [triggered, plain] = [1, 2].map(CompoundId::from_u128);
    let interface = Interface::default()
        .with_event_ports(EventPorts {
            events: vec![],
            trigger: Some(EventDecl::new("go")),
        })
        .unwrap();
    insert_compound(
        &mut fixture.document,
        triggered,
        Compound::new("triggered", interface),
    );
    insert_compound(
        &mut fixture.document,
        plain,
        Compound::new("plain", Interface::default()),
    );
    let with_trigger = fixture.add_instance(triggered);
    let without_trigger = fixture.add_instance(plain);
    fixture.document.graphs.main_mut().set_subscription_state(
        Subscription::new(EventPort::new(emitter, PortId::new(1)), without_trigger),
        SubscriptionState::Ghost(GhostSubscription::default()),
    );
    let mut h = CanvasHarness::new(fixture);
    h.prime(2);
    let shows = |id| h.ui.rect(id).is_some();
    assert!(shows(wid::trigger_glyph(
        GraphId::Main,
        TriggerPort::from(with_trigger)
    )));
    assert!(!shows(wid::ghost_trigger_glyph(
        GraphId::Main,
        TriggerPort::from(with_trigger)
    )));
    assert!(!shows(wid::trigger_glyph(
        GraphId::Main,
        TriggerPort::from(without_trigger)
    )));
    assert!(shows(wid::ghost_trigger_glyph(
        GraphId::Main,
        TriggerPort::from(without_trigger)
    )));
}

/// An instance wears a compound mark after its title and a func's node none; a press on it opens
/// the compound through the instance. Its glyph says whether the compound takes edits: a small
/// graph while it does, a padlock while locked, a chain while it follows the library.
#[test]
fn an_instances_compound_mark_opens_it_and_says_whether_it_takes_edits() {
    use scenarium::{Compound, CompoundId, InstancePath, Interface};

    use crate::core::document::compound_side::ReadOnly;
    use crate::core::document::internals::insert_compound;
    use crate::gui::pane::graph_ui::node_ui::header::compound_look;
    use crate::gui::widgets::icon::Icon;

    let mut fixture = DocumentFixture::probes(1);
    let id = CompoundId::from_u128(1);
    insert_compound(
        &mut fixture.document,
        id,
        Compound::new("inner", Interface::default()),
    );
    let instance = fixture.add_instance(id);
    let mut h = CanvasHarness::sized(fixture, SURFACE);
    let probe = h.node(0);
    h.prime(2);
    let mark = |node| wid::of_node("compound_mark", GraphId::Main, node);
    assert!(h.ui.rect(mark(probe)).is_none(), "a func's node has none");
    h.ui.click(mark(instance));
    h.frame();
    assert_eq!(h.entries, [(id, Some(InstancePath::new(&[instance])))]);

    let glyph = |read_only| compound_look(read_only).0;
    assert_eq!(glyph(None), Icon::Compound);
    assert_eq!(glyph(Some(ReadOnly::Locked)), Icon::Lock);
    assert_eq!(glyph(Some(ReadOnly::Follows)), Icon::Link);
}
