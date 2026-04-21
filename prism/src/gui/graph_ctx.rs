use crate::model::ArgumentValuesCache;
use crate::model::ViewGraph;
use scenarium::prelude::{ExecutionStats, FuncLib};

/// Read-mostly bundle of frame-level dependencies for the view layer.
///
/// `view_graph` is borrowed immutably: every mutation goes through
/// `GraphUiAction::apply` in `AppData::handle_actions`, not through this
/// context. `argument_values_cache` is still `&mut` because rendering
/// lazily fills it (texture handles for node previews) — that is
/// UI-owned cache state, not graph domain state.
#[derive(Debug)]
pub struct GraphContext<'a> {
    pub func_lib: &'a FuncLib,
    pub view_graph: &'a ViewGraph,
    pub execution_stats: Option<&'a ExecutionStats>,
    pub autorun: bool,
    pub argument_values_cache: &'a mut ArgumentValuesCache,
}
