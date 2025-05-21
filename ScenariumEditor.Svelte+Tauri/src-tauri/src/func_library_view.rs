use crate::AppState;
use graph::function::FuncLib;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::State;

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
pub(crate) fn get_func_library(state: State<'_, Mutex<AppState>>) -> FuncLibraryView {
    state.lock().ctx.func_library_view.clone()
}

#[tauri::command]
pub(crate) fn get_func_by_id(state: State<'_, Mutex<AppState>>, id: &str) -> FuncView {
    state
        .lock()
        .ctx
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
    use parking_lot::Mutex as ParkingMutex;
    use std::panic::AssertUnwindSafe;
    use tauri::test::MockRuntime;
    use tauri::{App, Manager, State};

    fn create_app_state() -> App<MockRuntime> {
        let mut app_state = AppState::default();
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
        app_state.ctx.func_library_view = func_library_view;

        let app = tauri::test::mock_app();
        app.manage(ParkingMutex::new(app_state));
        app
    }

    #[test]
    fn get_func_by_id_returns_func() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();

        let f = get_func_by_id(state, "1");
        assert_eq!(f.title, "Multiply");
    }

    #[test]
    fn get_func_by_id_none() {
        let app = create_app_state();
        let state: State<'_, ParkingMutex<AppState>> = app.state();

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            get_func_by_id(state, "999");
        }));
        assert!(result.is_err());
    }
}
