use crate::invoke::*;

pub struct LuaInvoker {}

impl LuaInvoker {
    pub fn new() -> LuaInvoker {
        LuaInvoker {}
    }
}

impl Invokable for LuaInvoker {
    fn call(&self, inputs: &Args, outputs: &mut Args) {

    }
}