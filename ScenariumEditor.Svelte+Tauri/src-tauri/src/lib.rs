mod ctx;
mod func_library_view;
mod graph_view;

use crate::func_library_view::{get_func_by_id, get_func_library};
use crate::graph_view::{
    add_connection_to_graph_view, add_node_to_graph_view, debug_assert_graph_view, get_graph_view,
    remove_connections_from_graph_view, remove_node_from_graph_view, update_graph, update_node,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_graph_view,
            get_func_library,
            get_func_by_id,
            add_node_to_graph_view,
            add_connection_to_graph_view,
            remove_connections_from_graph_view,
            remove_node_from_graph_view,
            update_node,
            update_graph,
            debug_assert_graph_view
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
