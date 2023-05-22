#[cfg(test)]
mod graph_tests {
    use crate::compute::*;
    use crate::graph::{ConnectionBehavior, NodeBehavior};
    use crate::invoke::LambdaInvoker;
    use crate::workspace::*;

    #[test]
    fn from_json() {
        let mut workspace = Workspace::from_json_file("./test_resources/test_workspace.json");


        static mut RESULT: i32 = 0;
        static mut A: i32 = 2;
        static mut B: i32 = 5;

        let mut invoker = LambdaInvoker::new();
        invoker.add_lambda("val0", |_, outputs| {
            outputs[0] = unsafe { A };
        });
        invoker.add_lambda("val1", |_, outputs| {
            outputs[0] = unsafe { B };
        });
        invoker.add_lambda("sum", |inputs, outputs| {
            outputs[0] = inputs[0] + inputs[1];
        });
        invoker.add_lambda("mult", |inputs, outputs| {
            outputs[0] = inputs[0] * inputs[1];
        });

        invoker.add_lambda("print", |inputs, _| unsafe {
            RESULT = inputs[0];
        });


        let mut compute = Compute::new();
        compute.invoker = Box::new(invoker);

        compute.run(&mut workspace);
        assert_eq!(unsafe { RESULT }, 35);

        compute.run(&mut workspace);
        assert_eq!(unsafe { RESULT }, 35);

        unsafe { B = 7; }
        workspace.graph_mut().node_by_id_mut(2).unwrap().behavior = NodeBehavior::Active;
        compute.run(&mut workspace);
        assert_eq!(unsafe { RESULT }, 49);

        workspace.graph_mut().input_by_id_mut(10).unwrap().connection_behavior = ConnectionBehavior::Always;
        compute.run(&mut workspace);
        assert_eq!(unsafe { RESULT }, 63);

        drop(workspace);
    }
}