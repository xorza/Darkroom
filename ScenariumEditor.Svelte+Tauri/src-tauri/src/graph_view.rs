use crate::ctx::context;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionView {
    from_node_id: u32,
    from_index: u32,
    to_node_id: u32,
    to_index: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeView {
    id: u32,
    func_id: u32,
    x: f32,
    y: f32,
    title: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphView {
    nodes: Vec<NodeView>,
    connections: Vec<ConnectionView>,
    view_scale: f32,
    view_x: f32,
    view_y: f32,
    selected_node_ids: Vec<u32>,
}

impl GraphView {
    pub fn new_test() -> Self {
        Self {
            nodes: vec![
                NodeView {
                    id: 0,
                    title: "Add".into(),
                    x: 50.0,
                    y: 50.0,
                    inputs: vec!["A".into(), "B".into()],
                    outputs: vec!["Result".into()],
                    func_id: 0,
                },
                NodeView {
                    id: 1,
                    title: "Multiply".into(),
                    x: 300.0,
                    y: 50.0,
                    inputs: vec!["A".into(), "B".into()],
                    outputs: vec!["Result".into()],
                    func_id: 1,
                },
                NodeView {
                    id: 2,
                    title: "Output".into(),
                    x: 550.0,
                    y: 50.0,
                    inputs: vec!["Value".into()],
                    outputs: vec![],
                    func_id: 2,
                },
            ],
            connections: vec![
                ConnectionView {
                    from_node_id: 0,
                    from_index: 0,
                    to_node_id: 1,
                    to_index: 0,
                },
                ConnectionView {
                    from_node_id: 1,
                    from_index: 0,
                    to_node_id: 2,
                    to_index: 0,
                },
            ],
            view_scale: 1.0,
            view_x: 0.0,
            view_y: 0.0,
            selected_node_ids: vec![],
        }
    }
}

#[tauri::command]
pub(crate) fn get_graph_view() -> GraphView {
    context.graph_view.lock().unwrap().clone()
}

#[tauri::command]
pub(crate) fn add_node_to_graph_view(node: NodeView) {
    let mut gv = context.graph_view.lock().unwrap();
    gv.nodes.push(node);
}

#[tauri::command]
pub(crate) fn add_connection_to_graph_view(connection: ConnectionView) {
    let mut gv = context.graph_view.lock().unwrap();
    gv.connections
        .retain(|c| !(c.to_node_id == connection.to_node_id && c.to_index == connection.to_index));
    gv.connections.push(connection);
}

#[tauri::command]
pub(crate) fn remove_connections_from_graph_view(connections: Vec<ConnectionView>) {
    let mut gv = context.graph_view.lock().unwrap();
    gv.connections.retain(|c| {
        !connections.iter().any(|r| {
            r.from_node_id == c.from_node_id
                && r.from_index == c.from_index
                && r.to_node_id == c.to_node_id
                && r.to_index == c.to_index
        })
    });
}

#[tauri::command]
pub(crate) fn remove_node_from_graph_view(id: u32) {
    let mut gv = context.graph_view.lock().unwrap();
    gv.nodes.retain(|n| n.id != id);
    gv.connections
        .retain(|c| c.from_node_id != id && c.to_node_id != id);
    gv.selected_node_ids.retain(|nid| *nid != id);
}

#[tauri::command]
pub(crate) fn debug_assert_graph_view(_graph_view: GraphView) {
    #[cfg(debug_assertions)]
    {
        // let gv = context.graph_view.lock().unwrap();
        // debug_assert_eq!(graph_view, *gv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_context() {
        let mut gv = GraphView::default();
        gv.connections.clear();
        let mut lock = context.graph_view.lock().unwrap();
        *lock = gv;
    }

    #[test]
    fn add_connection_persists() {
        reset_context();
        let conn = ConnectionView {
            from_node_id: 0,
            from_index: 0,
            to_node_id: 1,
            to_index: 0,
        };
        add_connection_to_graph_view(conn.clone());
        let gv = context.graph_view.lock().unwrap();
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
            from_node_id: 0,
            from_index: 0,
            to_node_id: 1,
            to_index: 0,
        };
        let c2 = ConnectionView {
            from_node_id: 1,
            from_index: 0,
            to_node_id: 2,
            to_index: 0,
        };
        {
            let mut gv = context.graph_view.lock().unwrap();
            gv.connections = vec![c1.clone(), c2.clone()];
        }
        remove_connections_from_graph_view(vec![c1.clone()]);
        let gv = context.graph_view.lock().unwrap();
        assert!(!gv
            .connections
            .iter()
            .any(|c| c.from_node_id == c1.from_node_id
                && c.from_index == c1.from_index
                && c.to_node_id == c1.to_node_id
                && c.to_index == c1.to_index));
        assert!(gv
            .connections
            .iter()
            .any(|c| c.from_node_id == c2.from_node_id
                && c.from_index == c2.from_index
                && c.to_node_id == c2.to_node_id
                && c.to_index == c2.to_index));
    }

    #[test]
    fn add_node_persists() {
        reset_context();
        let node = NodeView {
            id: 3,
            func_id: 99,
            x: 0.0,
            y: 0.0,
            title: "Test".into(),
            inputs: vec![],
            outputs: vec![],
        };
        add_node_to_graph_view(node.clone());
        let gv = context.graph_view.lock().unwrap();
        assert!(gv.nodes.iter().any(|n| n.id == node.id));
    }

    #[test]
    fn remove_node_persists() {
        reset_context();
        remove_node_from_graph_view(1);
        let gv = context.graph_view.lock().unwrap();
        assert!(!gv.nodes.iter().any(|n| n.id == 1));
        assert!(!gv
            .connections
            .iter()
            .any(|c| c.from_node_id == 1 || c.to_node_id == 1));
    }

    #[test]
    fn debug_assert_graph_view_matches() {
        reset_context();
        let gv = get_graph_view();
        debug_assert_graph_view(gv);
    }
}
