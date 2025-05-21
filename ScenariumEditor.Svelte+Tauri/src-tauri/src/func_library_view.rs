use serde::{Deserialize, Serialize};
use crate::ctx::context;

#[derive(Serialize,Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncView {
    id: u32,
    title: String,
    description: String,
}

#[derive(Serialize,Deserialize, Clone)]
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

#[tauri::command]
pub(crate) fn get_func_by_id(id: u32) -> Option<FuncView> {
    context
        .func_library_view
        .funcs
        .iter()
        .find(|f| f.id == id)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_func_by_id_returns_func() {
        let f = get_func_by_id(1).unwrap();
        assert_eq!(f.title, "Multiply");
    }

    #[test]
    fn get_func_by_id_none() {
        assert!(get_func_by_id(999).is_none());
    }
}
