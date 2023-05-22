
use crate::invoke::*;

pub struct LuaInvoker {


}

impl LuaInvoker {
    pub fn new() -> LuaInvoker {
        LuaInvoker {

        }
    }

    pub fn load(&self, _script: &str) {

    }
    pub fn load_file(&self, file: &str) {
        let script = std::fs::read_to_string(file).unwrap();
        self.load(&script);
    }
}

impl Invoker for LuaInvoker {
    fn call(&mut self, _function_name: &str, _context_id: u32, _inputs: &Args, _outputs: &mut Args) {

    }

    fn finish(&mut self) {}
}
