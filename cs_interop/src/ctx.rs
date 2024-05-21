use graph::graph::Graph;
use graph::invoke_context::LambdaInvoker;
use graph::runtime_graph::RuntimeGraph;

#[derive(Debug, Default)]
pub(crate) struct Context {
    pub(crate) invoker: LambdaInvoker,
    pub(crate) runtime_graph: RuntimeGraph,
    pub(crate) graph: Graph,
}
