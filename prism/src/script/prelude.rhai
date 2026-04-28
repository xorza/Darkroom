// Built-in Rhai helpers loaded into every prism scripting engine.
//
// These wrap the generic `apply` / `apply_all` mutation primitives
// (and a few narrowly-scoped Rust helpers like `host::make_add_node`)
// into ergonomic constructors. Every helper specifies *intent only* —
// the previous state is captured by Session at apply time, so callers
// never have to pass a "before" value. Adding a new helper here is
// one Rhai function unless the action requires data only the host has
// (FuncLib lookups, graph walks).
//
// Loaded as a global module by `script::build_engine`, so every
// function defined here is callable from any user script.

// Spawn a new node bound to `func_id` at view-space position `(x, y)`.
// Returns the new node's id (UUID string) so the caller can chain
// follow-up actions (connections, selection, …). `host::make_add_node`
// is a host-internal helper (Func lookup + `From<&Func> for Node`
// conversion); power users can call it directly to inspect or modify
// the action map before applying.
fn create_node(func_id, x, y) {
    let action = host::make_add_node(func_id, x, y);
    apply(action);
    action.AddNode.view_node.id
}

// Remove `node_id` from the graph. Session walks the graph at apply
// time to capture every reference into the doomed node so undo can
// fully restore it.
fn delete_node(node_id) {
    apply(#{ RemoveNode: #{ node_id: node_id } });
}

// Wire `output_node`'s output port `output_port` into `input_node`'s
// input port `input_port`. Names match the port kinds so the `to:`
// field reads unambiguously as "the new binding value".
fn connect(output_node, output_port, input_node, input_port) {
    apply(#{ SetInput: #{
        node_id: input_node,
        input_idx: input_port,
        to: #{ Bind: #{ target_id: output_node, port_idx: output_port } },
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

// Move `node_id` to view-space `(x, y)`.
fn move_node(node_id, x, y) {
    apply(#{ MoveNode: #{ node_id: node_id, to: #{ x: x, y: y } } });
}

// Highlight `node_id` (or `()` to clear).
fn select_node(node_id) {
    apply(#{ SelectNode: #{ to: node_id } });
}

// Rename `node_id`.
fn rename_node(node_id, name) {
    apply(#{ RenameNode: #{ node_id: node_id, to: name } });
}
