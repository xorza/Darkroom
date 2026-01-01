#[macro_export]
macro_rules! async_lambda {
    (move |$($args:pat_param),*| { $($name:ident = $init:expr),* $(,)? } => $body:block) => {
        $crate::function::FuncLambda::new(move |$($args),*| {
            $(let $name = $init;)*
            Box::pin(async move $body)
        })
    };
    (|$($args:pat_param),*| { $($name:ident = $init:expr),* $(,)? } => $body:block) => {
        $crate::function::FuncLambda::new(|$($args),*| {
            $(let $name = $init;)*
            Box::pin(async move $body)
        })
    };
    (move |$($args:pat_param),*| $body:block) => {
        $crate::function::FuncLambda::new(move |$($args),*| {
            Box::pin(async move $body)
        })
    };
    (|$($args:pat_param),*| $body:block) => {
        $crate::function::FuncLambda::new(|$($args),*| {
            Box::pin(async move $body)
        })
    };
}
