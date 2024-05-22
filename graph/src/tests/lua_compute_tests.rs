use std::str::FromStr;

use common::output_stream::OutputStream;

use crate::compute::ArgSet;
use crate::elements::lua_invoker::LuaInvoker;
use crate::function::FuncId;
use crate::invoke_context::{InvokeCache, Invoker};

#[test]
fn lua_works() {
    let lua = mlua::Lua::new();

    lua.load(
        r#"
        function test_func(a, b, t)
            local sum = a * b + t.val0 + t.val1
            return sum, "hello world!"
        end
        "#,
    )
        .exec()
        .unwrap();

    let var_args = mlua::Variadic::from_iter(vec![
        mlua::Value::Integer(3),
        mlua::Value::String(lua.create_string("111").unwrap()),
        mlua::Value::Table(
            lua.create_table_from(vec![("val0", 11), ("val1", 7)])
                .unwrap(),
        ),
    ]);

    let test_func: mlua::Function = lua.globals().get("test_func").unwrap();
    let result: mlua::Variadic<mlua::Value> = test_func.call(var_args).unwrap();

    for value in result {
        match value {
            mlua::Value::Integer(int) => {
                assert_eq!(int, 351);
            }
            mlua::Value::String(text) => {
                assert_eq!(text, "hello world!")
            }
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
    let lua = mlua::Lua::new();

    let data = TestStruct { a: 4, b: 5 };
    let data_ptr = &data as *const TestStruct;

    let test_function = lua
        .create_function(move |_, ()| {
            let local_data = unsafe { &*data_ptr };

            Ok(local_data.a + local_data.b)
        })
        .unwrap();
    lua.globals().set("test_func", test_function).unwrap();

    let r: i32 = lua.load("test_func()").eval().unwrap();

    assert_eq!(r, 9);
}

#[test]
fn load_functions_from_lua_file() -> anyhow::Result<()> {
    let mut invoker = LuaInvoker::default();
    let output_stream = OutputStream::new();
    invoker.use_output_stream(&output_stream);

    invoker.load_file("../test_resources/test_lua.lua")?;

    let funcs = invoker.get_func_lib();
    assert_eq!(funcs.len(), 5);

    let mut inputs: ArgSet = ArgSet::from_vec(vec![3, 5]);
    let mut outputs: ArgSet = ArgSet::from_vec(vec![0]);

    let mut cache = InvokeCache::default();
    // call 'mult' function
    invoker.invoke(
        FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
        &mut cache,
        inputs.as_mut_slice(),
        outputs.as_mut_slice(),
    )?;
    let result: i64 = outputs[0].as_int();
    assert_eq!(result, 15);

    let graph = invoker.map_graph()?;
    assert_eq!(graph.nodes().len(), 5);

    let mult_node = graph
        .nodes()
        .iter()
        .find(|node| node.name == "mult")
        .unwrap();
    assert_eq!(mult_node.inputs.len(), 2);
    assert!(mult_node.inputs[0].binding.is_some());

    let binding = mult_node.inputs[0].binding.as_output_binding().unwrap();
    let bound_node = graph.node_by_id(binding.output_node_id).unwrap();
    assert_eq!(bound_node.name, "sum");

    let binding = mult_node.inputs[1].binding.as_output_binding().unwrap();
    let bound_node = graph.node_by_id(binding.output_node_id).unwrap();
    assert_eq!(bound_node.name, "get_b");

    let output = output_stream.take();
    assert_eq!(output[0], "117");

    Ok(())
}
