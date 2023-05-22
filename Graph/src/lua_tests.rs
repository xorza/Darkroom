#[cfg(test)]
mod lua_tests {
    use std::fs;
    use rlua::Lua;

    #[test]
    fn lua_works() {
        let lua = Lua::new();
        lua.context(|lua_ctx| {
            let globals = lua_ctx.globals();
            globals.set("x", 8).unwrap();
            globals.set("y", 3).unwrap();
            let z: i32 = lua_ctx.load("x + y").eval().unwrap();

            assert_eq!(z, 11);
        });
        lua.context(|lua_ctx| {
            let z: i32 = lua_ctx.load("x * y").eval().unwrap();

            assert_eq!(z, 24);
        });
    }

    #[test]
    fn lua_from_file() {
        let script = fs::read_to_string("./test_resources/test_lua.lua").unwrap();

        let lua = Lua::new();
        lua.context(|lua_ctx| {
            let z: i32 = lua_ctx.load(&script).eval().unwrap();

            assert_eq!(z, 466);
        });
    }
}