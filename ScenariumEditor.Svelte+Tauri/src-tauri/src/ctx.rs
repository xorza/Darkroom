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
        let func_lib = invoker.get_func_lib();

        Self {
            graph_view: GraphView::default(),
            func_library_view: FuncLibraryView::from(&func_lib),

            graph: Graph::default(),
            invoker,
            func_lib,
            worker: None,
        }
    }
}
