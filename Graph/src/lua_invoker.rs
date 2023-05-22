use rlua::Lua;
use crate::invoke::*;

pub struct LuaInvoker {
    lua: Lua,

}

impl LuaInvoker {
    pub fn new() -> LuaInvoker {
        LuaInvoker {
            lua: Lua::new(),
        }
    }

    pub fn test(&self) {}
}

impl Invokable for LuaInvoker {
    fn call(&self, _inputs: &Args, _outputs: &mut Args) {}
}