use crate::AppState;
use glam::Vec2;
use graph::function::FuncId;
use graph::graph::Node;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tauri::State;

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
    view_pos: Vec2,
    title: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphView {
    nodes: Vec<NodeView>,
    connections: Vec<ConnectionView>,
    view_scale: f32,
    view_pos: Vec2,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            nodes: vec![],
            connections: vec![],
            view_scale: 1.0,
            view_pos: Vec2::ZERO,
        }
    }
}

impl PartialEq for NodeView {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.func_id == other.func_id
            // comparing position often fails due to asynchronous updates
            // && self.view_pos == other.view_pos
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
            && self.view_pos == other.view_pos
    }
}

#[tauri::command]
pub(crate) fn get_graph_view(state: State<'_, parking_lot::Mutex<AppState>>) -> GraphView {
    state.lock().ctx.graph_view.clone()
}

#[tauri::command]
pub(crate) fn get_node_by_id(state: State<'_, parking_lot::Mutex<AppState>>, id: &str) -> NodeView {
    let ctx = &state.lock().ctx;

    let node = { ctx.graph_view.nodes.iter().find(|n| n.id == id).cloned() };
    node.expect("Node not found")
}

#[tauri::command]
pub(crate) fn add_connection_to_graph_view(
    state: State<'_, parking_lot::Mutex<AppState>>,
    connection: ConnectionView,
) {
    let mut app_state = state.lock();
    let ctx = &mut app_state.ctx;

    ctx.graph_view
        .connections
        .retain(|c| !(c.to_node_id == connection.to_node_id && c.to_index == connection.to_index));
    ctx.graph_view.connections.push(connection);
}

#[tauri::command]
pub(crate) fn remove_connections_from_graph_view(
    state: State<'_, parking_lot::Mutex<AppState>>,
    connections: Vec<ConnectionView>,
) {
    let mut app_state = state.lock();
    let ctx = &mut app_state.ctx;

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
pub(crate) fn remove_node_from_graph_view(
    state: State<'_, parking_lot::Mutex<AppState>>,
    id: &str,
) {
    let mut app_state = state.lock();
    let ctx = &mut app_state.ctx;

    ctx.graph_view.nodes.retain(|n| n.id != id);
    ctx.graph_view
        .connections
        .retain(|c| c.from_node_id != id && c.to_node_id != id);
}

#[tauri::command]
pub(crate) fn create_node(
    state: State<'_, parking_lot::Mutex<AppState>>,
    func_id: &str,
) -> NodeView {
    let func_id = FuncId::from_str(func_id).expect("Invalid func id");

    let mut app_state = state.lock();
    let ctx = &mut app_state.ctx;
    let func = ctx
        .func_lib
        .func_by_id(func_id)
        .cloned()
        .expect("Function not found");

    let node = Node::from_function(&func);
    ctx.graph.add_node(node.clone());

    let node_view = NodeView {
        id: node.id.to_string(),
        func_id: node.func_id.to_string(),
        view_pos: Vec2::ZERO,
        title: func.name.clone(),
        inputs: func.inputs.iter().map(|i| i.name.clone()).collect(),
        outputs: func.outputs.iter().map(|o| o.name.clone()).collect(),
    };

    ctx.graph_view.nodes.push(node_view.clone());

    node_view
}

#[tauri::command]
pub(crate) fn update_node(
    state: State<'_, parking_lot::Mutex<AppState>>,
    id: &str,
    view_pos: Vec2,
) {
    let mut app_state = state.lock();
    let ctx = &mut app_state.ctx;
    if let Some(node) = ctx.graph_view.nodes.iter_mut().find(|n| n.id == id) {
        node.view_pos = view_pos;
    } else {
        panic!("Node with id {} not found", id);
    }
}

#[tauri::command]
pub(crate) fn update_graph(
    state: State<'_, parking_lot::Mutex<AppState>>,
    view_scale: f32,
    view_pos: Vec2,
) {
    let mut app_state = state.lock();
    let ctx = &mut app_state.ctx;
    ctx.graph_view.view_scale = view_scale;
    ctx.graph_view.view_pos = view_pos;
}

#[tauri::command]
pub(crate) fn debug_assert_graph_view(
    state: State<'_, parking_lot::Mutex<AppState>>,
    graph_view: GraphView,
) {
    #[cfg(debug_assertions)]
    {
        let gv = &state.lock().ctx.graph_view;
        debug_assert_eq!(graph_view, *gv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::function::{Func, FuncBehavior, FuncId, FuncLib};
    use graph::graph::Graph;
    use parking_lot::Mutex as ParkingMutex;
    use std::panic::AssertUnwindSafe;
    use std::str::FromStr;
    use tauri::test::MockRuntime;
    use tauri::{App, Manager, State};

    fn create_app_state() -> App<MockRuntime> {
        let mut app_state = AppState::default();
        let mut gv = GraphView {
            nodes: vec![
                NodeView {
                    id: "0".to_string(),
                    func_id: "0".to_string(),
                    title: "Add".into(),
                    view_pos: Vec2::new(50.0, 50.0),
                    inputs: vec!["A".into(), "B".into()],
                    outputs: vec!["Result".into()],
                },
                NodeView {
                    id: "1".to_string(),
                    func_id: "1".to_string(),
                    title: "Multiply".into(),
                    view_pos: Vec2::new(300.0, 50.0),
                    inputs: vec!["A".into(), "B".into()],
                    outputs: vec!["Result".into()],
                },
                NodeView {
                    id: "2".to_string(),
                    func_id: "2".to_string(),
                    title: "Output".into(),
                    view_pos: Vec2::new(550.0, 50.0),
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
            view_pos: Vec2::ZERO,
        };
        gv.connections.clear();
        app_state.ctx.graph_view = gv;

        // minimal function library for node creation
        let func_id = FuncId::from_str("00000000-0000-0000-0000-000000000001").unwrap();
        let func = Func {
            id: func_id,
            name: "Test".into(),
            category: "".into(),
            description: None,
            behavior: FuncBehavior::Active,
            is_output: false,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
        };
        let mut lib = FuncLib::default();
        lib.add(func);
        app_state.ctx.func_lib = lib;
        app_state.ctx.graph = Graph::default();

        let app = tauri::test::mock_app();
        app.manage(ParkingMutex::new(app_state));

        app
    }

    #[test]
    fn add_connection_persists() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();

        let conn = ConnectionView {
            from_node_id: "0".to_string(),
            from_index: 0,
            to_node_id: "1".to_string(),
            to_index: 0,
        };
        add_connection_to_graph_view(state.clone(), conn.clone());
        let gv = &state.lock().ctx.graph_view;
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
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();

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
            state.lock().ctx.graph_view.connections = vec![c1.clone(), c2.clone()];
        }
        remove_connections_from_graph_view(state.clone(), vec![c1.clone()]);

        let gv = &state.lock().ctx.graph_view;
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
    fn remove_node_persists() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();
        remove_node_from_graph_view(state.clone(), "1");
        let gv = &state.lock().ctx.graph_view;
        assert!(!gv.nodes.iter().any(|n| n.id == "1"));
        assert!(!gv
            .connections
            .iter()
            .any(|c| c.from_node_id == "1" || c.to_node_id == "1"));
    }

    #[test]
    fn create_node_persists() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();

        let node = create_node(state.clone(), "00000000-0000-0000-0000-000000000001");
        let app_state = state.lock();
        assert!(app_state
            .ctx
            .graph_view
            .nodes
            .iter()
            .any(|n| n.id == node.id));
        assert!(app_state
            .ctx
            .graph
            .nodes()
            .iter()
            .any(|n| n.id.to_string() == node.id));
        assert_eq!(node.func_id, "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn get_node_by_id_none() {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let app = create_app_state();
            let state: State<'_, ParkingMutex<AppState>> = app.state();

            get_node_by_id(state, "999");
        }));
        assert!(result.is_err());
    }

    #[test]
    fn update_graph_persists() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();
        update_graph(state.clone(), 2.0, Vec2::new(5.0, 6.0));
        let gv = &state.lock().ctx.graph_view;
        assert_eq!(gv.view_scale, 2.0);
        assert_eq!(gv.view_pos, Vec2::new(5.0, 6.0));
    }

    #[test]
    fn debug_assert_graph_view_matches() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();
        let gv = get_graph_view(state.clone());
        debug_assert_graph_view(state, gv);
    }
}
