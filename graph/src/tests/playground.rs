use std::sync::Arc;
use mlua::FromLuaMulti;

use parking_lot::Mutex;

trait SendSync: Sync + Send {}

#[derive(Debug, Clone)]
struct LuaCtxInternal {
    lua: *mut mlua::Lua,
}

#[derive(Debug, Clone)]
struct LuaCtx {
    inner: Arc<Mutex<LuaCtxInternal>>,
}

impl Default for LuaCtx {
    fn default() -> Self {
        LuaCtx {
            inner: Arc::new(Mutex::new(LuaCtxInternal::default())),
        }
    }
}

impl LuaCtx {
    fn load(&self, script: &str) -> Result<(), mlua::Error> {
        let inner = self.inner.lock();
        inner.load(script)
    }

    fn call<R: FromLuaMulti<'static>>(&self, func: &str) -> Result<R, mlua::Error> {
        let inner = self.inner.lock();
        inner.call(func)
    }
}


impl Default for LuaCtxInternal {
    fn default() -> Self {
        let lua = Box::new(mlua::Lua::new());
        let lua: *mut mlua::Lua = Box::into_raw(lua);
        LuaCtxInternal { lua }
    }
}

impl Drop for LuaCtxInternal {
    fn drop(&mut self) {
        unsafe { drop(Box::from_raw(self.lua)); }
    }
}

impl LuaCtxInternal {
    fn load(&self, script: &str) -> Result<(), mlua::Error> {
        let lua = unsafe { &mut *self.lua };
        lua.load(script).exec()?;

        Ok(())
    }

    fn call<R: FromLuaMulti<'static>>(&self, func: &str) -> Result<R, mlua::Error> {
        let lua = unsafe { &mut *self.lua };
        let func: mlua::Function = lua.globals().get(func)?;
        let r: R = func.call(())?;

        Ok(r)
    }
}

unsafe impl Send for LuaCtx {}

unsafe impl Sync for LuaCtx {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua() {
        let script = r#"
            function test()
                return 42
            end
        "#;

        tokio::runtime::Runtime::new().unwrap()
            .block_on(async move {
                let lua = LuaCtx::default();
                lua.load(script).unwrap();
                let n = lua.call::<u32>("test").unwrap();
                assert_eq!(42, n);

                for _ in 0..1000 {
                    let lua = lua.clone();

                    tokio::spawn(async move {
                        let n = lua.call::<u32>("test").unwrap();
                        assert_eq!(42, n);
                    }).await.unwrap();
                }
            });
    }
}