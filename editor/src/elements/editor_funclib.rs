use std::time::Instant;

use common::BoolExt;
use graph::prelude::{Func, FuncBehavior, FuncId, FuncLambda, FuncLib};

#[derive(Debug)]
pub struct EditorFuncLib {
    func_lib: FuncLib,
}

impl EditorFuncLib {
    pub const RUN_FUNC_ID: FuncId = FuncId::from_u128(0xe871ddf47a534ae59728927a88649673);

    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl Default for EditorFuncLib {
    fn default() -> EditorFuncLib {
        let mut invoker = FuncLib::default();

        invoker.add(Func {
            id: Self::RUN_FUNC_ID,
            name: "run".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            category: "Timers".to_string(),
            terminal: false,
            inputs: vec![],
            outputs: vec![],
            events: vec!["run".into()],
            required_contexts: vec![],
            lambda: FuncLambda::None,
            ..Default::default()
        });

        EditorFuncLib { func_lib: invoker }
    }
}

impl From<EditorFuncLib> for FuncLib {
    fn from(value: EditorFuncLib) -> Self {
        value.func_lib
    }
}
