use anyhow::Error;
use common::output_stream::OutputStream;
use hashbrown::HashMap;
use std::fmt::Debug;
use std::mem::take;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::data::DataType;
use crate::data::DynamicValue;
use crate::function::{Func, FuncBehavior, FuncId, FuncLambda, FuncLib, InvokeArgs, InvokeCache};
use crate::graph::{Binding, Graph, Node, NodeId};
use crate::{data, function};

#[derive(Clone, Debug)]
struct FuncConnections {
    name: String,
    inputs: Vec<u32>,
    outputs: Vec<u32>,
}

#[derive(Debug)]
pub struct LuaInvoker {
    lua: Arc<mlua::Lua>,
    output_stream: Arc<Mutex<Option<OutputStream>>>,
    func_lib: FuncLib,
    lua_funcs: HashMap<FuncId, mlua::Function>,
}

impl Default for LuaInvoker {
    fn default() -> Self {
        LuaInvoker {
            lua: Arc::new(mlua::Lua::new()),
            output_stream: Arc::new(Mutex::new(None)),
            func_lib: FuncLib::default(),
            lua_funcs: HashMap::new(),
        }
    }
}

impl LuaInvoker {
    pub fn load_file(&mut self, file: &str) -> anyhow::Result<()> {
        let script = std::fs::read_to_string(file)?;
        self.load(&script)?;

        Ok(())
    }

    pub fn load(&mut self, script: &str) -> anyhow::Result<()> {
        let output_stream = self.output_stream.clone();

        let print_function = self.lua.create_function(
            move |_lua: &mlua::Lua, args: mlua::Variadic<mlua::Value>| {
                let mut output = String::new();

                for (index, arg) in args.into_iter().enumerate() {
                    if index > 0 {
                        output.push('\t');
                    }

                    match arg {
                        mlua::Value::Nil => output.push_str("Nil"),
                        mlua::Value::Boolean(v) => output.push_str(&v.to_string()),
                        mlua::Value::LightUserData(_) => output.push_str("LightUserData"),
                        mlua::Value::Integer(v) => output.push_str(&v.to_string()),
                        mlua::Value::Number(v) => output.push_str(&v.to_string()),
                        mlua::Value::String(v) => output
                            .push_str(v.to_str().expect("Lua string is not valid UTF-8").as_ref()),
                        mlua::Value::Table(_) => output.push_str("Table"),
                        mlua::Value::Function(_) => output.push_str("Function"),
                        mlua::Value::Thread(_) => output.push_str("Thread"),
                        mlua::Value::UserData(_) => output.push_str("UserData"),
                        mlua::Value::Error(err) => output.push_str(&err.to_string()),
                        _ => panic!("not supported"),
                    }
                }

                let mut guard = output_stream
                    .try_lock()
                    .expect("Output stream mutex is already locked");
                if let Some(stream) = guard.as_mut() {
                    stream.write(output);
                }
                Ok(())
            },
        )?;
        self.lua.globals().set("print", print_function)?;

        self.lua.load(script).exec()?;

        self.read_function_info()?;

        Ok(())
    }

    fn read_function_info(&mut self) -> anyhow::Result<()> {
        let functions_table: mlua::Table = self.lua.globals().get("functions")?;
        self.lua_funcs.clear();

        while let Ok(function_table) = functions_table.pop() {
            let mut func = Self::function_from_table(&function_table)?;
            let lua_func: mlua::Function = self.lua.globals().get(func.name.as_str())?;

            func.lambda = FuncLambda::new({
                let func = func.clone();
                let lua_func = lua_func.clone();
                let lua = Arc::clone(&self.lua);

                move |_cache, inputs, outputs| {
                    let input_len = inputs.len();
                    let expected_input_len = func.inputs.len();
                    assert_eq!(
                        input_len, expected_input_len,
                        "Lua function {} input length mismatch",
                        func.name
                    );

                    let output_len = outputs.len();
                    let expected_output_len = func.outputs.len();
                    assert_eq!(
                        output_len, expected_output_len,
                        "Lua function {} output length mismatch",
                        func.name
                    );

                    let mut input_args: mlua::Variadic<mlua::Value> = mlua::Variadic::new();
                    for (input_info, input) in func.inputs.iter().zip(inputs.iter()) {
                        assert_eq!(input_info.data_type, *input.data_type());

                        let invoke_value = to_lua_value(&lua, input)?;
                        input_args.push(invoke_value);
                    }

                    let output_args: mlua::Variadic<mlua::Value> =
                        lua_func.call(input_args).map_err(anyhow::Error::from)?;
                    assert_eq!(
                        output_args.len(),
                        expected_output_len,
                        "Lua function {} returned unexpected output count",
                        func.name
                    );

                    for ((index, output_info), output_arg) in
                        func.outputs.iter().enumerate().zip(output_args.into_iter())
                    {
                        let output = data::DynamicValue::from(&output_arg);
                        assert_eq!(output_info.data_type, *output.data_type());
                        outputs[index] = output;
                    }

                    Ok(())
                }
            });

            self.lua_funcs.insert(func.id, lua_func);
            self.func_lib.add(func);
        }

        Ok(())
    }

    fn function_from_table(table: &mlua::Table) -> anyhow::Result<Func> {
        let id_str: String = table.get("id")?;
        let name: String = table.get("name")?;

        let mut function_info = Func {
            id: FuncId::from_str(&id_str)?,
            name,
            category: "".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
            lambda: FuncLambda::None,
        };

        let inputs: mlua::Table = table.get("inputs")?;
        let input_count = inputs.len()?;
        for i in 1..=input_count {
            let input: mlua::Table = inputs.get(i)?;
            let name: String = input.get(1)?;
            let data_type_name: String = input.get(2)?;
            let data_type = data_type_name.parse::<DataType>().map_err(|_| {
                Error::msg(format!(
                    "Error parsing DataType \"{}\" for input {} on function {}",
                    data_type_name, i, function_info.name
                ))
            })?;

            let default_value: Option<data::StaticValue> = None;

            function_info.inputs.push(function::FuncInput {
                name,
                required: true,
                data_type,
                default_value,
                value_options: Vec::new(),
            });
        }

        let outputs: mlua::Table = table.get("outputs")?;
        let output_count = outputs.len()?;
        for i in 1..=output_count {
            let output: mlua::Table = outputs
                .get(i)
                .unwrap_or_else(|_| panic!("Missing output entry {} in Lua table", i));
            let name: String = output
                .get(1)
                .unwrap_or_else(|_| panic!("Missing output name for entry {} in Lua table", i));
            let data_type_name: String = output
                .get(2)
                .unwrap_or_else(|_| panic!("Missing output type for entry {} in Lua table", i));
            let data_type = data_type_name.parse::<DataType>().unwrap_or_else(|_| {
                panic!(
                    "Invalid data type name \"{}\" for output {} on function {}",
                    data_type_name, i, function_info.name
                )
            });

            function_info
                .outputs
                .push(function::FuncOutput { name, data_type });
        }

        Ok(function_info)
    }

    pub fn map_graph(&self) -> anyhow::Result<Graph> {
        let connections = self.build_connections()?;

        let graph = self.create_graph(connections);

        Ok(graph)
    }

    fn build_connections(&self) -> anyhow::Result<Vec<FuncConnections>> {
        let connections: Arc<Mutex<Vec<FuncConnections>>> = Arc::new(Mutex::new(Vec::new()));
        let mut output_index: u32 = 0;

        // substitute functions
        for func in self.func_lib.funcs.iter() {
            let outputs_len = func.outputs.len();

            let new_function = self
                .lua
                .create_function({
                    let connections_ref = Arc::clone(&connections);
                    let func_name = func.name.clone();
                    let outputs_len = outputs_len as u32;

                    move |_lua: &mlua::Lua,
                          mut inputs: mlua::Variadic<mlua::Value>|
                          -> Result<mlua::Variadic<mlua::Value>, mlua::Error> {
                        let mut connection = FuncConnections {
                            name: func_name.clone(),
                            inputs: Vec::new(),
                            outputs: Vec::with_capacity(outputs_len as usize),
                        };

                        while let Some(input) = inputs.pop() {
                            if let mlua::Value::Integer(output_index) = input {
                                connection.inputs.push(output_index as u32);
                            }
                        }

                        let mut result: mlua::Variadic<mlua::Value> = mlua::Variadic::new();
                        for idx in 0..outputs_len {
                            let index = output_index + idx;
                            result.push(mlua::Value::Integer(index as mlua::Integer));
                            connection.outputs.push(index);
                        }

                        connections_ref
                            .try_lock()
                            .expect("Connections mutex is already locked")
                            .push(connection);

                        Ok(result)
                    }
                })
                .expect("Failed to register Lua function");

            self.lua
                .globals()
                .set(func.name.as_str(), new_function)
                .expect("Failed to bind Lua function");

            output_index += outputs_len as u32;
        }

        let graph_function: mlua::Function = self.lua.globals().get("graph")?;
        graph_function.call::<()>(())?;

        // restore functions
        for (&id, lua_func) in self.lua_funcs.iter() {
            let func = self
                .func_lib
                .by_id(id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", id));
            self.lua
                .globals()
                .set(func.name.clone(), lua_func.clone())
                .expect("Failed to restore Lua function");
        }

        let mut guard = connections
            .try_lock()
            .expect("Connections mutex is already locked");
        Ok(take(&mut *guard))
    }

    fn create_graph(&self, connections: Vec<FuncConnections>) -> Graph {
        #[derive(Debug)]
        struct OutputAddr {
            idx: usize,
            node_id: NodeId,
        }
        let mut output_ids: HashMap<u32, OutputAddr> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::with_capacity(connections.len());

        for connection in connections.iter() {
            let func = self
                .func_lib
                .by_name(&connection.name)
                .unwrap_or_else(|| panic!("Func named {:?} not found", connection.name));

            let node = Node::from_function(func);

            assert!(
                connection.inputs.len() <= node.inputs.len(),
                "Lua connections exceed function input count for {}",
                node.name
            );
            assert!(!node.id.is_nil());
            for (idx, output_id) in connection.outputs.iter().cloned().enumerate() {
                output_ids.insert(
                    output_id,
                    OutputAddr {
                        idx,
                        node_id: node.id,
                    },
                );
            }

            nodes.push(node);
        }

        let mut graph = Graph::default();
        for (connection, mut node) in connections.into_iter().rev().zip(nodes.into_iter().rev()) {
            for (input_index, output_id) in connection.inputs.iter().enumerate() {
                let input = &mut node.inputs[input_index];
                let output_addr = output_ids
                    .get(output_id)
                    .expect("Missing output address for Lua graph");

                input.binding = Binding::from_output_binding(output_addr.node_id, output_addr.idx)
            }

            graph.add(node);
        }

        graph
            .validate()
            .expect("Lua graph validation failed after wiring");

        graph
    }

    pub fn call(&self, func_name: &str) -> anyhow::Result<()> {
        let lua_func: mlua::Function = self.lua.globals().get(func_name)?;
        lua_func.call::<()>(())?;

        Ok(())
    }

    pub(crate) async fn use_output_stream(&mut self, output_stream: &OutputStream) {
        self.output_stream
            .lock()
            .await
            .replace(output_stream.clone());
    }

    pub(crate) fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }
}

fn to_lua_value(lua: &mlua::Lua, value: &DynamicValue) -> anyhow::Result<mlua::Value> {
    match value {
        DynamicValue::Null => Ok(mlua::Value::Nil),
        DynamicValue::Float(v) => Ok(mlua::Value::Number(*v)),
        DynamicValue::Int(v) => Ok(mlua::Value::Integer(*v)),
        DynamicValue::Bool(v) => Ok(mlua::Value::Boolean(*v)),
        DynamicValue::String(v) => Ok(mlua::Value::String(lua.create_string(v)?)),
        _ => panic!(
            "Lua value conversion does not support {:?}",
            value.data_type()
        ),
    }
}

impl From<&mlua::Value> for DynamicValue {
    fn from(value: &mlua::Value) -> Self {
        match value {
            mlua::Value::Nil => DynamicValue::Null,
            mlua::Value::Boolean(v) => (*v).into(),
            mlua::Value::Integer(v) => (*v).into(),
            mlua::Value::Number(v) => (*v).into(),
            mlua::Value::String(v) => {
                DynamicValue::from(v.to_str().expect("Lua string is not valid UTF-8").as_ref())
            }
            _ => panic!("Lua value conversion does not support {:?}", value),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use common::output_stream::OutputStream;

    use super::*;
    use crate::compute::ArgSet;
    use crate::function::FuncId;

    #[test]
    fn lua_works() -> anyhow::Result<()> {
        let lua = mlua::Lua::new();

        lua.load(
            r#"
        function test_func(a, b, t)
            local sum = a * b + t.val0 + t.val1
            return sum, "hello world!"
        end
        "#,
        )
        .exec()?;

        let var_args = mlua::Variadic::from_iter(vec![
            mlua::Value::Integer(3),
            mlua::Value::String(lua.create_string("111")?),
            mlua::Value::Table(lua.create_table_from(vec![("val0", 11), ("val1", 7)])?),
        ]);

        let test_func: mlua::Function = lua.globals().get("test_func")?;
        let result: mlua::Variadic<mlua::Value> = test_func.call(var_args)?;
        let mut result = result.into_iter();
        let sum = match result.next().expect("Missing sum result") {
            mlua::Value::Integer(value) => value,
            other => panic!("Sum result must be an integer, got {:?}", other),
        };
        assert_eq!(sum, 351);

        let message = match result.next().expect("Missing message result") {
            mlua::Value::String(value) => value
                .to_str()
                .expect("Message result must be valid UTF-8")
                .to_string(),
            other => panic!("Message result must be a string, got {:?}", other),
        };
        assert_eq!(message, "hello world!");

        Ok(())
    }

    #[test]
    fn local_data_test() -> anyhow::Result<()> {
        #[derive(Debug)]
        struct TestStruct {
            a: i32,
            b: i32,
        }
        let lua = mlua::Lua::new();
        let data = TestStruct { a: 4, b: 5 };
        let test_function = lua.create_function(move |_, ()| Ok(data.a + data.b))?;
        lua.globals().set("test_func", test_function)?;
        let result: i32 = lua.load("test_func()").eval()?;
        assert_eq!(result, 9);

        Ok(())
    }

    #[tokio::test]
    async fn load_functions_from_lua_file() -> anyhow::Result<()> {
        let mut invoker = LuaInvoker::default();
        let output_stream = OutputStream::new();
        invoker.use_output_stream(&output_stream).await;

        invoker.load_file("../test_resources/test_lua.lua")?;

        let funcs = invoker.func_lib();
        assert_eq!(funcs.funcs.len(), 5);

        let mut inputs: ArgSet = ArgSet::from_vec(vec![3, 5]);
        let mut outputs: ArgSet = ArgSet::from_vec(vec![0]);

        let mut cache = InvokeCache::default();
        // call 'mult' function
        invoker
            .func_lib()
            .invoke_by_id(
                FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
                &mut cache,
                inputs.as_mut_slice(),
                outputs.as_mut_slice(),
            )
            .map_err(anyhow::Error::from)?;
        let result: i64 = outputs[0].as_int();
        assert_eq!(result, 15);

        let graph = invoker.map_graph()?;
        assert_eq!(graph.nodes.len(), 5);

        let mult_node = graph
            .nodes
            .iter()
            .find(|node| node.name == "mult")
            .expect("Missing mult node");
        assert_eq!(mult_node.inputs.len(), 2);
        assert!(mult_node.inputs[0].binding.is_some());

        let bound_name = |index: usize| -> &str {
            let binding = mult_node.inputs[index]
                .binding
                .as_output_binding()
                .unwrap_or_else(|| panic!("Missing output binding for input {}", index));
            graph
                .by_id(binding.output_node_id)
                .unwrap_or_else(|| panic!("Node with id {:?} not found", binding.output_node_id))
                .name
                .as_str()
        };
        assert_eq!(bound_name(0), "sum");
        assert_eq!(bound_name(1), "get_b");

        invoker.call("graph")?;

        let output = output_stream.take().await;
        assert_eq!(output[0], "117");

        Ok(())
    }

    #[tokio::test]
    async fn invoke_lua_mult_function() -> anyhow::Result<()> {
        let mut invoker = LuaInvoker::default();
        invoker.load(include_str!("../../../../test_resources/test_lua.lua"))?;

        let mut inputs: ArgSet = ArgSet::from_vec(vec![6, 7]);
        let mut outputs: ArgSet = ArgSet::from_vec(vec![0]);
        let mut cache = InvokeCache::default();

        invoker
            .func_lib()
            .invoke_by_id(
                FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
                &mut cache,
                inputs.as_mut_slice(),
                outputs.as_mut_slice(),
            )
            .map_err(anyhow::Error::from)?;
        let result: i64 = outputs[0].as_int();
        assert_eq!(result, 42);

        Ok(())
    }
}
