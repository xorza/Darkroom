use crate::ctx::context;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncView {
    id: u32,
    title: String,
    description: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncLibraryView {
    pub(crate) funcs: Vec<FuncView>,
}

impl FuncLibraryView {
    pub fn new_test() -> Self {
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
pub(crate) fn get_func_by_id(id: u32) -> FuncView {
    context
        .func_library_view
        .funcs
        .iter()
        .find(|f| f.id == id)
        .cloned()
        .expect("Function not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_func_by_id_returns_func() {
        let f = get_func_by_id(1);
        assert_eq!(f.title, "Multiply");
    }

    #[test]
    fn get_func_by_id_none() {
        let result = std::panic::catch_unwind(|| {
            get_func_by_id(999);
        });
        assert!(result.is_err());
    }
}
