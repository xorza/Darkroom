use std::sync::LazyLock;

use graph::data::DataType;
use imaginarium::BlendMode;

pub static BLENDMODE_DATATYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::from_enum::<BlendMode>("54d531cf-d353-4e30-8ea7-8823a9b5305f", "BlendMode")
});
