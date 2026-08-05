use super::*;

use crate::execution::compile::Compiler;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::graph::Graph;
use crate::library::Library;
use crate::worker::protocol::WorkerMessage;

fn batch_intent(msgs: impl IntoIterator<Item = WorkerMessage>) -> BatchIntent {
    let mut intent = BatchIntent::default();
    intent.reset(msgs, []);
    intent
}

/// A trivially-valid `Update` payload for batch reduction tests, which only inspect the
/// reduced intent — the program's content is irrelevant.
fn empty_compiled() -> Arc<CompiledGraph> {
    Compiler::default()
        .compile(&Graph::default(), &Library::default())
        .unwrap()
        .into()
}

#[test]
fn batch_intent_accumulates_simple_flags() {
    let (reply_ack, _ack_rx) = oneshot::channel();
    let node_id = NodeId::from_u128(1);
    let event = EventPort {
        node_id: NodeId::unique(),
        event_idx: 0,
    };

    let mut intent = batch_intent([
        WorkerMessage::Clear,
        WorkerMessage::StartEventLoop,
        WorkerMessage::EvictCache {
            nodes: vec![node_id],
        },
        WorkerMessage::FlushCache {
            nodes: vec![node_id],
        },
        WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        },
        WorkerMessage::Run {
            seeds: RunSeeds::events(vec![event]),
        },
        WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![node_id]),
        },
        WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![node_id]),
        },
        WorkerMessage::Sync { reply: reply_ack },
    ]);

    assert!(matches!(intent.graph_state, Some(GraphOp::Clear)));
    assert!(matches!(intent.loop_request, Some(LoopCommand::Start)));
    assert!(intent.seeds.sinks);
    assert_eq!(intent.seeds.events.len(), 1);
    assert!(intent.seeds.events.contains(&event));
    assert_eq!(
        intent.seeds.node_ids.len(),
        1,
        "duplicate node seeds union to one"
    );
    assert!(intent.seeds.node_ids.contains(&node_id));
    assert!(intent.evict_cache.contains(&node_id));
    assert!(intent.flush_cache.contains(&node_id));
    assert_eq!(intent.syncs.len(), 1);

    let event_capacity = intent.seeds.events.capacity();
    let node_capacity = intent.seeds.node_ids.capacity();
    let eviction_capacity = intent.evict_cache.capacity();
    let flush_capacity = intent.flush_cache.capacity();
    let sync_capacity = intent.syncs.capacity();

    intent.reset([WorkerMessage::StopEventLoop], []);

    assert!(intent.graph_state.is_none());
    assert!(matches!(intent.loop_request, Some(LoopCommand::Stop)));
    assert!(!intent.seeds.sinks);
    assert!(!intent.seeds.event_sources);
    assert!(intent.seeds.events.is_empty());
    assert!(intent.seeds.node_ids.is_empty());
    assert!(intent.evict_cache.is_empty());
    assert!(intent.flush_cache.is_empty());
    assert!(intent.syncs.is_empty());
    assert_eq!(intent.seeds.events.capacity(), event_capacity);
    assert_eq!(intent.seeds.node_ids.capacity(), node_capacity);
    assert_eq!(intent.evict_cache.capacity(), eviction_capacity);
    assert_eq!(intent.flush_cache.capacity(), flush_capacity);
    assert_eq!(intent.syncs.capacity(), sync_capacity);
}

#[test]
fn batch_intent_deduplicates_events() {
    let node_id = NodeId::unique();
    let event = EventPort {
        node_id,
        event_idx: 0,
    };

    let mut intent = BatchIntent::default();
    intent.reset(
        [
            WorkerMessage::Run {
                seeds: RunSeeds::events(vec![event]),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::events(vec![event]),
            },
            WorkerMessage::Run {
                seeds: RunSeeds::events(vec![event, event]),
            },
        ],
        [event],
    );

    assert_eq!(
        intent.seeds.events.len(),
        1,
        "duplicate events must collapse to one"
    );
}

/// The two cache commands accumulate the same way — unique, in the order
/// first named — and into *separate* sets, so a batch that evicts one node
/// and flushes another does both rather than confusing the two.
#[test]
fn batch_intent_accumulates_unique_cache_nodes_in_order() {
    let first = NodeId::from_u128(1);
    let second = NodeId::from_u128(2);
    let third = NodeId::from_u128(3);
    let intent = batch_intent([
        WorkerMessage::EvictCache {
            nodes: vec![first, second],
        },
        WorkerMessage::EvictCache {
            nodes: vec![second, first],
        },
        WorkerMessage::FlushCache {
            nodes: vec![third, first],
        },
        WorkerMessage::FlushCache { nodes: vec![first] },
    ]);

    assert_eq!(
        intent.evict_cache.into_iter().collect::<Vec<_>>(),
        vec![first, second]
    );
    assert_eq!(
        intent.flush_cache.into_iter().collect::<Vec<_>>(),
        vec![third, first]
    );
}

#[test]
fn batch_intent_update_overwrites_earlier_update_in_same_batch() {
    // Two Updates in one batch: the last one wins. This is
    // implicit today (Option::replace) but worth pinning since
    // callers do send [Update(A), Update(B)] during rapid edits.
    let intent = batch_intent([
        WorkerMessage::Update {
            compiled: empty_compiled(),
        },
        WorkerMessage::Update {
            compiled: empty_compiled(),
        },
    ]);

    assert!(matches!(intent.graph_state, Some(GraphOp::Replace(_))));
}

#[test]
fn batch_intent_last_write_wins_per_slot() {
    type Expect = fn(&BatchIntent) -> bool;
    let cases: [(&str, [WorkerMessage; 2], Expect); 4] = [
        (
            "Clear then Update -> last write (Update) wins",
            [
                WorkerMessage::Clear,
                WorkerMessage::Update {
                    compiled: empty_compiled(),
                },
            ],
            |intent| matches!(intent.graph_state, Some(GraphOp::Replace(_))),
        ),
        (
            "Update then Clear -> last write (Clear) wins",
            [
                WorkerMessage::Update {
                    compiled: empty_compiled(),
                },
                WorkerMessage::Clear,
            ],
            |intent| matches!(intent.graph_state, Some(GraphOp::Clear)),
        ),
        (
            "Start then Stop -> last write (Stop) wins",
            [WorkerMessage::StartEventLoop, WorkerMessage::StopEventLoop],
            |intent| matches!(intent.loop_request, Some(LoopCommand::Stop)),
        ),
        (
            "Stop then Start -> last write (Start) wins",
            [WorkerMessage::StopEventLoop, WorkerMessage::StartEventLoop],
            |intent| matches!(intent.loop_request, Some(LoopCommand::Start)),
        ),
    ];

    for (label, msgs, expected) in cases {
        let intent = batch_intent(msgs);
        assert!(expected(&intent), "{label}");
    }
}
