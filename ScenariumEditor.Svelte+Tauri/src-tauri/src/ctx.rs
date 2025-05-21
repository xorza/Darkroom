use crate::func_library_view::FuncLibraryView;
use crate::graph_view::GraphView;
use graph::elements::basic_invoker::BasicInvoker;
use graph::function::FuncLib;
use graph::graph::Graph;
use graph::invoke::{Invoker, UberInvoker};
use graph::worker::Worker;
use lazy_static::lazy_static;
use parking_lot::Mutex;

pub(crate) struct Ctx {
    pub(crate) graph_view: GraphView,
    pub(crate) func_library_view: FuncLibraryView,
    pub(crate) graph: Graph,
    pub(crate) invoker: UberInvoker,
    pub(crate) func_lib: FuncLib,
    pub(crate) worker: Option<Worker>,
}

impl Default for Ctx {
    fn default() -> Self {
        let invoker = UberInvoker::new(vec![Box::<BasicInvoker>::default()]);
        let func_lib = invoker.get_func_lib();

        let result = Self {
            graph_view: GraphView::default(),
            func_library_view: FuncLibraryView::from(&func_lib),

            graph: Graph::default(),
            invoker,
            func_lib,
            worker: None,
        };

        result
    }
}

lazy_static! {
    pub(crate) static ref context: Mutex<Ctx> = Mutex::new(Ctx::default());
}
