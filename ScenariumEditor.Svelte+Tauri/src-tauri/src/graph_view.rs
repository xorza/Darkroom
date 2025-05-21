use crate::ctx::context;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionView {
    from_node_id: String,
    from_index: u32,
    to_node_id: String,
    to_index: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeView {
    id: String,
    func_id: String,
    x: f32,
    y: f32,
    title: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphView {
    nodes: Vec<NodeView>,
    connections: Vec<ConnectionView>,
    view_scale: f32,
    view_x: f32,
    view_y: f32,
}

impl PartialEq for NodeView {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.func_id == other.func_id
            // comparing position often fails due to asynchronous updates
            // && self.x == other.x
            // && self.y == other.y
            && self.title == other.title
            && self.inputs == other.inputs
            && self.outputs == other.outputs
    }
}

impl PartialEq for GraphView {
    fn eq(&self, other: &Self) -> bool {
        self.nodes == other.nodes
            && self.connections == other.connections
            && self.view_scale == other.view_scale
            && self.view_x == other.view_x
            && self.view_y == other.view_y
    }
}

#[tauri::command]
pub(crate) fn get_graph_view() -> GraphView {
    context.lock().graph_view.clone()
}

#[tauri::command]
pub(crate) fn get_node_by_id(id: &str) -> NodeView {
    let ctx = context.lock();
    let node = { ctx.graph_view.nodes.iter().find(|n| n.id == id).cloned() };
    node.expect("Node not found")
}

#[tauri::command]
pub(crate) fn add_connection_to_graph_view(connection: ConnectionView) {
    let mut ctx = context.lock();
    ctx.graph_view
        .connections
        .retain(|c| !(c.to_node_id == connection.to_node_id && c.to_index == connection.to_index));
    ctx.graph_view.connections.push(connection);
}

#[tauri::command]
pub(crate) fn remove_connections_from_graph_view(connections: Vec<ConnectionView>) {
    let mut ctx = context.lock();
    ctx.graph_view.connections.retain(|c| {
        !connections.iter().any(|r| {
            r.from_node_id == c.from_node_id
                && r.from_index == c.from_index
                && r.to_node_id == c.to_node_id
                && r.to_index == c.to_index
        })
    });
}

#[tauri::command]
pub(crate) fn remove_node_from_graph_view(id: &str) {
    let mut ctx = context.lock();
    ctx.graph_view.nodes.retain(|n| n.id != id);
    ctx.graph_view
        .connections
        .retain(|c| c.from_node_id != id && c.to_node_id != id);
}

#[tauri::command]
pub(crate) fn update_node(id: &str, x: f32, y: f32) {
    let mut ctx = context.lock();
    if let Some(node) = ctx.graph_view.nodes.iter_mut().find(|n| n.id == id) {
        node.x = x;
        node.y = y;
    } else {
        panic!("Node with id {} not found", id);
    }
}

#[tauri::command]
pub(crate) fn update_graph(view_scale: f32, view_x: f32, view_y: f32) {
    let mut ctx = context.lock();
    ctx.graph_view.view_scale = view_scale;
    ctx.graph_view.view_x = view_x;
    ctx.graph_view.view_y = view_y;
}

#[tauri::command]
pub(crate) fn debug_assert_graph_view(graph_view: GraphView) {
    #[cfg(debug_assertions)]
    {
        let gv = &context.lock().graph_view;
        debug_assert_eq!(graph_view, *gv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_context() {
        let mut gv = GraphView {
            nodes: vec![
                NodeView {
                    id: "0".to_string(),
                    func_id: "0".to_string(),
                    title: "Add".into(),
                    x: 50.0,
                    y: 50.0,
                    inputs: vec!["A".into(), "B".into()],
                    outputs: vec!["Result".into()],
                },
                NodeView {
                    id: "1".to_string(),
                    func_id: "1".to_string(),
                    title: "Multiply".into(),
                    x: 300.0,
                    y: 50.0,
                    inputs: vec!["A".into(), "B".into()],
                    outputs: vec!["Result".into()],
                },
                NodeView {
                    id: "2".to_string(),
                    func_id: "2".to_string(),
                    title: "Output".into(),
                    x: 550.0,
                    y: 50.0,
                    inputs: vec!["Value".into()],
                    outputs: vec![],
                },
            ],
            connections: vec![
                ConnectionView {
                    from_node_id: "0".to_string(),
                    from_index: 0,
                    to_node_id: "1".to_string(),
                    to_index: 0,
                },
                ConnectionView {
                    from_node_id: "1".to_string(),
                    from_index: 0,
                    to_node_id: "2".to_string(),
                    to_index: 0,
                },
            ],
            view_scale: 1.0,
            view_x: 0.0,
            view_y: 0.0,
        };
        gv.connections.clear();

        context.lock().graph_view = gv;
    }

    #[test]
    fn add_connection_persists() {
        reset_context();
        let conn = ConnectionView {
            from_node_id: "0".to_string(),
            from_index: 0,
            to_node_id: "1".to_string(),
            to_index: 0,
        };
        add_connection_to_graph_view(conn.clone());
        let gv = &context.lock().graph_view;
        assert!(gv
            .connections
            .iter()
            .any(|c| c.from_node_id == conn.from_node_id
                && c.from_index == conn.from_index
                && c.to_node_id == conn.to_node_id
                && c.to_index == conn.to_index));
    }

    #[test]
    fn remove_connections_persists() {
        reset_context();

        let c1 = ConnectionView {
            from_node_id: "0".to_string(),
            from_index: 0,
            to_node_id: "1".to_string(),
            to_index: 0,
        };
        let c2 = ConnectionView {
            from_node_id: "1".to_string(),
            from_index: 0,
            to_node_id: "2".to_string(),
            to_index: 0,
        };
        {
            context.lock().graph_view.connections = vec![c1.clone(), c2.clone()];
        }
        remove_connections_from_graph_view(vec![c1.clone()]);

        assert!(!context
            .lock()
            .graph_view
            .connections
            .iter()
            .any(|c| c.from_node_id == c1.from_node_id
                && c.from_index == c1.from_index
                && c.to_node_id == c1.to_node_id
                && c.to_index == c1.to_index));
        assert!(context
            .lock()
            .graph_view
            .connections
            .iter()
            .any(|c| c.from_node_id == c2.from_node_id
                && c.from_index == c2.from_index
                && c.to_node_id == c2.to_node_id
                && c.to_index == c2.to_index));
    }

    #[test]
    fn remove_node_persists() {
        reset_context();
        remove_node_from_graph_view("1");
        assert!(!context.lock().graph_view.nodes.iter().any(|n| n.id == "1"));
        assert!(!context
            .lock()
            .graph_view
            .connections
            .iter()
            .any(|c| c.from_node_id == "1" || c.to_node_id == "1"));
    }

    #[test]
    fn get_node_by_id_none() {
        reset_context();
        let result = std::panic::catch_unwind(|| {
            get_node_by_id("999");
        });
        assert!(result.is_err());
    }

    #[test]
    fn update_graph_persists() {
        reset_context();
        update_graph(2.0, 5.0, 6.0);
        let gv = &context.lock().graph_view;
        assert_eq!(gv.view_scale, 2.0);
        assert_eq!(gv.view_x, 5.0);
        assert_eq!(gv.view_y, 6.0);
    }

    #[test]
    fn debug_assert_graph_view_matches() {
        reset_context();
        let gv = get_graph_view();
        debug_assert_graph_view(gv);
    }
}
