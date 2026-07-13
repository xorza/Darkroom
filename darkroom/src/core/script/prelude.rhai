// Built-in Rhai helpers loaded into every darkroom scripting engine.
//
// These wrap the generic `apply` / `apply_all` mutation primitives (and a
// couple of narrowly-scoped Rust helpers, `host::make_add_node` /
// `host::make_move_node`) into ergonomic constructors. Every helper
// specifies *intent only* — the previous state is captured by the editor
// at apply time, so callers never pass a "before" value. The map shapes
// below mirror `crate::edit::intent::Intent`'s serde representation
// (externally-tagged enums; a `NodeId` is a UUID string).
//
// Loaded as a global module by `script::build_engine`, so every function
// defined here is callable from any user script.

// Spawn a new node bound to `func_id` at world-space position `(x, y)`.
// Returns the new node's id (UUID string) so the caller can chain
// follow-up actions (connections, selection, …). `host::make_add_node`
// is a host-internal helper (Func lookup + `From<&Func> for Node`); power
// users can call it directly to inspect or modify the action map before
// applying.
fn create_node(func_id, x, y) {
    let action = host::make_add_node(func_id, x, y);
    apply(action);
    action.AddNode.view_node.id
}

// Remove `node_id` from the graph. The editor walks the graph at apply
// time to capture every reference into the doomed node so undo can fully
// restore it.
fn delete_node(node_id) {
    apply(#{ RemoveNode: #{ node_id: node_id } });
}

// Wire `output_node`'s output port `output_port` into `input_node`'s
// input port `input_port` (a `SetInput` with a `Bind` binding).
fn connect(output_node, output_port, input_node, input_port) {
    apply(#{ SetInput: #{
        node_id: input_node,
        input_idx: input_port,
        to: #{ Bind: #{ node_id: output_node, port_idx: output_port } },
    }});
}

// Drop the binding on `input_node`'s input port `input_port`.
fn disconnect(input_node, input_port) {
    apply(#{ SetInput: #{
        node_id: input_node,
        input_idx: input_port,
        to: "None",
    }});
}

// Move `node_id` to world-space `(x, y)`. `host::make_move_node` builds
// the `MoveNodes` intent in Rust so the prelude needn't know `Vec2`'s
// serde shape.
fn move_node(node_id, x, y) {
    apply(host::make_move_node(node_id, x, y));
}

// Replace the selection with exactly `node_id`. A selection member is
// tagged by kind (`Node`/`Pin` — see `crate::core::document::SelectionKey`);
// this helper only ever selects a node body.
fn select_node(node_id) {
    apply(#{ SetSelection: #{ to: [#{ Node: node_id }] } });
}

// Clear the selection.
fn select_none() {
    apply(#{ SetSelection: #{ to: [] } });
}

// Rename `node_id`.
fn rename_node(node_id, name) {
    apply(#{ RenameNode: #{ node_id: node_id, to: name } });
}
