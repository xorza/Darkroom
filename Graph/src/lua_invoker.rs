use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::transmute;
use std::rc::Rc;
use mlua::{Error, Function, Lua, Table, Variadic};
use crate::data_type::DataType;
use crate::graph::{Binding, Graph, Input, Node, Output};
use crate::invoke;
use crate::invoke::{Args, Invoker};

#[derive(Clone)]
pub struct Argument {
    name: String,
    data_type: DataType,
}

#[derive(Clone)]
pub struct FunctionInfo {
    name: String,
    inputs: Vec<Argument>,
    outputs: Vec<Argument>,
}

struct Cache {
    output_stream: Vec<String>,
}

struct LuaFuncInfo<'lua> {
    info: FunctionInfo,
    lua_func: Function<'lua>,
}

#[derive(Clone)]
struct FuncConnections {
    name: String,
    inputs: Vec<u32>,
    outputs: Vec<u32>,
}

pub struct LuaInvoker<'lua> {
    lua: &'static Lua,
    cache: Rc<RefCell<Cache>>,
    funcs: HashMap<String, LuaFuncInfo<'lua>>,
    connections: Rc<RefCell<Vec<FuncConnections>>>,
}

impl LuaInvoker<'_> {
    pub fn new<'lua>() -> LuaInvoker<'lua> {
        let lua = Box::new(Lua::new());
        let lua: &'static Lua = Box::leak(lua);

        let result = LuaInvoker {
            lua,
            cache: Rc::new(RefCell::new(Cache::new())),
            funcs: HashMap::new(),
            connections: Rc::new(RefCell::new(Vec::new())),
        };

        return result;
    }

    pub fn load_file(&mut self, file: &str) {
        let script = std::fs::read_to_string(file).unwrap();
        self.load(&script);
    }
    //noinspection RsNonExhaustiveMatch
    pub fn load(&mut self, script: &str) {
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
        self.lua.globals().set("print", print_function).unwrap();

        self.lua.load(script).exec().unwrap();

        self.read_function_info();
    }
    fn read_function_info(&mut self) {
        let functions_table: Table = self.lua.globals().get("functions").unwrap();
        while let Ok(function_table) = functions_table.pop() {
            let function_info = FunctionInfo::new(&function_table);
            let function: Function = function_table.get("func").unwrap();
            self.funcs.insert(function_info.name.clone(),
                              LuaFuncInfo {
                                  info: function_info,
                                  lua_func: function,
                              },
            );
        }
    }

    pub fn map_graph(&self) -> Graph {
        self.substitute_functions();

        let graph_function: Function = self.lua.globals().get("graph").unwrap();
        graph_function.call::<_, ()>(()).unwrap();

        self.restore_functions();

        graph_function.call::<_, ()>(()).unwrap();

        self.create_graph()
    }
    fn substitute_functions(&self) {
        let functions = self.functions_info();

        let mut output_index: u32 = 0;

        for lua_func_info in functions {
            let function_info_clone = lua_func_info.clone();
            let connections = self.connections.clone();

            let new_function = self.lua.create_function(
                move |_lua: &Lua, mut inputs: Variadic<mlua::Value>| -> Result<Variadic<mlua::Value>, Error>  {
                    let mut connection = FuncConnections {
                        name: function_info_clone.name.clone(),
                        inputs: Vec::new(),
                        outputs: Vec::new(),
                    };

                    while let Some(input) = inputs.pop() {
                        match input {
                            mlua::Value::Integer(output_index) => {
                                connection.inputs.push(output_index as u32);
                            }
                            _ => { () }
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

                    return Ok(result);
                }
            ).unwrap();

            self.lua.globals().set(lua_func_info.name.clone(), new_function).unwrap();

            output_index += lua_func_info.outputs.len() as u32;
        }
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
    fn create_graph(&self) -> Graph {
        let mut graph = Graph::new();
        let mut connections_ref = self.connections.borrow_mut();
        let mut connections = connections_ref.clone();
        connections_ref.clear();
        drop(connections_ref);


        struct OutputAddr {
            index: u32,
            node_id: u32,
        }
        let mut output_ids: HashMap<u32, OutputAddr> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::new();

        for connection in connections.iter_mut() {
            let function = &self.funcs.get(&connection.name).unwrap().info;
            let mut node = Node::new();
            node.name = function.name.clone();
            graph.add_node(&mut node); //to get node.id


            for (i, _input_id) in connection.inputs.iter().enumerate() {
                let input = function.inputs.get(i).unwrap();
                node.inputs.push(Input {
                    name: input.name.clone(),
                    data_type: input.data_type,
                    is_required: true,
                    binding: None,
                });
            }
            for (i, output_id) in connection.outputs.iter().cloned().enumerate() {
                let output = function.outputs.get(i).unwrap();
                node.outputs.push(Output {
                    name: output.name.clone(),
                    data_type: output.data_type,
                });

                assert_ne!(node.id(), 0);
                output_ids.insert(output_id, OutputAddr {
                    index: i as u32,
                    node_id: node.id(),
                });
            }

            graph.add_node(&mut node); //to update its state with the same id
            nodes.push(node);
        }

        for (i, connection) in connections.iter().enumerate() {
            let node = &mut nodes[i];

            for (i, output_id) in connection.inputs.iter().enumerate() {
                let input = &mut node.inputs[i];
                let output_addr = output_ids.get(output_id).unwrap();
                let binding = Binding::new(output_addr.node_id, output_addr.index);

                input.binding = Some(binding);
            }

            graph.add_node(node); //to update its state with the same id
        }

        drop(nodes);
        drop(output_ids);
        drop(connections);

        assert!(graph.validate());

        return graph;
    }

    pub fn get_output(&self) -> String {
        let mut cache = self.cache.borrow_mut();
        let result = cache.output_stream.join("\n");
        cache.output_stream.clear();
        return result;
    }
    pub fn functions_info(&self) -> impl Iterator<Item=&FunctionInfo> {
        self.funcs.values().map(|f| &f.info)
    }
}

impl FunctionInfo {
    fn new(table: &Table) -> FunctionInfo {
        let mut function_info = FunctionInfo {
            name: table.get("name").unwrap(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        };

        let inputs: Table = table.get("inputs").unwrap();
        for i in 1..=inputs.len().unwrap() {
            let input: Table = inputs.get(i).unwrap();
            let name: String = input.get(1).unwrap();
            let data_type_name: String = input.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            function_info.inputs.push(Argument { name, data_type});
        }

        let outputs: Table = table.get("outputs").unwrap();
        for i in 1..=outputs.len().unwrap() {
            let output: Table = outputs.get(i).unwrap();
            let name: String = output.get(1).unwrap();
            let data_type_name: String = output.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            function_info.outputs.push(Argument { name, data_type });
        }

        return function_info;
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn inputs(&self) -> &Vec<Argument> {
        &self.inputs
    }
    pub fn outputs(&self) -> &Vec<Argument> {
        &self.outputs
    }
}

impl Drop for LuaInvoker<'_> {
    fn drop(&mut self) {
        self.funcs.clear();

        unsafe {
            let lua = transmute::<&'static Lua, Box<Lua>>(self.lua);
            drop(lua);
        }
    }
}

impl Invoker for LuaInvoker<'_> {
    fn start(&self) {}
    fn call(&self, function_name: &str, context_id: u32, inputs: &Args, outputs: &mut Args) {
        self.lua.globals().set("context_id", context_id).unwrap();

        let function_info = self.funcs.get(function_name).unwrap();

        let mut input_args: Variadic<mlua::Value> = Variadic::new();
        for (i, input) in function_info.info.inputs.iter().enumerate() {
            assert_eq!(input.data_type, inputs[i].data_type());
            input_args.push(from_lua_value(&inputs[i], self.lua));
        }

        let output_args: Variadic<mlua::Value> = function_info.lua_func.call(input_args).unwrap();

        for (i, output) in function_info.info.outputs.iter().enumerate() {
            let output_arg = output_args.get(i).unwrap();
            outputs[i] = from_invoke_value(output_arg);
            assert_eq!(output.data_type, outputs[i].data_type());
        }

        self.lua.globals().set("context_id", mlua::Value::Nil).unwrap();
    }
    fn finish(&self) {}
}

fn from_lua_value<'lua>(value: &'lua invoke::Value, lua: &'lua Lua) -> mlua::Value<'lua> {
    match value {
        invoke::Value::Null => { mlua::Value::Nil }
        invoke::Value::Float(v) => { mlua::Value::Number(*v) }
        invoke::Value::Int(v) => { mlua::Value::Integer(*v) }
        invoke::Value::Bool(v) => { mlua::Value::Boolean(*v) }
        invoke::Value::String(v) => {
            let lua_string = lua.create_string(v).unwrap();
            mlua::Value::String(lua_string)
        }
    }
}

fn from_invoke_value(value: &mlua::Value) -> invoke::Value {
    match value {
        mlua::Value::Nil => { invoke::Value::Null }
        mlua::Value::Boolean(v) => { (*v).into() }
        mlua::Value::Integer(v) => { (*v).into() }
        mlua::Value::Number(v) => { (*v).into() }
        mlua::Value::String(v) => { v.to_str().unwrap().into() }
        _ => { panic!("not supported") }
    }
}


impl Cache {
    pub fn new() -> Cache {
        Cache {
            output_stream: Vec::new(),
        }
    }
}
