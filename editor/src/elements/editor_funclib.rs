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
    pub const RUN_FUNC_ID: FuncId = FuncId::from_u128(0xe871ddf47a534ae59728927a88649673);

    pub fn new(run_event: Arc<Notify>) -> EditorFuncLib {
        let mut invoker = FuncLib::default();
        let lambda = EventLambda::new(move || {
            let run_event = Arc::clone(&run_event);
            Box::pin(async move {
                run_event.notified().await;
                0
            })
        });

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
            event_lambda: lambda,
        });

        EditorFuncLib { func_lib: invoker }
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
