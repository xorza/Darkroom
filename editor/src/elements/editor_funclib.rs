use std::sync::Arc;

use common::BoolExt;
use graph::{
    event::EventLambda,
    prelude::{Func, FuncBehavior, FuncId, FuncLambda, FuncLib},
};
use tokio::sync::Notify;

#[derive(Debug)]
pub struct EditorFuncLib {
    func_lib: FuncLib,
}

impl EditorFuncLib {
    pub fn new() -> EditorFuncLib {
        EditorFuncLib {
            func_lib: FuncLib::default(),
        }
    }

    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl From<EditorFuncLib> for FuncLib {
    fn from(value: EditorFuncLib) -> Self {
        value.func_lib
    }
}
