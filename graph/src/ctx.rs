use crate::compute::Compute;
use crate::function::FuncLib;
use crate::graph::Graph;
use crate::invoke::UberInvoker;
use crate::runtime_graph::RuntimeGraph;
use crate::worker::Worker;

#[derive(Default, Debug)]
pub struct Context {
    pub graph: Graph,
    pub runtime_graph: RuntimeGraph,
    pub compute: Compute,
    pub invoker: UberInvoker,
    pub func_lib: FuncLib,

    pub worker: Option<Worker>,
}
