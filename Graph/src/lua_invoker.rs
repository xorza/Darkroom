use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::transmute;
use std::rc::Rc;

use mlua::{Error, Function, Lua, Table, Variadic};
use uuid::Uuid;

use crate::{data, functions, invoker};
use crate::data::DataType;
use crate::graph::{Binding, Graph, Input, Node, Output};
use crate::invoker::{InvokeArgs, Invoker};
use crate::runtime::RuntimeInfo;

#[derive(Default)]
struct Cache {
    output_stream: Vec<String>,
}

struct LuaFuncInfo {
    info: functions::Function,
    lua_func: Function<'static>,
}

#[derive(Clone)]
struct FuncConnections {
    name: String,
    inputs: Vec<u32>,
    outputs: Vec<u32>,
}

pub(crate) struct LuaInvoker {
    lua: &'static Lua,
    cache: Rc<RefCell<Cache>>,
    funcs: HashMap<Uuid, LuaFuncInfo>,
}

impl Default for LuaInvoker {
    fn default() -> Self {
        let lua = Box::new(Lua::new());
        let lua: &'static Lua = Box::leak(lua);

        LuaInvoker {
            lua,
            cache: Rc::new(RefCell::new(Cache::default())),
            funcs: HashMap::new(),
        }
    }
}

impl LuaInvoker {
    pub fn load_file(&mut self, file: &str) -> anyhow::Result<()> {
        let script = std::fs::read_to_string(file)?;
        self.load(&script)?;

        Ok(())
    }
    //noinspection RsNonExhaustiveMatch
    pub fn load(&mut self, script: &str) -> anyhow::Result<()> {
        let cache = Rc::clone(&self.cache);
        let print_function = self.lua.create_function(
            move |_lua: &Lua, args: Variadic<mlua::Value>| {
                let mut output = String::new();

                for arg in args {
                    match arg {
                        mlua::Value::Nil => { output += "Nil"; }
                        mlua::Value::Boolean(v) => { output += &v.to_string(); }
                        mlua::Value::LightUserData(_) => { output += "LightUserData"; }
                        mlua::Value::Integer(v) => { output += &v.to_string(); }
                        mlua::Value::Number(v) => { output += &v.to_string(); }
                        mlua::Value::String(v) => { output += v.to_str().unwrap(); }
                        mlua::Value::Table(_) => { output += "Table"; }
                        mlua::Value::Function(_) => { output += "Function"; }
                        mlua::Value::Thread(_) => { output += "Thread"; }
                        mlua::Value::UserData(_) => { output += "UserData"; }
                        mlua::Value::Error(err) => { output += &err.to_string() }
                    }
                }

                let mut cache = cache.borrow_mut();
                cache.output_stream.push(output);
                Ok(())
            }
        ).unwrap();
        self.lua.globals().set("print", print_function)?;

        self.lua.load(script).exec()?;

        self.read_function_info()?;

        Ok(())
    }

    fn read_function_info(&mut self) -> anyhow::Result<()> {
        let functions_table: Table = self.lua.globals().get("functions")?;
        while let Ok(function_table) = functions_table.pop() {
            let function_info = Self::function_from_table(&function_table)?;
            let function: Function = self.lua.globals().get(function_info.name.as_str())?;

            self.funcs.insert(
                function_info.id(),
                LuaFuncInfo {
                    info: function_info,
                    lua_func: function,
                },
            );
        }

        Ok(())
    }
    fn function_from_table(table: &Table) -> anyhow::Result<functions::Function> {
        let id_str: String = table.get("id")?;

        let mut function_info = functions::Function::new(Uuid::parse_str(&id_str)?);
        function_info.name = table.get("name")?;
        function_info.inputs = Vec::new();
        function_info.outputs = Vec::new();

        let inputs: Table = table.get("inputs")?;
        for i in 1..=inputs.len()? {
            let input: Table = inputs.get(i).unwrap();
            let name: String = input.get(1).unwrap();
            let data_type_name: String = input.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            let mut default_value: Option<data::Value> = None;
            if input.len()? > 2 {
                default_value = None;
            }

            function_info.inputs.push(functions::InputInfo { name, data_type, const_value: default_value });
        }

        let outputs: Table = table.get("outputs")?;
        for i in 1..=outputs.len()? {
            let output: Table = outputs.get(i).unwrap();
            let name: String = output.get(1).unwrap();
            let data_type_name: String = output.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            function_info.outputs.push(functions::OutputInfo { name, data_type });
        }

        Ok(function_info)
    }

    pub fn map_graph(&self) -> anyhow::Result<Graph> {
        let connections = self.substitute_functions();

        self.restore_functions();

        let graph_function: Function = self.lua.globals().get("graph").unwrap();
        graph_function.call::<_, ()>(()).unwrap();

        let graph = self.create_graph(connections);

        Ok(graph)
    }

    fn substitute_functions(&self) -> Vec<FuncConnections> {
        let connections: Rc<RefCell<Vec<FuncConnections>>> = Rc::new(RefCell::new(Vec::new()));

        let functions = self.get_all_functions();

        let mut output_index: u32 = 0;

        for lua_func_info in functions {
            let function_info_clone = lua_func_info.clone();
            let connections = connections.clone();

            let new_function = self.lua.create_function(
                move |_lua: &Lua, mut inputs: Variadic<mlua::Value>| -> Result<Variadic<mlua::Value>, Error>  {
                    let mut connection = FuncConnections {
                        name: function_info_clone.name.clone(),
                        inputs: Vec::new(),
                        outputs: Vec::new(),
                    };

                    while let Some(input) = inputs.pop() {
                        if let mlua::Value::Integer(output_index) = input {
                            connection.inputs.push(output_index as u32);
                        }
                    }

                    let mut result: Variadic<mlua::Value> = Variadic::new();
                    for i in 0..function_info_clone.outputs.len() {
                        let index = output_index + i as u32;
                        result.push(mlua::Value::Integer(index as i64));
                        connection.outputs.push(index);
                    }

                    let mut connections = connections.borrow_mut();
                    connections.push(connection);

                    Ok(result)
                }
            ).unwrap();

            self.lua.globals().set(lua_func_info.name.clone(), new_function).unwrap();

            output_index += lua_func_info.outputs.len() as u32;
        }

        let graph_function: Function = self.lua.globals().get("graph").unwrap();
        graph_function.call::<_, ()>(()).unwrap();

        connections.take()
    }
    fn restore_functions(&self) {
        let functions = self.funcs.values().collect::<Vec<&LuaFuncInfo>>();

        for lua_func_info in functions.iter() {
            self.lua.globals().set(
                lua_func_info.info.name.clone(),
                lua_func_info.lua_func.clone(),
            ).unwrap();
        }
    }
    fn create_graph(&self, mut connections: Vec<FuncConnections>) -> Graph {
        let mut graph = Graph::default();

        struct OutputAddr {
            index: u32,
            node_id: Uuid,
        }
        let mut output_ids: HashMap<u32, OutputAddr> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::new();

        for connection in connections.iter() {
            let function = &self.funcs
                .iter()
                .find(|(_, func)| func.info.name == connection.name)
                .unwrap()
                .1.info;
            nodes.push(Node::new());
            let node = nodes.last_mut().unwrap();

            node.name = function.name.clone();

            for (i, _input_id) in connection.inputs.iter().enumerate() {
                let input = function.inputs.get(i).unwrap();
                node.inputs.push(Input {
                    name: input.name.clone(),
                    data_type: input.data_type,
                    is_required: true,
                    binding: Binding::None,
                    const_value: None,
                });
            }
            for (i, output_id) in connection.outputs.iter().cloned().enumerate() {
                let output = function.outputs.get(i).unwrap();
                node.outputs.push(Output {
                    name: output.name.clone(),
                    data_type: output.data_type,
                });

                assert_ne!(node.id(), Uuid::nil());
                output_ids.insert(output_id, OutputAddr {
                    index: i as u32,
                    node_id: node.id(),
                });
            }
        }

        while let Some(connection) = connections.pop() {
            let mut node = nodes.pop().unwrap();

            for (input_index, output_id) in connection.inputs.iter().enumerate() {
                let input = &mut node.inputs[input_index];
                let output_addr = output_ids.get(output_id).unwrap();

                input.binding = Binding::from_output_binding(output_addr.node_id, output_addr.index)
            }

            graph.add_node(node);
        }

        assert!(graph.validate().is_ok());

        graph
    }


    pub fn get_output(&self) -> String {
        let mut cache = self.cache.borrow_mut();
        let result = cache.output_stream.join("\n");
        cache.output_stream.clear();
        result
    }

    pub fn get_all_functions(&self) -> Vec<&functions::Function> {
        self.funcs.values().map(|f| &f.info).collect()
    }
}


impl Drop for LuaInvoker {
    fn drop(&mut self) {
        self.funcs.clear();

        unsafe {
            let lua = transmute::<&'static Lua, Box<Lua>>(self.lua);
            drop(lua);
        }
    }
}

impl Invoker for LuaInvoker {
    fn invoke(&self, function_id: Uuid, context_id: Uuid, inputs: &InvokeArgs, outputs: &mut InvokeArgs) -> anyhow::Result<()> {
        self.lua.globals().set("context_id", context_id.to_string())?;

        let function_info = self.funcs
            .get(&function_id)
            .unwrap();

        let mut input_args: Variadic<mlua::Value> = Variadic::new();
        for (index, input_info) in function_info.info.inputs.iter().enumerate() {
            let input = inputs.get(index)
                .unwrap()
                .as_ref()
                .expect("input is required");
            assert_eq!(input_info.data_type, input.data_type());

            let invoke_value = to_lua_value(self.lua, &input)?;
            input_args.push(invoke_value);
        }

        let output_args: Variadic<mlua::Value> = function_info.lua_func.call(input_args)?;

        for (index, output_info) in function_info.info.outputs.iter().enumerate() {
            let output_arg: &mlua::Value = output_args
                .get(index)
                .unwrap();

            let output = data::Value::from(output_arg);
            assert_eq!(output_info.data_type, output.data_type());
            outputs[index] = Some(output);
        }

        self.lua.globals().set("context_id", mlua::Value::Nil)?;

        Ok(())
    }
}

fn to_lua_value<'lua>(lua: &'lua Lua, value: &data::Value) -> anyhow::Result<mlua::Value<'lua>> {
    match value {
        data::Value::Null => { Ok(mlua::Value::Nil) }
        data::Value::Float(v) => { Ok(mlua::Value::Number(*v as mlua::Number)) }
        data::Value::Int(v) => { Ok(mlua::Value::Integer(*v as mlua::Integer)) }
        data::Value::Bool(v) => { Ok(mlua::Value::Boolean(*v)) }
        data::Value::String(v) => {
            let lua_string = lua.create_string(v)?;
            Ok(mlua::Value::String(lua_string))
        }
    }
}

impl From<&mlua::Value<'_>> for data::Value {
    fn from(value: &mlua::Value) -> Self {
        match value {
            mlua::Value::Nil => { data::Value::Null }
            mlua::Value::Boolean(v) => { (*v).into() }
            mlua::Value::Integer(v) => { (*v).into() }
            mlua::Value::Number(v) => { (*v).into() }
            mlua::Value::String(v) => { v.to_str().unwrap().into() }
            _ => { panic!("not supported") }
        }
    }
}
