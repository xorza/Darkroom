#[cfg(test)]
mod graph_tests {
    use crate::compute::*;
    use crate::workspace::*;

    #[test]
    fn from_json() {
        let mut compute = Compute::new();
        let mut workspace = Workspace::from_json_file("./test_resources/test_workspace.json");

        compute.run(&mut workspace);

        compute.run(&mut workspace);

        drop(workspace);
    }
}