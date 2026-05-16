use super::*;
use crate::gui::graph_ui::gesture::Gesture;
use crate::model::argument_values_cache::{CacheEvent, NodeCache};
use scenarium::graph::NodeId;

/// Reset must replace every per-graph field, including the cache, so
/// renderer state can never outlive the graph it was built for.
#[test]
fn reset_clears_cache_and_gesture() {
    let mut graph_ui = GraphUi::default();

    // Seed cache + gesture with non-default state so we can prove
    // they're discarded.
    let node_id = NodeId::unique();
    graph_ui
        .argument_values_cache
        .insert(node_id, NodeCache::default());
    graph_ui.gesture.start_panning();
    assert!(graph_ui.argument_values_cache.get_mut(&node_id).is_some());
    assert!(graph_ui.gesture.is_panning());

    graph_ui.apply_render_events(vec![RenderEvent::Reset]);

    assert!(graph_ui.argument_values_cache.get_mut(&node_id).is_none());
    assert!(matches!(graph_ui.gesture, Gesture::Idle));
}

/// Cache events queued *after* a Reset in the same batch must apply
/// to the freshly-defaulted state — Session pushes Reset first in
/// `replace_graph` and downstream events should not be lost.
#[test]
fn cache_event_after_reset_applies_to_fresh_state() {
    let mut graph_ui = GraphUi::default();
    let stale_id = NodeId::unique();
    let fresh_id = NodeId::unique();
    graph_ui
        .argument_values_cache
        .insert(stale_id, NodeCache::default());

    graph_ui.apply_render_events(vec![
        RenderEvent::Reset,
        RenderEvent::Cache(CacheEvent::Insert(fresh_id, NodeCache::default())),
    ]);

    assert!(graph_ui.argument_values_cache.get_mut(&stale_id).is_none());
    assert!(graph_ui.argument_values_cache.get_mut(&fresh_id).is_some());
}

/// Without a Reset, cache events apply incrementally without
/// touching gesture/popup/layout state.
#[test]
fn cache_events_alone_preserve_non_cache_state() {
    let mut graph_ui = GraphUi::default();
    graph_ui.gesture.start_panning();

    let node_id = NodeId::unique();
    graph_ui.apply_render_events(vec![RenderEvent::Cache(CacheEvent::Insert(
        node_id,
        NodeCache::default(),
    ))]);

    assert!(graph_ui.argument_values_cache.get_mut(&node_id).is_some());
    assert!(graph_ui.gesture.is_panning());
}
