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

    pub fn load(&self, script: &str) {
        self.lua.context(|lua_ctx| {
            lua_ctx.load(script).exec().unwrap();
        });
    }
    pub fn load_file(&self, file: &str) {
        let script = std::fs::read_to_string(file).unwrap();
        self.load(&script);
    }
}

impl Invokable for LuaInvoker {
    fn call(& self, _context_d: u32, _inputs: &Args, _outputs: &mut Args) {}
}
