use crate::ctx::context;
use graph::function::FuncLib;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncView {
    id: String,
    title: String,
    description: String,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FuncLibraryView {
    pub(crate) funcs: Vec<FuncView>,
}

impl From<&FuncLib> for FuncLibraryView {
    fn from(value: &FuncLib) -> Self {
        let mut funcs = Vec::new();
        for func in value.iter() {
            funcs.push(FuncView {
                id: func.id.to_string(),
                title: func.name.clone(),
                description: func
                    .description
                    .as_ref()
                    .unwrap_or(&("No description provided".to_string()))
                    .to_string(),
            });
        }
        Self { funcs }
    }
}

#[tauri::command]
pub(crate) fn get_func_library() -> FuncLibraryView {
    context.lock().func_library_view.clone()
}

#[tauri::command]
pub(crate) fn get_func_by_id(id: &str) -> FuncView {
    context
        .lock()
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

    fn reset_context() {
        let func_library_view = FuncLibraryView {
            funcs: vec![
                FuncView {
                    id: "0".to_string(),
                    title: "Add".into(),
                    description: "Adds two numbers together.".into(),
                },
                FuncView {
                    id: "1".to_string(),
                    title: "Multiply".into(),
                    description: "Multiplies two numbers together.".into(),
                },
                FuncView {
                    id: "2".to_string(),
                    title: "Output".into(),
                    description: "Outputs a value.".into(),
                },
            ],
        };
        context.lock().func_library_view = func_library_view;
    }

    #[test]
    fn get_func_by_id_returns_func() {
        reset_context();

        let f = get_func_by_id("1");
        assert_eq!(f.title, "Multiply");
    }

    #[test]
    fn get_func_by_id_none() {
        reset_context();

        let result = std::panic::catch_unwind(|| {
            get_func_by_id("999");
        });
        assert!(result.is_err());
    }
}
