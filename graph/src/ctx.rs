use crate::compute::Compute;
use crate::graph::Graph;
use crate::invoke::UberInvoker;
use crate::runtime_graph::RuntimeGraph;
use crate::worker::Worker;

#[derive(Default, Debug)]
pub struct Context {
    pub(crate) graph: Graph,
    pub(crate) runtime_graph: RuntimeGraph,
    pub(crate) compute: Compute,
    pub(crate) invoker: UberInvoker,

    pub(crate) worker: Option<Worker>,
}
