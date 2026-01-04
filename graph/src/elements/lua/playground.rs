use common::Shared;
use mlua::FromLuaMulti;


#[derive(Debug, Clone)]
struct LuaCtx {
    inner: Shared<mlua::Lua>,
}

impl Default for LuaCtx {
    fn default() -> Self {
        LuaCtx {
            inner: Shared::new(mlua::Lua::new()),
        }
    }
}

impl LuaCtx {
    async fn load(&self, script: &str) -> Result<(), mlua::Error> {
        let lua = self.inner.lock().await;
        lua.load(script).exec()
    }

    async fn call<R: FromLuaMulti>(&self, func: &str) -> Result<R, mlua::Error> {
        let lua = self.inner.lock().await;
        let func: mlua::Function = lua.globals().get(func)?;
        func.call(())
    }
}

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

        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let lua = LuaCtx::default();
                lua.load(script).await.unwrap();
                let n = lua.call::<u32>("test").await.unwrap();
                assert_eq!(42, n);

                for _ in 0..1000 {
                    let lua = lua.clone();

                    tokio::spawn(async move {
                        let n = lua.call::<u32>("test").await.unwrap();
                        assert_eq!(42, n);
                    })
                    .await
                    .unwrap();
                }
            });
    }
}
