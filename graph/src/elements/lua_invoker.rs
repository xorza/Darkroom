use std::cell::RefCell;
use std::fmt::{Debug, Formatter};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use hashbrown::HashMap;
use parking_lot::Mutex;

use common::output_stream::OutputStream;

use crate::{data, function};
use crate::data::DataType;
use crate::data::DynamicValue;
use crate::function::{Func, FuncId, FuncLib};
use crate::graph::{Binding, Graph, Input, Node, NodeId};
use crate::invoke::{InvokeArgs, InvokeCache, Invoker};

#[derive(Clone)]
struct FuncConnections {
    name: String,
    inputs: Vec<u32>,
    outputs: Vec<u32>,
}

#[derive(Debug)]
pub struct LuaInvoker {
    inner: Arc<Mutex<LuaInner>>,
    func_lib: FuncLib,
}


pub(crate) struct LuaInner {
    lua: &'static mlua::Lua,
    output_stream: Arc<Mutex<Option<OutputStream>>>,
    funcs: HashMap<FuncId, mlua::Function<'static>>,
    func_lib: FuncLib,
}


unsafe impl Send for LuaInvoker {}

unsafe impl Sync for LuaInvoker {}

impl Invoker for LuaInvoker {
    fn take_func_lib(&mut self) -> FuncLib {
        std::mem::take(&mut self.func_lib)
    }

    fn invoke(&self, function_id: FuncId, cache: &mut InvokeCache, inputs: &mut InvokeArgs, outputs: &mut InvokeArgs) -> anyhow::Result<()> {
        self.inner.lock().invoke(function_id, cache, inputs, outputs)
    }
}

impl Default for LuaInvoker {
    fn default() -> Self {
        LuaInvoker {
            inner: Arc::new(Mutex::new(LuaInner::default())),
            func_lib: FuncLib::default(),
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
        let mut inner = self.inner.lock();
        inner.load(script)?;
        self.func_lib = inner.get_func_lib().clone();

        Ok(())
    }

    pub fn use_output_stream(&mut self, output_stream: &OutputStream) {
        self.inner.lock().use_output_stream(output_stream);
    }

    pub fn map_graph(&self) -> anyhow::Result<Graph> {
        self.inner.lock().map_graph()
    }
}

impl Default for LuaInner {
    fn default() -> Self {
        let lua = Box::new(mlua::Lua::new());
        let lua: &'static mlua::Lua = Box::leak(lua);

        LuaInner {
            lua,
            output_stream: Arc::new(Mutex::new(None)),
            funcs: HashMap::new(),
            func_lib: FuncLib::default(),
        }
    }
}

impl LuaInner {
    pub fn load_file(&mut self, file: &str) -> anyhow::Result<()> {
        let script = std::fs::read_to_string(file)?;
        self.load(&script)?;

        Ok(())
    }

    pub fn load(&mut self, script: &str) -> anyhow::Result<()> {
        let output_stream = self.output_stream.clone();

        let print_function = self
            .lua
            .create_function(move |_lua: &mlua::Lua, args: mlua::Variadic<mlua::Value>| {
                let mut output = String::new();

                for arg in args {
                    #[allow(unreachable_patterns)]
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
                            output += v.to_str().unwrap();
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

                let _ = output_stream.lock().as_mut().is_some_and(|stream| {
                    stream.write(output);
                    true
                });
                Ok(())
            })
            .unwrap();
        self.lua.globals().set("print", print_function)?;

        self.lua.load(script).exec()?;

        self.read_function_info()?;


        Ok(())
    }

    fn read_function_info(&mut self) -> anyhow::Result<()> {
        let functions_table: mlua::Table = self.lua.globals().get("functions")?;
        while let Ok(function_table) = functions_table.pop() {
            let func = Self::function_from_table(&function_table)?;
            let lua_func: mlua::Function = self.lua.globals().get(func.name.as_str())?;

            self.funcs.insert(
                func.id,
                lua_func,
            );

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
            behavior: Default::default(),
            is_output: false,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
        };

        let inputs: mlua::Table = table.get("inputs")?;
        for i in 1..=inputs.len()? {
            let input: mlua::Table = inputs.get(i).unwrap();
            let name: String = input.get(1).unwrap();
            let data_type_name: String = input.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            let mut default_value: Option<data::StaticValue> = None;
            if input.len()? > 2 {
                default_value = None;
            }

            function_info.inputs.push(function::InputInfo {
                name,
                is_required: true,
                data_type,
                default_value,
                variants: vec![],
            });
        }

        let outputs: mlua::Table = table.get("outputs")?;
        for i in 1..=outputs.len()? {
            let output: mlua::Table = outputs.get(i).unwrap();
            let name: String = output.get(1).unwrap();
            let data_type_name: String = output.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            function_info
                .outputs
                .push(function::OutputInfo { name, data_type });
        }

        Ok(function_info)
    }

    pub fn map_graph(&self) -> anyhow::Result<Graph> {
        let connections = self.substitute_functions();

        self.restore_functions();

        let graph_function: mlua::Function = self.lua.globals().get("graph").unwrap();
        graph_function.call::<_, ()>(()).unwrap();

        let graph = self.create_graph(connections);

        Ok(graph)
    }

    fn substitute_functions(&self) -> Vec<FuncConnections> {
        let connections: Rc<RefCell<Vec<FuncConnections>>> = Rc::new(RefCell::new(Vec::new()));

        let functions = &self.func_lib;

        let mut output_index: u32 = 0;

        for (_id, func) in functions.iter() {
            let func_clone = func.clone();
            let connections = connections.clone();

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

                        let mut connections = connections.borrow_mut();
                        connections.push(connection);

                        Ok(result)
                    },
                )
                .unwrap();

            self.lua
                .globals()
                .set(func.name.clone(), new_function)
                .unwrap();

            output_index += func.outputs.len() as u32;
        }

        let graph_function: mlua::Function = self.lua.globals().get("graph").unwrap();
        graph_function.call::<_, ()>(()).unwrap();

        connections.take()
    }
    fn restore_functions(&self) {
        for (&id, lua_func) in self.funcs.iter() {
            let func = self.func_lib.func_by_id(id).unwrap();
            self.lua
                .globals()
                .set(
                    func.name.clone(),
                    lua_func.clone(),
                )
                .unwrap();
        }
    }
    fn create_graph(&self, mut connections: Vec<FuncConnections>) -> Graph {
        let mut graph = Graph::default();

        struct OutputAddr {
            index: u32,
            node_id: NodeId,
        }
        let mut output_ids: HashMap<u32, OutputAddr> = HashMap::new();
        let mut nodes: Vec<Node> = Vec::new();

        for connection in connections.iter() {
            let func = self.func_lib.func_by_name(&connection.name).unwrap();

            nodes.push(Node::default());
            let node = nodes.last_mut().unwrap();
            node.name = func.name.clone();

            for (_idx, _input_id) in connection.inputs.iter().enumerate() {
                node.inputs.push(Input {
                    binding: Binding::None,
                    const_value: None,
                });
            }
            for (i, output_id) in connection.outputs.iter().cloned().enumerate() {
                assert!(!node.id.is_nil());
                output_ids.insert(
                    output_id,
                    OutputAddr {
                        index: i as u32,
                        node_id: node.id,
                    },
                );
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

    pub(crate) fn use_output_stream(&mut self, output_stream: &OutputStream) {
        self.output_stream.lock().replace(output_stream.clone());
    }

    pub(crate) fn get_func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn invoke(
        &self,
        func_id: FuncId,
        _cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        // self.lua.globals().set("context_id", context_id.to_string())?;

        let lua_func = self.funcs.get(&func_id).unwrap();
        let func = self.func_lib.func_by_id(func_id).unwrap();

        let mut input_args: mlua::Variadic<mlua::Value> = mlua::Variadic::new();
        for (index, input_info) in func.inputs.iter().enumerate() {
            let input = &inputs[index];
            assert_eq!(input_info.data_type, *input.data_type());

            let invoke_value = to_lua_value(self.lua, input)?;
            input_args.push(invoke_value);
        }

        let output_args: mlua::Variadic<mlua::Value> = lua_func.call(input_args)?;

        for (index, output_info) in func.outputs.iter().enumerate() {
            let output_arg: &mlua::Value = output_args.get(index).unwrap();

            let output = data::DynamicValue::from(output_arg);
            assert_eq!(output_info.data_type, *output.data_type());
            outputs[index] = output;
        }

        self.lua.globals().set("context_id", mlua::Value::Nil)?;

        Ok(())
    }
}

impl Drop for LuaInner {
    fn drop(&mut self) {
        self.funcs.clear();

        let _lua: Box<mlua::Lua> =
            unsafe { Box::from_raw((self.lua as *const mlua::Lua) as *mut mlua::Lua) };
    }
}

impl Debug for LuaInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "LuaInvoker")
    }
}

fn to_lua_value<'lua>(
    lua: &'lua mlua::Lua,
    value: &DynamicValue,
) -> anyhow::Result<mlua::Value<'lua>> {
    match value {
        data::DynamicValue::Null => Ok(mlua::Value::Nil),
        data::DynamicValue::Float(v) => Ok(mlua::Value::Number(*v as mlua::Number)),
        data::DynamicValue::Int(v) => Ok(mlua::Value::Integer(*v as mlua::Integer)),
        data::DynamicValue::Bool(v) => Ok(mlua::Value::Boolean(*v)),
        data::DynamicValue::String(v) => {
            let lua_string = lua.create_string(v)?;
            Ok(mlua::Value::String(lua_string))
        }
        _ => panic!("not supported"),
    }
}

impl From<&mlua::Value<'_>> for DynamicValue {
    fn from(value: &mlua::Value) -> Self {
        match value {
            mlua::Value::Nil => DynamicValue::Null,
            mlua::Value::Boolean(v) => (*v).into(),
            mlua::Value::Integer(v) => (*v).into(),
            mlua::Value::Number(v) => (*v).into(),
            mlua::Value::String(v) => v.to_str().unwrap().into(),
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

    use crate::compute::ArgSet;
    use crate::elements::lua_invoker::LuaInvoker;
    use crate::function::FuncId;
    use crate::invoke::{InvokeCache, Invoker};

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

        let funcs = invoker.take_func_lib();
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
}