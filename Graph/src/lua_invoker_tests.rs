#[cfg(test)]
mod lua_invoker_tests {
    use std::fs;
    use mlua::{Function, Lua, Value, Variadic};

    #[test]
    fn lua_works() {
        let lua = Lua::new();

        lua.load(r#"
        function test_func(a, b, t)
            local sum = a * b + t.val0 + t.val1
            return sum, "hello world!"
        end
        "#).exec().unwrap();

        let var_args = Variadic::from_iter(
            vec![
                Value::Integer(3),
                Value::String(lua.create_string("111").unwrap()),
                Value::Table(lua.create_table_from(vec![
                    ("val0", 11),
                    ("val1", 7),
                ]).unwrap()),
            ]
        );

        let test_func: Function = lua.globals().get("test_func").unwrap();
        let result: Variadic<Value> = test_func.call(var_args).unwrap();

        for value in result {
            match value {
                Value::Integer(int) => { assert_eq!(int, 351); }
                Value::String(text) => { assert_eq!(text, "hello world!") }
                _ => {}
            }
        }
    }

    #[test]
    fn lua_from_file() {
        let _script = fs::read_to_string("./test_resources/test_lua.lua").unwrap();
    }
}
