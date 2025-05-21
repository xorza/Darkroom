use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NodeLibraryItem {
    id: u32,
    title: String,
    description: String,
}


#[tauri::command]
pub(crate) fn get_node_library() -> Vec<NodeLibraryItem> {
    vec![
        NodeLibraryItem {
            id: 0,
            title: "Add".into(),
            description: "Adds two numbers together.".into(),
        },
        NodeLibraryItem {
            id: 1,
            title: "Multiply".into(),
            description: "Multiplies two numbers together.".into(),
        },
        NodeLibraryItem {
            id: 2,
            title: "Output".into(),
            description: "Outputs a value.".into(),
        },
    ]
}
