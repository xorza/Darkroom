use crate::model::{ArgumentValuesCache, NodeExecutionIndex, ViewGraph};
use scenarium::prelude::{ExecutionStats, FuncLib};

/// Read-mostly bundle of frame-level dependencies for the view layer.
///
/// `view_graph` is borrowed immutably: every mutation goes through
/// `GraphUiAction::apply` in `Session::commit_actions`, not through this
/// context. `argument_values_cache` is still `&mut` because rendering
/// lazily fills it (texture handles for node previews) — that is
/// UI-owned cache state, not graph domain state. `exec_info_index` is
/// built once per frame from `execution_stats` so per-node renderers
/// don't re-scan the stats lists.
#[derive(Debug)]
pub struct GraphContext<'a> {
    pub func_lib: &'a FuncLib,
    pub view_graph: &'a ViewGraph,
    pub execution_stats: Option<&'a ExecutionStats>,
    pub exec_info_index: NodeExecutionIndex<'a>,
    pub autorun: bool,
    pub argument_values_cache: &'a mut ArgumentValuesCache,
}
