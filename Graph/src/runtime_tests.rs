#[cfg(test)]
mod runtime_tests {
    use crate::graph::*;
    use crate::invoke::{Args, Invoker, LambdaInvoker, Value};
    use crate::runtime::Runtime;

    struct EmptyInvoker {}

    impl Invoker for EmptyInvoker {
        fn call(&self, _: &str, _: u32, _: &Args, _: &mut Args) {}
    }

    #[test]
    fn simple_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.prepare(&graph);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.should_execute), true);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.executed), false);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), false);

        runtime.run(&graph, &invoker);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.executed), true);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
    }

    #[test]
    fn double_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);

        runtime.prepare(&graph);
        assert_eq!(runtime.nodes().iter().all(|node| node.has_outputs), true);
        assert_eq!(runtime.nodes().iter().all(|node| node.executed), false);

        runtime.run(&graph, &invoker);
        assert_eq!(runtime.nodes().iter().all(|node| node.has_outputs), true);
        assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(2).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(3).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(4).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn node_behavior_active_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);

        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(2).unwrap().executed, true);
        assert_eq!(runtime.node_by_id(3).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(4).unwrap().executed, true);
        assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn edge_behavior_once_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);

        graph.node_by_id_mut(4).unwrap()
            .inputs.get_mut(1).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Once;

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(2).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(3).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(4).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn edge_behavior_always_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);

        graph.node_by_id_mut(3).unwrap()
            .inputs.get_mut(0).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Always;

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);
        assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
        assert_eq!(runtime.node_by_id(1).unwrap().executed, true);
        assert_eq!(runtime.node_by_id(2).unwrap().executed, false);
        assert_eq!(runtime.node_by_id(3).unwrap().executed, true);
        assert_eq!(runtime.node_by_id(4).unwrap().executed, true);
        assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn multiple_runs_with_various_modifications() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.prepare(&graph);
        runtime.run(&graph, &invoker);

        {
            runtime.prepare(&graph);
            runtime.run(&graph, &invoker);
            assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(2).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(3).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(4).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
        }
        {
            graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
            runtime.prepare(&graph);
            runtime.run(&graph, &invoker);
            assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(2).unwrap().executed, true);
            assert_eq!(runtime.node_by_id(3).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(4).unwrap().executed, true);
            assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
        }
        {
            // graph.input_by_id_mut(13).unwrap().binding.as_mut().unwrap().behavior = BindingBehavior::Once;
            graph.node_by_id_mut(4).unwrap()
                .inputs.get_mut(1).unwrap()
                .binding.as_mut().unwrap()
                .behavior = BindingBehavior::Once;
            runtime.prepare(&graph);
            runtime.run(&graph, &invoker);
            assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(2).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(3).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(4).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
        }
        {
            // graph.input_by_id_mut(11).unwrap().binding.as_mut().unwrap().behavior = BindingBehavior::Always;
            graph.node_by_id_mut(3).unwrap()
                .inputs.get_mut(1).unwrap()
                .binding.as_mut().unwrap()
                .behavior = BindingBehavior::Always;
            runtime.prepare(&graph);
            runtime.run(&graph, &invoker);
            assert_eq!(runtime.nodes().iter().all(|_node| _node.has_outputs), true);
            assert_eq!(runtime.node_by_id(1).unwrap().executed, false);
            assert_eq!(runtime.node_by_id(2).unwrap().executed, true);
            assert_eq!(runtime.node_by_id(3).unwrap().executed, true);
            assert_eq!(runtime.node_by_id(4).unwrap().executed, true);
            assert_eq!(runtime.node_by_id(5).unwrap().executed, true);
        }
    }


    #[test]
    fn simple_compute_test() {
        static mut RESULT: i64 = 0;
        static mut A: i64 = 2;
        static mut B: i64 = 5;

        let mut invoker = LambdaInvoker::new();
        invoker.add_lambda("print", |_, inputs, _| unsafe {
            RESULT = inputs[0].to_int();
        });
        invoker.add_lambda("val0", |_, _, outputs| {
            outputs[0] = Value::from_int(unsafe { A });
        });
        invoker.add_lambda("val1", |_, _, outputs| {
            outputs[0] = Value::from_int(unsafe { B });
        });
        invoker.add_lambda("sum", |_, inputs, outputs| {
            outputs[0] = Value::from_int(inputs[0].to_int() + inputs[1].to_int());
        });
        invoker.add_lambda("mult", |_, inputs, outputs| {
            outputs[0] = Value::from_int(inputs[0].to_int() * inputs[1].to_int());
        });


        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut compute = Runtime::new();

        compute.prepare(&graph);
        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 35);

        compute.prepare(&graph);
        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 35);

        unsafe { B = 7; }
        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        compute.prepare(&graph);
        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 49);

        graph
            .node_by_id_mut(3).unwrap()
            .inputs.get_mut(0).unwrap()
            .binding.as_mut().unwrap().behavior = BindingBehavior::Always;
        compute.prepare(&graph);
        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 63);

        drop(graph);
    }
}
