#![allow(dead_code)]

mod ctx;
mod func_library_view;
mod graph_view;

use crate::ctx::Ctx;
use crate::func_library_view::{get_func_by_id, get_func_library};
use crate::graph_view::{
    add_connection_to_graph_view, create_node, debug_assert_graph_view, get_graph_view,
    get_node_by_id, new_graph, remove_connections_from_graph_view, remove_node_from_graph_view,
    update_graph, update_node,
};

#[derive(Debug, Default)]
pub(crate) struct AppState {
    ctx: Ctx,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState { ctx: Ctx::new() };

    tauri::Builder::default()
        .manage(parking_lot::Mutex::new(app_state))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_graph_view,
            get_func_library,
            get_func_by_id,
            get_node_by_id,
            create_node,
            add_connection_to_graph_view,
            remove_connections_from_graph_view,
            remove_node_from_graph_view,
            update_node,
            update_graph,
            new_graph,
            debug_assert_graph_view,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
