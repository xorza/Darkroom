#[cfg(test)]
mod graph_tests {
    use crate::compute::*;
    use crate::graph::{BindingBehavior, Graph, NodeBehavior};
    use crate::invoke::LambdaInvoker;

    #[test]
    fn from_json() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");

        static mut RESULT: i32 = 0;
        static mut A: i32 = 2;
        static mut B: i32 = 5;

        let mut invoker = LambdaInvoker::new();
        invoker.add_lambda("val0", |_, _, outputs| {
            outputs[0] = unsafe { A };
        });
        invoker.add_lambda("val1", |_, _, outputs| {
            outputs[0] = unsafe { B };
        });
        invoker.add_lambda("sum", |_, inputs, outputs| {
            outputs[0] = inputs[0] + inputs[1];
        });
        invoker.add_lambda("mult", |_, inputs, outputs| {
            outputs[0] = inputs[0] * inputs[1];
        });

        invoker.add_lambda("print", |_, inputs, _| unsafe {
            RESULT = inputs[0];
        });


        let mut compute = Compute::new();
        compute.invoker = Box::new(invoker);

        compute.run(&graph);
        assert_eq!(unsafe { RESULT }, 35);

        compute.run(&graph);
        assert_eq!(unsafe { RESULT }, 35);

        unsafe { B = 7; }
        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        compute.run(&graph);
        assert_eq!(unsafe { RESULT }, 49);

        graph
            .node_by_id_mut(3).unwrap()
            .inputs.get_mut(0).unwrap()
            .binding.as_mut().unwrap().behavior = BindingBehavior::Always;

        compute.run(&graph);
        assert_eq!(unsafe { RESULT }, 63);

        drop(graph);
    }
}