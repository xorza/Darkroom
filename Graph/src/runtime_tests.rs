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

        let nodes = runtime.run(&graph, &invoker);
        assert_eq!(nodes.nodes.iter().all(|_node| _node.executed), true);
        assert_eq!(nodes.nodes.iter().all(|_node| _node.has_arguments), true);
    }

    #[test]
    fn double_run() {
        let graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.run(&graph, &invoker);

        let nodes = runtime.run(&graph, &invoker);
        assert_eq!(nodes.nodes.iter().all(|node| node.has_arguments), true);
        assert_eq!(nodes.node_by_id(1).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(2).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(3).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(4).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn node_behavior_active_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.run(&graph, &invoker);

        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        let nodes = runtime.run(&graph, &invoker);
        assert_eq!(nodes.nodes.iter().all(|_node| _node.has_arguments), true);
        assert_eq!(nodes.node_by_id(1).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(2).unwrap().executed, true);
        assert_eq!(nodes.node_by_id(3).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(4).unwrap().executed, true);
        assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn edge_behavior_once_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.run(&graph, &invoker);

        graph.node_by_id_mut(4).unwrap()
            .inputs.get_mut(1).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Once;

        let nodes = runtime.run(&graph, &invoker);
        assert_eq!(nodes.nodes.iter().all(|_node| _node.has_arguments), true);
        assert_eq!(nodes.node_by_id(1).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(2).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(3).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(4).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn edge_behavior_always_test() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.run(&graph, &invoker);

        graph.node_by_id_mut(3).unwrap()
            .inputs.get_mut(0).unwrap()
            .binding.as_mut().unwrap()
            .behavior = BindingBehavior::Always;

        let nodes = runtime.run(&graph, &invoker);
        assert_eq!(nodes.nodes.iter().all(|_node| _node.has_arguments), true);
        assert_eq!(nodes.node_by_id(1).unwrap().executed, true);
        assert_eq!(nodes.node_by_id(2).unwrap().executed, false);
        assert_eq!(nodes.node_by_id(3).unwrap().executed, true);
        assert_eq!(nodes.node_by_id(4).unwrap().executed, true);
        assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
    }

    #[test]
    fn multiple_runs_with_various_modifications() {
        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut runtime = Runtime::new();
        let invoker = EmptyInvoker {};

        runtime.run(&graph, &invoker);

        {
            let nodes = runtime.run(&graph, &invoker);
            assert_eq!(nodes.node_by_id(1).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(2).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(3).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(4).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
        }
        {
            graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
            let nodes = runtime.run(&graph, &invoker);
            assert_eq!(nodes.node_by_id(1).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(2).unwrap().executed, true);
            assert_eq!(nodes.node_by_id(3).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(4).unwrap().executed, true);
            assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
        }
        {
            graph.node_by_id_mut(4).unwrap()
                .inputs.get_mut(1).unwrap()
                .binding.as_mut().unwrap()
                .behavior = BindingBehavior::Once;
            let nodes = runtime.run(&graph, &invoker);
            assert_eq!(nodes.node_by_id(1).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(2).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(3).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(4).unwrap().executed, false);
            assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
        }
        {
            graph.node_by_id_mut(3).unwrap()
                .inputs.get_mut(1).unwrap()
                .binding.as_mut().unwrap()
                .behavior = BindingBehavior::Always;
            let nodes = runtime.run(&graph, &invoker);
            assert_eq!(nodes.nodes.iter().all(|_node| _node.has_arguments), true);
            assert_eq!(nodes.node_by_id(1).unwrap().executed, true);
            assert_eq!(nodes.node_by_id(2).unwrap().executed, true);
            assert_eq!(nodes.node_by_id(3).unwrap().executed, true);
            assert_eq!(nodes.node_by_id(4).unwrap().executed, true);
            assert_eq!(nodes.node_by_id(5).unwrap().executed, true);
        }
    }


    #[test]
    fn simple_compute_test() {
        static mut RESULT: i64 = 0;
        static mut A: i64 = 2;
        static mut B: i64 = 5;

        let mut invoker = LambdaInvoker::new();
        invoker.add_lambda("print", |_, inputs, _| unsafe {
            RESULT = inputs[0].as_int();
        });
        invoker.add_lambda("val0", |_, _, outputs| {
            outputs[0] = Value::from(unsafe { A });
        });
        invoker.add_lambda("val1", |_, _, outputs| {
            outputs[0] = Value::from(unsafe { B });
        });
        invoker.add_lambda("sum", |_, inputs, outputs| {
            let a: i64 = inputs[0].as_int();
            let b: i64 = inputs[1].as_int();
            outputs[0] = Value::from(a + b);
        });
        invoker.add_lambda("mult", |_, inputs, outputs| {
            let a: i64 = inputs[0].as_int();
            let b: i64 = inputs[1].as_int();
            outputs[0] = Value::from(a * b);
        });

        let mut graph = Graph::from_json_file("./test_resources/test_graph.json");
        let mut compute = Runtime::new();

        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 35);

        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 35);

        unsafe { B = 7; }
        graph.node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;

        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 49);

        graph
            .node_by_id_mut(3).unwrap()
            .inputs.get_mut(0).unwrap()
            .binding.as_mut().unwrap().behavior = BindingBehavior::Always;

        compute.run(&graph, &invoker);
        assert_eq!(unsafe { RESULT }, 63);

        drop(graph);
    }
}
