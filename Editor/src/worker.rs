use graph_lib::graph::Graph;

#[derive(Debug, Default)]
pub(crate) struct Worker {}

impl Worker {
    pub(crate) fn run_once(
        &mut self,
        _graph: &Graph,
    ) {}
}