use crate::model::{NodeExecutionIndex, ViewGraph};
use scenarium::prelude::{ExecutionStats, FuncLib};

/// Read-only bundle of frame-level dependencies for the view layer.
///
/// Every field is borrowed immutably: graph mutations go through
/// `GraphUiAction::apply` in `Session::commit_actions`, and the
/// `ArgumentValuesCache` lives on the renderer (`GraphUi`) — not in
/// this context. `exec_info_index` is built once per frame from
/// `execution_stats` so per-node renderers don't re-scan the lists.
#[derive(Debug)]
pub struct GraphContext<'a> {
    pub func_lib: &'a FuncLib,
    pub view_graph: &'a ViewGraph,
    pub execution_stats: Option<&'a ExecutionStats>,
    pub exec_info_index: NodeExecutionIndex<'a>,
    pub autorun: bool,
}
