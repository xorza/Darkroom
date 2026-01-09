use crate::model::ViewGraph;
use graph::prelude::{ExecutionStats, FuncLib};

#[derive(Debug)]
pub struct GraphContext<'a> {
    pub func_lib: &'a FuncLib,
    pub view_graph: &'a mut ViewGraph,
    pub execution_stats: Option<&'a ExecutionStats>,
}

impl<'a> GraphContext<'a> {
    pub fn new(
        func_lib: &'a FuncLib,
        view_graph: &'a mut ViewGraph,
        execution_stats: Option<&'a ExecutionStats>,
    ) -> Self {
        Self {
            func_lib,
            view_graph,
            execution_stats,
        }
    }
}
