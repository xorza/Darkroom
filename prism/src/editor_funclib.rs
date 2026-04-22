use scenarium::prelude::FuncLib;

#[derive(Debug, Default)]
pub struct EditorFuncLib {
    func_lib: FuncLib,
}

impl EditorFuncLib {
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
