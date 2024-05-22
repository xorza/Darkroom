use graph::function::FuncLib;

use crate::{FfiBuf, FfiStr, FfiStrVec, Id};

#[repr(C)]
#[derive(Debug)]
pub enum FuncBehavior {
    Active,
    Passive,
}

#[repr(C)]
#[derive(Debug)]
struct Func {
    id: Id,
    name: FfiStr,
    category: FfiStr,
    behaviour: FuncBehavior,
    is_output: bool,
    inputs: FfiBuf,
    outputs: FfiBuf,
    events: FfiStrVec,
}

#[no_mangle]
extern "C" fn get_funcs() -> FfiBuf {
    let yaml = include_str!("../../test_resources/test_funcs.yml");
    let func_lib = FuncLib::from_yaml(yaml).unwrap();

    func_lib
        .iter()
        .map(|(_func_id, func)| Func::from(func))
        .collect::<Vec<Func>>()
        .into()
}

impl From<&graph::function::Func> for Func {
    fn from(func: &graph::function::Func) -> Self {
        // let events = func
        //     .events
        //     .iter()
        //     .map(|event| event.name.clone())
        //     .collect::<Vec<String>>();
        let events = FfiStrVec::from_iter(["event1".to_string(), "event2".to_string()]);

        Func {
            id: func.id.as_uuid().into(),
            name: func.name.clone().into(),
            category: func.category.clone().into(),
            behaviour: match func.behavior {
                graph::graph::FuncBehavior::Active => FuncBehavior::Active,
                graph::graph::FuncBehavior::Passive => FuncBehavior::Passive,
            },
            is_output: func.is_output,
            inputs: FfiBuf::default(),
            outputs: FfiBuf::default(),
            events,
        }
    }
}

#[no_mangle]
extern "C" fn dummy2(_a: Func) {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_get_funcs() {
        let funcs = super::get_funcs();
        assert!(!funcs.is_null());
    }
}
