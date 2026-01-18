#[macro_export]
macro_rules! async_lambda {
    (move |$ctx_manager:pat_param, $state:pat_param, $event_state:pat_param, $inputs:pat_param, $outputs_meta:pat_param, $outputs:pat_param| { $($name:ident = $init:expr),* $(,)? } => $body:block) => {
        $crate::lambda::FuncLambda::new(move |$ctx_manager, $state, $event_state, $inputs, $outputs_meta, $outputs| {
            $(let $name = $init;)*
            Box::pin(async move $body)
        })
    };
    (|$ctx_manager:pat_param, $state:pat_param, $event_state:pat_param, $inputs:pat_param, $outputs_meta:pat_param, $outputs:pat_param| { $($name:ident = $init:expr),* $(,)? } => $body:block) => {
        $crate::lambda::FuncLambda::new(|$ctx_manager, $state, $event_state, $inputs, $outputs_meta, $outputs| {
            $(let $name = $init;)*
            Box::pin(async move $body)
        })
    };
    (move |$ctx_manager:pat_param, $state:pat_param, $event_state:pat_param, $inputs:pat_param, $outputs_meta:pat_param, $outputs:pat_param| $body:block) => {
        $crate::lambda::FuncLambda::new(move |$ctx_manager, $state, $event_state, $inputs, $outputs_meta, $outputs| {
            Box::pin(async move $body)
        })
    };
    (|$ctx_manager:pat_param, $state:pat_param, $event_state:pat_param, $inputs:pat_param, $outputs_meta:pat_param, $outputs:pat_param| $body:block) => {
        $crate::lambda::FuncLambda::new(|$ctx_manager, $state, $event_state, $inputs, $outputs_meta, $outputs| {
            Box::pin(async move $body)
        })
    };
}
