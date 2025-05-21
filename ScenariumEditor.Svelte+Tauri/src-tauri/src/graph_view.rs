use serde::Serialize;


#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
enum PinType {
    Input,
    Output,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pin {
    node_id: u32,
    #[serde(rename = "type")]
    pin_type: PinType,
    index: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionView {
    from_node_id: u32,
    from_index: u32,
    to_node_id: u32,
    to_index: u32,
}

#[derive(Serialize, Clone)]
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraphView {
    nodes: Vec<NodeView>,
    connections: Vec<ConnectionView>,
    view_scale: f32,
    view_x: f32,
    view_y: f32,
    selected_node_ids: Vec<u32>,
}

impl Default for GraphView {
    fn default() -> Self {
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
    GraphView::default()
}