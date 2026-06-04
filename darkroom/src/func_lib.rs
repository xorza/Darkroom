//! The built-in runtime function library, shared by every frontend.

use lens::ImageFuncLib;
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::prelude::FuncLib;

/// Assemble the built-in runtime function library. Builtins carry no
/// subgraph defs, so `func_lib.subgraphs` *is* the shared subgraph library —
/// loaded from the library file at startup, grown by "promote".
pub(crate) fn builtin_func_lib() -> FuncLib {
    let mut func_lib = FuncLib::default();
    func_lib.merge(BasicFuncLib::default());
    func_lib.merge(WorkerEventsFuncLib::default());
    func_lib.merge(ImageFuncLib::default());
    func_lib
}
