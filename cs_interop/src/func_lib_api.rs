use std::ffi::c_void;
use graph::function::Func;
use crate::{get_context, FfiBuf};


#[no_mangle]
extern "C" fn get_func_lib(ctx: *mut c_void) -> FfiBuf {
    let funcs = get_context(ctx)
        .func_lib
        .iter()
        .collect::<Vec<&Func>>();

    serde_json::to_string(&funcs).unwrap().into()
}


#[cfg(test)]
mod tests {
    use crate::create_context;
    use crate::func_lib_api::get_func_lib;

    #[test]
    fn test_get_funcs() {
        let ctx = create_context();
        let buf = get_func_lib(ctx);
        drop(buf);
    }
}
