use std::str::FromStr;

use mlua::{Function, Lua, Value, Variadic};

use crate::compute::ArgSet;
use crate::function::FunctionId;
use crate::invoke::Invoker;
use crate::lua_invoker::LuaInvoker;
use crate::runtime_graph::InvokeContext;

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
fn local_data_test() {
    struct TestStruct {
        a: i32,
        b: i32,
    }
    let lua = Lua::new();

    let data = TestStruct { a: 4, b: 5 };
    let data_ptr = &data as *const TestStruct;

    let test_function = lua.create_function(move |_, ()| {
        let local_data = unsafe { &*data_ptr };

        Ok(local_data.a + local_data.b)
    }).unwrap();
    lua.globals().set("test_func", test_function).unwrap();

    let r: i32 = lua.load("test_func()").eval().unwrap();

    assert_eq!(r, 9);
}

#[test]
fn load_functions_from_lua_file() -> anyhow::Result<()> {
    let mut invoker = LuaInvoker::default();
    invoker.load_file("../test_resources/test_lua.lua")?;

    let funcs = invoker.get_all_functions();
    assert_eq!(funcs.len(), 5);

    let inputs: ArgSet = ArgSet::from_vec(vec![Some(3), Some(5)]);
    let mut outputs: ArgSet = ArgSet::from_vec(vec![Some(0)]);

    let mut ctx = InvokeContext::default();
    // call 'mult' function
    invoker.invoke(
        FunctionId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
        &mut ctx,
        inputs.as_slice(),
        outputs.as_mut_slice(),
    )?;
    let result: i64 = outputs[0].as_ref().unwrap().as_int();
    assert_eq!(result, 15);

    let graph = invoker.map_graph()?;
    assert_eq!(graph.nodes().len(), 5);

    let mult_node = graph.nodes().iter().find(|node| node.name == "mult").unwrap();
    assert_eq!(mult_node.inputs.len(), 2);
    assert!(mult_node.inputs[0].binding.is_some());

    let binding = mult_node.inputs[0].binding.as_output_binding().unwrap();
    let bound_node = graph.node_by_id(binding.output_node_id).unwrap();
    assert_eq!(bound_node.name, "sum");

    let binding = mult_node.inputs[1].binding.as_output_binding().unwrap();
    let bound_node = graph.node_by_id(binding.output_node_id).unwrap();
    assert_eq!(bound_node.name, "val1");

    let output = invoker.get_output();
    assert_eq!(output, "52");

    Ok(())
}
