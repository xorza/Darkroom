use crate::func_library_view::FuncLibraryView;
use crate::graph_view::GraphView;
use graph::elements::basic_invoker::BasicInvoker;
use graph::function::FuncLib;
use graph::graph::Graph;
use graph::invoke::{Invoker, UberInvoker};
use graph::worker::Worker;

#[derive(Debug, Default)]
#[allow(dead_code)]
pub(crate) struct Ctx {
    pub(crate) graph_view: GraphView,
    pub(crate) func_library_view: FuncLibraryView,
    pub(crate) graph: Graph,
    pub(crate) invoker: UberInvoker,
    pub(crate) func_lib: FuncLib,
    pub(crate) worker: Option<Worker>,
}

impl Ctx {
    pub(crate) fn new() -> Self {
        let invoker = UberInvoker::new(vec![Box::<BasicInvoker>::default()]);

        let graph = Graph::from_yaml_file("test_resources\\test_graph.yml").unwrap();

        let mut func_lib = invoker.get_func_lib();
        let func_lib2 = FuncLib::from_yaml_file("test_resources\\test_funcs.yml").unwrap();
        func_lib.merge(func_lib2);

        let graph_view = GraphView::from_graph_func_lib(&graph, &func_lib);

        Self {
            graph_view,
            func_library_view: FuncLibraryView::from(&func_lib),
            graph,
            invoker,
            func_lib,
            worker: None,
        }
    }

    pub(crate) fn sync_graph_from_view(&mut self) {
        self.graph = Graph::from(&self.graph_view);
    }
}
