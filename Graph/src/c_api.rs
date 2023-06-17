use std::ffi::{c_char, CString};

use uuid::Uuid;

use crate::data_type::DataType;
use crate::functions::{Arg, Function, Functions};
use crate::graph::Graph;

static mut GRAPH: *mut Graph = std::ptr::null_mut();
static mut FUNCTIONS: *mut Functions = std::ptr::null_mut();
static mut IS_INIT: bool = false;

#[no_mangle]
pub unsafe extern "C" fn c_graph_init() {
    assert!(!IS_INIT);

    GRAPH = Box::into_raw(Box::default());
    FUNCTIONS = Box::into_raw(Box::default());
    IS_INIT = true;


    let functions = FUNCTIONS.as_mut().unwrap();

    let mut func = Function::new(Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap());
    func.name = "Add".to_string();
    func.inputs.push(Arg {
        name: "a".to_string(),
        data_type: DataType::Int,
    });
    func.inputs.push(Arg {
        name: "b".to_string(),
        data_type: DataType::Int,
    });
    func.outputs.push(Arg {
        name: "sum".to_string(),
        data_type: DataType::Int,
    });
    functions.add_function(func);
}

#[no_mangle]
pub unsafe extern "C" fn c_graph_deinit() {
    assert!(IS_INIT);

    IS_INIT = false;
    let _ = Box::from_raw(GRAPH);
    let _ = Box::from_raw(FUNCTIONS);
}

#[repr(C)]
pub struct CArg {
    name: *const c_char,
    data_type: DataType,
}

#[repr(C)]
pub struct CFunctionInfo {
    self_id: [u8; 16],
    name: *const c_char,
    input_count: i32,
    inputs: *const CArg,
    output_count: i32,
    outputs: *const CArg,
}

#[repr(C)]
pub struct CFunctionInfoArray {
    count: i32,
    functions: *const CFunctionInfo,
}


#[no_mangle]
pub unsafe extern "C" fn c_graph_get_functions() -> *mut CFunctionInfoArray {
    assert!(IS_INIT);

    let c_functions =
        FUNCTIONS.as_ref().unwrap()
            .functions()
            .iter()
            .map(|func| {
                let c_inputs = func.inputs.iter().map(|arg| CArg {
                    name: CString::new(arg.name.clone()).unwrap().into_raw(),
                    data_type: arg.data_type,
                }).collect::<Vec<CArg>>();
                let c_outputs = func.outputs.iter().map(|arg| CArg {
                    name: CString::new(arg.name.clone()).unwrap().into_raw(),
                    data_type: arg.data_type,
                }).collect::<Vec<CArg>>();

                CFunctionInfo {
                    self_id: func.id().as_bytes().clone(),
                    name: CString::new(func.name.clone()).unwrap().into_raw(),
                    input_count: func.inputs.len() as i32,
                    inputs: Box::into_raw(c_inputs.into_boxed_slice()) as *mut CArg,
                    output_count: func.outputs.len() as i32,
                    outputs: Box::into_raw(c_outputs.into_boxed_slice()) as *mut CArg,
                }
            })
            .collect::<Vec<CFunctionInfo>>();

    let func_array: CFunctionInfoArray;
    if c_functions.is_empty() {
        func_array = CFunctionInfoArray {
            count: 0,
            functions: std::ptr::null_mut(),
        };
    } else {
        func_array = CFunctionInfoArray {
            count: c_functions.len() as i32,
            functions: Box::into_raw(c_functions.into_boxed_slice()) as *mut CFunctionInfo,
        };
    }

    Box::into_raw(Box::new(func_array))
}

#[no_mangle]
pub unsafe extern "C" fn c_graph_free_functions(data: *mut CFunctionInfoArray) {
    assert_ne!(data, std::ptr::null_mut());

    let _ = Box::from_raw(data);
}


#[cfg(test)]
mod test {
    use crate::c_api::*;

    #[test]
    fn test_graph_new() {
        unsafe {
            c_graph_init();

            let functions = c_graph_get_functions();
            c_graph_free_functions(functions);

            c_graph_deinit();
        }
    }
}

impl Drop for CFunctionInfo {
    fn drop(&mut self) {
        unsafe {
            assert!(!self.name.is_null());
            assert!(!self.inputs.is_null());
            assert!(!self.outputs.is_null());

            let _ = CString::from_raw(self.name as *mut c_char);
            let _ = Box::from_raw(std::slice::from_raw_parts_mut(self.inputs as *mut CArg, self.input_count as usize));
            let _ = Box::from_raw(std::slice::from_raw_parts_mut(self.outputs as *mut CArg, self.output_count as usize));
        }
    }
}

impl Drop for CFunctionInfoArray {
    fn drop(&mut self) {
        if !self.functions.is_null() {
            unsafe {
                let _ = Box::from_raw(std::slice::from_raw_parts_mut(self.functions as *mut CFunctionInfo, self.count as usize));
            }
        }
    }
}
