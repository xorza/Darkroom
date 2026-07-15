use std::sync::LazyLock;

use scenarium::{DataType, TypeId};

pub(crate) static BLENDMODE_TYPE_ID: LazyLock<TypeId> =
    LazyLock::new(|| "54d531cf-d353-4e30-8ea7-8823a9b5305f".into());

pub(crate) static BLENDMODE_DATATYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Enum(*BLENDMODE_TYPE_ID));
