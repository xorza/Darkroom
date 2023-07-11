use crate::compute::{Compute, InvokeArgs};
use crate::functions::FunctionId;
use crate::graph::*;
use crate::preprocess::Preprocess;
use crate::runtime_graph::{InvokeContext, RuntimeGraph};

struct EmptyInvoker {}

impl Compute for EmptyInvoker {
    fn invoke(&self,
              _function_id: FunctionId,
              _ctx: &mut InvokeContext,
              _inputs: &InvokeArgs,
              _outputs: &mut InvokeArgs)
        -> anyhow::Result<()> {
        Ok(())
    }
}


#[test]
fn simple_run() -> anyhow::Result<()> {
    let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
    let runtime = Preprocess::default();

    let nodes = runtime.run(&graph, &mut RuntimeGraph::default());

    let _yaml = serde_yaml::to_string(&nodes)?;

    Ok(())
}
