use anyhow::Error;
use common::output_stream::OutputStream;
use hashbrown::HashMap;
use std::fmt::Debug;
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
    funcs: Arc<Mutex<HashMap<FuncId, mlua::Function>>>,
}

impl Default for LuaInvoker {
    fn default() -> Self {
        LuaInvoker {
            lua: Arc::new(mlua::Lua::new()),
            output_stream: Arc::new(Mutex::new(None)),
            func_lib: FuncLib::default(),
            funcs: Arc::new(Mutex::new(HashMap::new())),
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

                for arg in args {
                    match arg {
                        mlua::Value::Nil => {
                            output += "Nil";
                        }
                        mlua::Value::Boolean(v) => {
                            output += &v.to_string();
                        }
                        mlua::Value::LightUserData(_) => {
                            output += "LightUserData";
                        }
                        mlua::Value::Integer(v) => {
                            output += &v.to_string();
                        }
                        mlua::Value::Number(v) => {
                            output += &v.to_string();
                        }
                        mlua::Value::String(v) => {
                            output += v.to_str().expect("Lua string is not valid UTF-8").as_ref();
                        }
                        mlua::Value::Table(_) => {
                            output += "Table";
                        }
                        mlua::Value::Function(_) => {
                            output += "Function";
                        }
                        mlua::Value::Thread(_) => {
                            output += "Thread";
                        }
                        mlua::Value::UserData(_) => {
                            output += "UserData";
                        }
                        mlua::Value::Error(err) => output += &err.to_string(),

                        _ => {
                            panic!("not supported");
                        }
                    }
                }

                let mut guard = output_stream
                    .try_lock()
                    .expect("Output stream mutex is already locked");
                let _ = guard.as_mut().is_some_and(|stream| {
                    stream.write(output);
                    true
                });
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
        self.funcs
            .try_lock()
            .expect("Lua function map mutex is already locked")
            .clear();
        while let Ok(function_table) = functions_table.pop() {
            let mut func = Self::function_from_table(&function_table)?;
            let lua_func: mlua::Function = self.lua.globals().get(func.name.as_str())?;

            let func_id = func.id;
            let lua = self.lua.clone();
            let func_clone = func.clone();
            self.funcs
                .try_lock()
                .expect("Lua function map mutex is already locked")
                .insert(func_id, lua_func.clone());

            func.lambda = Some(FuncLambda::new(move |_cache, inputs, outputs| {
                assert_eq!(
                    inputs.len(),
                    func_clone.inputs.len(),
                    "Lua function {} input length mismatch",
                    func_clone.name
                );
                assert_eq!(
                    outputs.len(),
                    func_clone.outputs.len(),
                    "Lua function {} output length mismatch",
                    func_clone.name
                );
                let mut input_args: mlua::Variadic<mlua::Value> = mlua::Variadic::new();
                for (index, input_info) in func_clone.inputs.iter().enumerate() {
                    let input = &inputs[index];
                    assert_eq!(input_info.data_type, *input.data_type());

                    let invoke_value = to_lua_value(&lua, input)?;
                    input_args.push(invoke_value);
                }

                let output_args: mlua::Variadic<mlua::Value> = lua_func.call(input_args)?;
                assert_eq!(
                    output_args.len(),
                    func_clone.outputs.len(),
                    "Lua function {} returned unexpected output count",
                    func_clone.name
                );

                for (index, output_info) in func_clone.outputs.iter().enumerate() {
                    let output_arg: &mlua::Value = output_args
                        .get(index)
                        .expect("Missing output value from Lua call");

                    let output = data::DynamicValue::from(output_arg);
                    assert_eq!(output_info.data_type, *output.data_type());
                    outputs[index] = output;
                }

                Ok(())
            }));
            self.func_lib.add(func);
        }

        Ok(())
    }
    fn function_from_table(table: &mlua::Table) -> anyhow::Result<Func> {
        let id_str: String = table.get("id")?;

        let mut function_info = Func {
            id: FuncId::from_str(&id_str)?,
            name: table.get("name")?,
            category: "".to_string(),
            description: None,
            behavior: FuncBehavior::Impure,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
            lambda: None,
        };

        let inputs: mlua::Table = table.get("inputs")?;
        for i in 1..=inputs.len()? {
            let input: mlua::Table = inputs.get(i)?;
            let name: String = input.get(1)?;
            let data_type_name: String = input.get(2)?;
            let data_type = data_type_name
                .parse::<DataType>()
                .map_err(|_| Error::msg("Error parsing DataType"))?;

            let mut default_value: Option<data::StaticValue> = None;
            if input.len()? > 2 {
                default_value = None;
            }

            function_info.inputs.push(function::FuncInput {
                name,
                required: true,
                data_type,
                default_value,
                variants: vec![],
            });
        }

        let outputs: mlua::Table = table.get("outputs")?;
        for i in 1..=outputs.len()? {
            let output: mlua::Table = outputs.get(i).expect("Missing output entry in Lua table");
            let name: String = output.get(1).expect("Missing output name in Lua table");
            let data_type_name: String = output.get(2).expect("Missing output type in Lua table");
            let data_type = data_type_name
                .parse::<DataType>()
                .expect("Invalid data type name in Lua table");

            function_info
                .outputs
                .push(function::FuncOutput { name, data_type });
        }

        Ok(function_info)
    }

    pub fn map_graph(&self) -> anyhow::Result<Graph> {
        let connections: Arc<Mutex<Vec<FuncConnections>>> = Arc::new(Mutex::new(Vec::new()));

        self.substitute_functions(Arc::clone(&connections));

        let graph_function: mlua::Function = self.lua.globals().get("graph")?;
        graph_function.call::<()>(())?;

        self.restore_functions();

        let connections: Vec<FuncConnections> = std::mem::take(
            &mut connections
                .try_lock()
                .expect("Connections mutex is already locked"),
        );
        let graph = self.create_graph(connections);
        Ok(graph)
    }

    fn substitute_functions(&self, connections: Arc<Mutex<Vec<FuncConnections>>>) {
        let mut output_index: u32 = 0;

        for func in self.func_lib.funcs.iter() {
            let func_clone = func.clone();
            let connections = Arc::clone(&connections);

            let new_function = self
                .lua
                .create_function(
                    move |_lua: &mlua::Lua,
                          mut inputs: mlua::Variadic<mlua::Value>|
                          -> Result<mlua::Variadic<mlua::Value>, mlua::Error> {
                        let mut connection = FuncConnections {
                            name: func_clone.name.clone(),
                            inputs: Vec::new(),
                            outputs: Vec::new(),
                        };

                        while let Some(input) = inputs.pop() {
                            if let mlua::Value::Integer(output_index) = input {
                                connection.inputs.push(output_index as u32);
                            }
                        }

                        let mut result: mlua::Variadic<mlua::Value> = mlua::Variadic::new();
                        for i in 0..func_clone.outputs.len() {
                            let index = output_index + i as u32;
                            result.push(mlua::Value::Integer(index as i64));
                            connection.outputs.push(index);
                        }

                        connections
                            .try_lock()
                            .expect("Connections mutex is already locked")
                            .push(connection);

                        Ok(result)
                    },
                )
                .expect("Failed to register Lua function");

            self.lua
                .globals()
                .set(func.name.as_str(), new_function)
                .expect("Failed to bind Lua function");

            output_index += func.outputs.len() as u32;
        }
    }
    fn restore_functions(&self) {
        let funcs = self
            .funcs
            .try_lock()
            .expect("Lua function map mutex is already locked");
        for (&id, lua_func) in funcs.iter() {
            let func = self
                .func_lib
                .by_id(id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", id));
            self.lua
                .globals()
                .set(func.name.clone(), lua_func.clone())
                .expect("Failed to restore Lua function");
        }
    }
    fn create_graph(&self, mut connections: Vec<FuncConnections>) -> Graph {
        let mut graph = Graph::default();

        #[derive(Debug)]
        struct OutputAddr {
            idx: usize,
            node_id: NodeId,
        }
        let mut output_ids: HashMap<u32, OutputAddr> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::new();

        for connection in connections.iter() {
            let func = self
                .func_lib
                .by_name(&connection.name)
                .unwrap_or_else(|| panic!("Func named {:?} not found", connection.name));

            nodes.push(Node::from_function(func));
            let node = nodes
                .last_mut()
                .expect("Missing node while building Lua graph");

            assert!(
                connection.inputs.len() <= node.inputs.len(),
                "Lua connections exceed function input count for {}",
                node.name
            );
            for (idx, output_id) in connection.outputs.iter().cloned().enumerate() {
                assert!(!node.id.is_nil());
                output_ids.insert(
                    output_id,
                    OutputAddr {
                        idx,
                        node_id: node.id,
                    },
                );
            }
        }

        while let Some(connection) = connections.pop() {
            let mut node = nodes.pop().expect("Missing node while wiring Lua graph");

            for (input_index, output_id) in connection.inputs.iter().enumerate() {
                let input = &mut node.inputs[input_index];
                let output_addr = output_ids
                    .get(output_id)
                    .expect("Missing output address for Lua graph");

                input.binding = Binding::from_output_binding(output_addr.node_id, output_addr.idx)
            }

            graph.add(node);
        }

        assert!(graph.validate().is_ok());

        graph
    }

    pub fn run(&self) -> anyhow::Result<()> {
        let lua_func: mlua::Function = self.lua.globals().get("graph")?;
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
        DynamicValue::Float(v) => Ok(mlua::Value::Number(*v as mlua::Number)),
        DynamicValue::Int(v) => Ok(mlua::Value::Integer(*v as mlua::Integer)),
        DynamicValue::Bool(v) => Ok(mlua::Value::Boolean(*v)),
        DynamicValue::String(v) => {
            let lua_string = lua.create_string(v)?;
            Ok(mlua::Value::String(lua_string))
        }
        _ => panic!("not supported"),
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
            _ => {
                panic!("not supported")
            }
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
        #[derive(Debug)]
        struct TestStruct {
            a: i32,
            b: i32,
        }
        let lua = mlua::Lua::new();

        let data = TestStruct { a: 4, b: 5 };

        let test_function = lua
            .create_function(move |_, ()| Ok(data.a + data.b))
            .unwrap();
        lua.globals().set("test_func", test_function).unwrap();

        let r: i32 = lua.load("test_func()").eval().unwrap();

        assert_eq!(r, 9);
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
        invoker.func_lib().invoke_by_id(
            FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")?,
            &mut cache,
            inputs.as_mut_slice(),
            outputs.as_mut_slice(),
        )?;
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

        let binding = mult_node.inputs[0]
            .binding
            .as_output_binding()
            .expect("Missing output binding");
        let bound_node = graph
            .by_id(binding.output_node_id)
            .unwrap_or_else(|| panic!("Node with id {:?} not found", binding.output_node_id));
        assert_eq!(bound_node.name, "sum");

        let binding = mult_node.inputs[1]
            .binding
            .as_output_binding()
            .expect("Missing output binding");
        let bound_node = graph
            .by_id(binding.output_node_id)
            .unwrap_or_else(|| panic!("Node with id {:?} not found", binding.output_node_id));
        assert_eq!(bound_node.name, "get_b");

        invoker.run()?;

        let output = output_stream.take().await;
        assert_eq!(output[0], "117");

        Ok(())
    }
}
