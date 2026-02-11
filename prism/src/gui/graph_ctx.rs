use crate::model::ArgumentValuesCache;
use crate::model::ViewGraph;
use scenarium::prelude::{ExecutionStats, FuncLib};

#[derive(Debug)]
pub struct GraphContext<'a> {
    pub func_lib: &'a FuncLib,
    pub view_graph: &'a mut ViewGraph,
    pub execution_stats: Option<&'a ExecutionStats>,
    pub autorun: bool,
    pub argument_values_cache: &'a mut ArgumentValuesCache,
}
