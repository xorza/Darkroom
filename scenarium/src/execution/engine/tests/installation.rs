use super::*;

use crate::testing::program::ProgramBuilder;

/// A program of `ids`, nothing but identities — enough for the pairing the
/// engine establishes at install.
fn program(ids: &[NodeId]) -> Arc<CompiledGraph> {
    let mut prog = ProgramBuilder::default();
    for &node_id in ids {
        prog.node().id(node_id).add();
    }
    Arc::new(prog.into_program())
}

#[test]
fn install_holds_one_canonical_artifact_for_the_engine_and_its_cache() {
    let compiled = program(&[NodeId::from_u128(1)]);
    let mut engine = ExecutionEngine::default();

    engine.install(Arc::clone(&compiled));

    assert!(Arc::ptr_eq(engine.compiled.as_ref().unwrap(), &compiled));
    engine.validate().unwrap();
}

/// A reinstall re-pairs the slots by stable id against the program being
/// left, which the engine supplies — so a node that survives the recompile
/// keeps its slot even when its dense index moves.
#[test]
fn install_carries_slots_across_a_shifted_index_space() {
    let surviving = NodeId::from_u128(2);

    let mut engine = ExecutionEngine::default();
    engine.install(program(&[NodeId::from_u128(1), surviving]));
    // Index 1 before the recompile: ids place in ascending order.
    engine.cache[NodeIdx(1)].state.set(17_u32);

    // Node 1 is dropped, so the survivor slides to index 0.
    engine.install(program(&[surviving, NodeId::from_u128(3)]));

    assert_eq!(engine.cache[NodeIdx(0)].state.get::<u32>(), Some(&17));
    assert!(engine.cache[NodeIdx(1)].state.is_none());
    engine.validate().unwrap();
}

#[test]
fn validation_rejects_a_cache_with_the_wrong_node_count() {
    let engine = ExecutionEngine {
        compiled: Some(program(&[NodeId::from_u128(1)])),
        ..Default::default()
    };

    assert_eq!(
        engine.validate().unwrap_err().to_string(),
        "runtime cache spans 0 nodes, not the compiled program's 1"
    );
}

/// A func-only graph builds with the node ids unchanged (caches survive).
#[test]
fn top_level_func_nodes_keep_identity() {
    let e = TestEngine::over(TestGraph::sample());

    assert_eq!(e.engine.compiled().e_nodes.len(), e.graph.graph.len());
    for node in e.graph.graph.iter() {
        assert!(e.engine.compiled().contains(node.id), "id preserved");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_resets_graph() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    e.run_sinks().await;
    assert!(!e.engine.compiled().e_nodes.is_empty());

    e.engine.clear();

    assert!(e.engine.compiled.is_none());
    assert!(e.engine.schedule.process_order.is_empty());
    assert_eq!(e.engine.cache.slot_count(), 0);
    Ok(())
}
