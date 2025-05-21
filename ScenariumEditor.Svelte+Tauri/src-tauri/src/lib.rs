mod graph_view;
mod func_library_view;
mod ctx;

use crate::graph_view::get_graph_view;
use crate::func_library_view::get_func_library;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_graph_view, get_func_library
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
