/// Wrap a lambda body as a [`FuncLambda`](crate::FuncLambda): the closure takes one
/// [`Invocation`](crate::Invocation) — destructure it in the pattern to name the parts you
/// use — and the body becomes the boxed future the ABI expects.
///
/// The `{ name = init }` form runs its bindings *before* the future is created, for state a
/// `move` closure must clone once per call rather than once per registration.
#[macro_export]
macro_rules! async_lambda {
    (move |$invocation:pat_param| { $($name:ident = $init:expr),* $(,)? } => $body:block) => {
        $crate::FuncLambda::new(move |$invocation| {
            $(let $name = $init;)*
            Box::pin(async move $body)
        })
    };
    (|$invocation:pat_param| { $($name:ident = $init:expr),* $(,)? } => $body:block) => {
        $crate::FuncLambda::new(|$invocation| {
            $(let $name = $init;)*
            Box::pin(async move $body)
        })
    };
    (move |$invocation:pat_param| $body:block) => {
        $crate::FuncLambda::new(move |$invocation| Box::pin(async move $body))
    };
    (|$invocation:pat_param| $body:block) => {
        $crate::FuncLambda::new(|$invocation| Box::pin(async move $body))
    };
}
