use serde::Serialize;
use crate::ctx::context;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncView {
    id: u32,
    title: String,
    description: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate)  struct FuncLibraryView {
    pub(crate) funcs: Vec<FuncView>,
}

impl Default for FuncLibraryView {
    fn default() -> Self {
        Self {
            funcs: vec![
                FuncView {
                    id: 0,
                    title: "Add".into(),
                    description: "Adds two numbers together.".into(),
                },
                FuncView {
                    id: 1,
                    title: "Multiply".into(),
                    description: "Multiplies two numbers together.".into(),
                },
                FuncView {
                    id: 2,
                    title: "Output".into(),
                    description: "Outputs a value.".into(),
                },
            ],
        }
    }
}


#[tauri::command]
pub(crate) fn get_func_library() -> &'static FuncLibraryView {
    &context.func_library_view
}
