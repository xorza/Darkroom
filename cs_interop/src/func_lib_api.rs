use std::ffi::c_void;
use prost::Message;
use crate::{get_context, FfiBuf, proto};

fn to_proto_id(id: &graph::function::FuncId) -> proto::Id {
    let (a, b) = id.as_u64_pair();

    proto::Id { a, b }
}

fn to_proto_behavior(behavior: graph::function::FuncBehavior) -> proto::FuncBehavior {
    match behavior {
        graph::function::FuncBehavior::Active => proto::FuncBehavior::Active,
        graph::function::FuncBehavior::Passive => proto::FuncBehavior::Passive,
    }
}

fn to_proto_func(func: &graph::function::Func) -> proto::Func {
    proto::Func {
        id: Some(to_proto_id(&func.id)),
        name: func.name.clone(),
        category: func.category.clone(),
        behavior: to_proto_behavior(func.behavior) as i32,
        is_output: func.is_output,
        inputs: vec![],
        outputs: vec![],
        events: vec![],
    }
}

#[no_mangle]
extern "C" fn get_funcs(ctx: *mut c_void) -> FfiBuf {
    let funcs = get_context(ctx)
        .func_lib
        .iter()
        .map(|(_func_id, func)| to_proto_func(func))
        .collect::<Vec<proto::Func>>();

    proto::FuncLibrary {
        funcs,
    }
        .encode_to_vec()
        .into()
}


#[cfg(test)]
mod tests {
    use crate::create_context;
    use crate::func_lib_api::get_funcs;

    #[test]
    fn test_get_funcs() {
        let ctx = create_context();
        let buf = get_funcs(ctx);
        drop(buf);
    }
}
