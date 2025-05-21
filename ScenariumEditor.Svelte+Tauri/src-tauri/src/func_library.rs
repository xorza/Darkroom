use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncLibraryItem {
    id: u32,
    title: String,
    description: String,
}


#[tauri::command]
pub(crate) fn get_func_library() -> Vec<FuncLibraryItem> {
    vec![
        FuncLibraryItem {
            id: 0,
            title: "Add".into(),
            description: "Adds two numbers together.".into(),
        },
        FuncLibraryItem {
            id: 1,
            title: "Multiply".into(),
            description: "Multiplies two numbers together.".into(),
        },
        FuncLibraryItem {
            id: 2,
            title: "Output".into(),
            description: "Outputs a value.".into(),
        },
    ]
}
