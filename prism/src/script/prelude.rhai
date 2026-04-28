// Built-in Rhai helpers loaded into every prism scripting engine.
//
// These wrap the generic `apply` / `apply_all` mutation primitives
// (and a few narrowly-scoped Rust helpers like `make_add_node`) into
// ergonomic constructors. Adding a new helper here is one Rhai
// function — no Rust changes needed unless the action requires data
// only the host has (FuncLib lookups, graph walks).
//
// Loaded as a global module by `script::build_engine`, so every
// function defined here is callable from any user script.

// Spawn a new node bound to `func_id` at view-space position `(x, y)`.
// Returns the new node's id (UUID string) so the caller can chain
// follow-up actions (connections, selection, …).
fn create_node(func_id, x, y) {
    let action = make_add_node(func_id, x, y);
    apply(action);
    action.AddNode.view_node.id
}

// Wire `from_node`'s output port `from_port` into `to_node`'s input
// port `to_port`. Pure-script helper: no Rust counterpart needed.
fn connect(from_node, from_port, to_node, to_port) {
    apply(#{ ChangeInput: #{
        node_id: to_node,
        input_idx: to_port,
        before: "None",
        after: #{ Bind: #{ target_id: from_node, port_idx: from_port } },
    }});
}

// Drop the binding on `to_node`'s input port `to_port`.
fn disconnect(to_node, to_port, prev_from_node, prev_from_port) {
    apply(#{ ChangeInput: #{
        node_id: to_node,
        input_idx: to_port,
        before: #{ Bind: #{ target_id: prev_from_node, port_idx: prev_from_port } },
        after: "None",
    }});
}

// Move `node_id` from `(bx, by)` to `(ax, ay)`. Caller supplies the
// previous position so undo round-trips cleanly.
fn move_node(node_id, bx, by, ax, ay) {
    apply(#{ MoveNode: #{
        node_id: node_id,
        before: #{ x: bx, y: by },
        after:  #{ x: ax, y: ay },
    }});
}

// Highlight `node_id` (or `()` to clear).
fn select_node(node_id) {
    apply(#{ SelectNode: #{ before: (), after: node_id } });
}

// Rename `node_id` from `before_name` to `after_name`.
fn rename_node(node_id, before_name, after_name) {
    apply(#{ RenameNode: #{
        node_id: node_id,
        before: before_name,
        after: after_name,
    }});
}
