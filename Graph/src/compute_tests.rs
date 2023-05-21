#[cfg(test)]
mod graph_tests {
    use std::hint::black_box;
    use crate::compute::Compute;
    use crate::graph::*;
    use crate::workspace::Workspace;

    #[test]
    fn from_json() {
        let mut compute = Compute::new();
        let mut workspace = Workspace::from_json_file("./test_resources/test_workspace.json");

        compute.run(&mut workspace);

        black_box(workspace);
    }
}