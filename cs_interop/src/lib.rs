#![allow(dead_code)]
#![deny(improper_ctypes_definitions)]

use crate::ffi::FfiBuf;
use graph::ctx::Context;
use graph::elements::basic_invoker::BasicInvoker;
use graph::elements::timers_invoker::TimersInvoker;
use graph::invoke::Invoker;
use std::ffi::c_void;
use std::sync::Arc;
use tokio::sync::Mutex;

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

#[repr(u32)]
pub enum CallbackType {
    OnGraphUpdate,
    OnFuncLibUpdate,
}

pub type CallbackDelegate = extern "C" fn(CallbackType);

lazy_static::lazy_static! {
    static ref CALLBACK: Arc<Mutex<Option<CallbackDelegate >>> = Arc::new(Mutex::new(None));
}

#[no_mangle]
pub extern "C" fn register_callback(callback: CallbackDelegate) {
    let mut cb = CALLBACK.blocking_lock();
    *cb = Some(callback);
}

pub fn trigger_callback(value: CallbackType) {
    let cb = CALLBACK.blocking_lock();
    if let Some(callback) = *cb {
        callback(value);
    }
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
