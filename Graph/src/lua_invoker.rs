use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::transmute;
use std::rc::Rc;
use mlua::{Error, Function, Lua, Table, Value, Variadic};
use crate::data_type::DataType;
use crate::invoke::*;

#[derive(Clone)]
pub struct Argument {
    name: String,
    data_type: DataType,
    index: i64,
}

#[derive(Clone)]
pub struct FunctionInfo<'lua> {
    name: String,
    function: Function<'lua>,
    inputs: Vec<Argument>,
    outputs: Vec<Argument>,
}

struct Cache<'lua> {
    funcs: HashMap<String, FunctionInfo<'lua>>,
    output_stream: Vec<String>,
}

pub struct LuaInvoker {
    lua: &'static Lua,
    cache: Rc<RefCell<Cache<'static>>>,
}

impl LuaInvoker {
    pub fn new<'lua>() -> LuaInvoker {
        let lua = Box::new(Lua::new());
        let lua: &'static Lua = Box::leak(lua);

        let result = LuaInvoker {
            lua,
            cache: Rc::new(RefCell::new(Cache::new())),
        };

        return result;
    }

    pub fn load_file(&self, file: &str) {
        let script = std::fs::read_to_string(file).unwrap();
        self.load(&script);
    }
    //noinspection RsNonExhaustiveMatch
    pub fn load(&self, script: &str) {
        let cache = Rc::clone(&self.cache);
        let register_function = self.lua.create_function(
            move |_lua: &Lua, table: Table| {
                let function_info = FunctionInfo::new(&table);

                let mut cache = cache.borrow_mut();
                cache.funcs.insert(function_info.name.clone(), function_info);
                Ok(())
            }
        ).unwrap();
        self.lua.globals().set("register_function", register_function).unwrap();

        let cache = Rc::clone(&self.cache);
        let print_function = self.lua.create_function(
            move |_lua: &Lua, args: Variadic<Value>| {
                let mut output = String::new();

                for arg in args {
                    match arg {
                        Value::Nil => { output += "Nil"; }
                        Value::Boolean(v) => { output += &v.to_string(); }
                        Value::LightUserData(_) => { output += "LightUserData"; }
                        Value::Integer(v) => { output += &v.to_string(); }
                        Value::Number(v) => { output += &v.to_string(); }
                        Value::String(v) => { output += v.to_str().unwrap(); }
                        Value::Table(_) => { output += "Table"; }
                        Value::Function(_) => { output += "Function"; }
                        Value::Thread(_) => { output += "Thread"; }
                        Value::UserData(_) => { output += "UserData"; }
                        Value::Error(err) => { output += &err.to_string() }
                    }
                }

                let mut cache = cache.borrow_mut();
                cache.output_stream.push(output);
                Ok(())
            }
        ).unwrap();
        self.lua.globals().set("print", print_function).unwrap();

        self.lua.load(script).exec().unwrap();
    }

    pub fn map_graph(&self) {
        self.substitute_functions();

        let graph_function: Function = self.lua.globals().get("graph").unwrap();
        graph_function.call::<_, ()>(()).unwrap();

        self.restore_functions();

        graph_function.call::<_, ()>(()).unwrap();
    }

    fn substitute_functions(&self) {
        let cache = self.cache.borrow();
        let functions = cache.funcs.values().cloned().collect::<Vec<FunctionInfo>>();
        drop(cache);

        let mut output_index: i64 = 0;

        for function_info in functions.iter() {
            let function_info_clone = function_info.clone();
            let cache = self.cache.clone();
            let new_function = self.lua.create_function(
                move |_lua: &Lua, inputs: Variadic<Value>| -> Result<Variadic<Value>, Error>  {
                    let mut cache = cache.borrow_mut();
                    let function_info = cache.funcs.get_mut(&function_info_clone.name).unwrap();

                    for (i, _arg) in function_info_clone.inputs.iter().enumerate() {
                        match inputs.get(i).unwrap() {
                            Value::Integer(output_index) => {
                                function_info.inputs[i].index = *output_index as i64;
                            }
                            _ => {}
                        }
                    }

                    let mut output_index = output_index;
                    let mut result: Variadic<Value> = Variadic::new();
                    for (i, _arg) in function_info_clone.outputs.iter().enumerate() {
                        result.push(Value::Integer(output_index));
                        function_info.outputs[i].index = output_index;
                        output_index += 1;
                    }
                    return Ok(result);
                }
            ).unwrap();

            self.lua.globals().set(function_info.name.clone(), new_function).unwrap();

            output_index += function_info.outputs.len() as i64;
        }
    }
    fn restore_functions(&self) {
        let cache = self.cache.borrow();
        let functions = cache.funcs.values().cloned().collect::<Vec<FunctionInfo>>();
        drop(cache);

        for function_info in functions.iter() {
            self.lua.globals().set(
                function_info.name.clone(),
                function_info.function.clone(),
            ).unwrap();
        }
    }

    pub fn get_output(&self) -> String {
        let mut cache = self.cache.borrow_mut();
        let result = cache.output_stream.join("\n");
        cache.output_stream.clear();
        return result;
    }
}

impl FunctionInfo<'_> {
    pub fn new<'lua>(table: &Table<'lua>) -> FunctionInfo<'lua> {
        let function: Function = table.get("func").unwrap();

        let mut function_info = FunctionInfo {
            name: table.get("name").unwrap(),
            function,
            inputs: Vec::new(),
            outputs: Vec::new(),
        };

        let inputs: Table = table.get("inputs").unwrap();
        for i in 1..=inputs.len().unwrap() {
            let input: Table = inputs.get(i).unwrap();
            let name: String = input.get(1).unwrap();
            let data_type_name: String = input.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            function_info.inputs.push(Argument { name, data_type, index: 0 });
        }

        let outputs: Table = table.get("outputs").unwrap();
        for i in 1..=outputs.len().unwrap() {
            let output: Table = outputs.get(i).unwrap();
            let name: String = output.get(1).unwrap();
            let data_type_name: String = output.get(2).unwrap();
            let data_type = data_type_name.parse::<DataType>().unwrap();

            function_info.outputs.push(Argument { name, data_type, index: 0 });
        }

        return function_info;
    }
}

impl Drop for LuaInvoker {
    fn drop(&mut self) {
        let mut cache = self.cache.borrow_mut();
        cache.funcs.clear();

        unsafe {
            let lua = transmute::<&'static Lua, Box<Lua>>(self.lua);
            drop(lua);
        }
    }
}

impl Invoker for LuaInvoker {
    fn call(&self, function_name: &str, context_id: u32, inputs: &Args, outputs: &mut Args) {
        self.lua.globals().set("context_id", context_id).unwrap();

        let cache = self.cache.borrow_mut();
        let function_info = cache.funcs.get(function_name).unwrap();
        let function: &Function = &function_info.function;

        let input_args: Variadic<i32> = Variadic::from_iter(inputs.iter().cloned());
        let output_args: Variadic<i32> = function.call(input_args).unwrap();

        for (i, output) in output_args.iter().enumerate() {
            outputs[i] = *output;
        }

        self.lua.globals().set("context_id", Value::Nil).unwrap();
    }

    fn finish(&self) {}
}

impl Cache<'_> {
    pub fn new<'lua>() -> Cache<'lua> {
        Cache {
            funcs: HashMap::new(),
            output_stream: Vec::new(),
        }
    }
}