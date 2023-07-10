use crate::compute::{Compute, InvokeArgs, InvokeContext};
use crate::functions::FunctionId;
use crate::graph::*;
use crate::preprocess::Preprocess;

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

    let nodes = runtime.run(&graph);

    let _yaml = serde_yaml::to_string(&nodes)?;

    Ok(())
}
