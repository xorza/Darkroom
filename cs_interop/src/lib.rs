#![allow(dead_code)]
#![deny(improper_ctypes_definitions)]

use crate::ffi::FfiBuf;
use graph::ctx::{CallbackType, Context};
use graph::elements::basic_invoker::BasicInvoker;
use graph::elements::timers_invoker::TimersInvoker;
use graph::invoke::Invoker;
use std::ffi::c_void;

mod ffi;
mod func_lib_api;
mod graph_api;
mod utils;

#[no_mangle]
extern "C" fn create_context() -> *mut c_void {
    let mut context = Box::<Context>::default();
    context.invoker.merge(BasicInvoker::default());
    context.invoker.merge(TimersInvoker::default());
    context.func_lib.merge(context.invoker.get_func_lib());

    context.graph =
        graph::graph::Graph::from_yaml(include_str!("../../test_resources/test_graph.yml"))
            .unwrap();

    Box::into_raw(context) as *mut c_void
}

#[no_mangle]
extern "C" fn destroy_context(ctx: *mut c_void) {
    unsafe { drop(Box::<Context>::from_raw(ctx as *mut Context)) };
}

#[no_mangle]
extern "C" fn destroy_ffi_buf(buf: FfiBuf) {
    drop(buf);
}

pub(crate) fn get_context<'a>(ctx: *mut c_void) -> &'a mut Context {
    unsafe { &mut *(ctx as *mut Context) }
}

pub type FfiCallbackDelegate = extern "C" fn(CallbackType);

#[no_mangle]
pub extern "C" fn register_callback(ctx: *mut c_void, callback: FfiCallbackDelegate) {
    let context = get_context(ctx);
    context.callback = Some(Box::from(move |t| {
        callback(t);
    }));
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_create_context() {
        let ctx = super::create_context();
        assert!(!ctx.is_null());
        super::destroy_context(ctx);
    }
}
