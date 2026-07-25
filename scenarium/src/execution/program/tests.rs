use crate::execution::identity::{ExecutionNodeId, ExecutionOutputPort};
use crate::execution::program::index::{NodeIdx, OutputAddr};
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput, ExecutionProgram, PendingBind,
};

/// Flatten's emit-order buffer: three producers with two output ports each, then
/// a consumer with two inputs. Ids are emitted high-first so adoption has to
/// re-order them.
fn emitted_nodes(program: &mut ExecutionProgram) -> Vec<(ExecutionNodeId, ExecutionNode)> {
    let mut e_nodes: Vec<_> = [3_u128, 1, 2]
        .into_iter()
        .map(|id| {
            let outputs = program
                .outputs
                .append((0..2).map(|_| ExecutionOutput::default()));
            (
                ExecutionNodeId::from_u128(id),
                ExecutionNode {
                    outputs,
                    ..Default::default()
                },
            )
        })
        .collect();
    let inputs = program
        .inputs
        .append((0..2).map(|_| ExecutionInput::default()));
    e_nodes.push((
        ExecutionNodeId::from_u128(4),
        ExecutionNode {
            inputs,
            ..Default::default()
        },
    ));
    e_nodes
}

/// Dense indices follow id order rather than flatten's walk order, and the
/// caller's buffer is left empty for the next compile.
#[test]
fn adopt_nodes_orders_by_id_and_drains_the_flattener_buffer() {
    let mut program = ExecutionProgram::default();
    let mut e_nodes = emitted_nodes(&mut program);
    let capacity = e_nodes.capacity();
    program.adopt_nodes(&mut e_nodes);

    assert!(e_nodes.is_empty(), "adoption drains the buffer");
    assert_eq!(
        e_nodes.capacity(),
        capacity,
        "and leaves its allocation for the next build"
    );
    assert_eq!(
        program.e_node_ids.iter().copied().collect::<Vec<_>>(),
        (1..=4).map(ExecutionNodeId::from_u128).collect::<Vec<_>>(),
        "ids 1..=4 take indices 0..=3 despite id 3 being emitted first"
    );
    for (i, id) in (1..=4).enumerate() {
        assert_eq!(
            program.e_node_index[&ExecutionNodeId::from_u128(id)],
            NodeIdx(i as u32),
            "the reverse map agrees with the id-ordered vector"
        );
    }
}

/// Interning rewrites flatten's id-based fixups into the dense index space. The
/// pools keep emit order, so a node's index and its output range are unrelated.
#[test]
fn intern_bindings_resolves_ids_to_dense_addresses_over_emit_ordered_pools() {
    let mut program = ExecutionProgram::default();
    let mut e_nodes = emitted_nodes(&mut program);
    program.adopt_nodes(&mut e_nodes);

    // The consumer owns input-pool slots 0 and 1; bind them to id 1 port 1 and
    // id 3 port 0, which adoption placed at indices 0 and 2.
    program.intern_bindings(&[
        PendingBind {
            input_idx: 0,
            producer: ExecutionOutputPort {
                e_node_id: ExecutionNodeId::from_u128(1),
                port_idx: 1,
            },
        },
        PendingBind {
            input_idx: 1,
            producer: ExecutionOutputPort {
                e_node_id: ExecutionNodeId::from_u128(3),
                port_idx: 0,
            },
        },
    ]);

    let consumer = program.by_id(ExecutionNodeId::from_u128(4));
    let addresses: Vec<_> = program.inputs[consumer.inputs]
        .iter()
        .map(|input| match input.binding {
            ExecutionBinding::Bind(addr) => addr,
            ref other => panic!("expected an interned bind, got {other:?}"),
        })
        .collect();
    assert_eq!(
        addresses,
        [
            OutputAddr {
                node_idx: NodeIdx(0),
                port_idx: 1
            },
            OutputAddr {
                node_idx: NodeIdx(2),
                port_idx: 0
            },
        ]
    );

    // Emit order 3, 1, 2 gave out output-pool starts 0, 2, 4. So id 1 sits at
    // index 0 while owning slots 2..4, and id 3 at index 2 owning slots 0..2.
    assert_eq!(program.output_idx(addresses[0]).idx(), 3);
    assert_eq!(program.output_idx(addresses[1]).idx(), 0);

    // A producer that was never adopted is a flatten bug, not bad input, so it
    // panics here rather than degrading to an unbound input.
    let unemitted = ExecutionNodeId::from_u128(9);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        program.intern_bindings(&[PendingBind {
            input_idx: 0,
            producer: ExecutionOutputPort {
                e_node_id: unemitted,
                port_idx: 0,
            },
        }]);
    }));
    let payload = panic.expect_err("interning an unemitted producer must panic");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(
        message.contains("flatten only binds producers it emitted"),
        "the panic must name the violated invariant, got {message:?}"
    );
}
