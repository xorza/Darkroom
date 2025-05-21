mod graph_view;
mod node_library;

use crate::graph_view::get_graph_view;
use crate::node_library::get_node_library;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_graph_view, get_node_library
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
