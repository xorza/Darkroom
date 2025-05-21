use crate::func_library_view::FuncLibraryView;
use crate::graph_view::GraphView;
use std::sync::Mutex;
use lazy_static::lazy_static;

pub(crate) struct Ctx {
    pub(crate) graph_view: Mutex<GraphView>,
    pub(crate) func_library_view: FuncLibraryView,
}

impl Default for Ctx {
    fn default() -> Self {
        Self {
            graph_view: Mutex::new(GraphView::default()),
            func_library_view: FuncLibraryView::default(),
        }
    }
}

lazy_static! {
    pub(crate) static ref context: Ctx = Ctx::default();
}
