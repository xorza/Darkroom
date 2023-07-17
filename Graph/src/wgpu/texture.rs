use std::str::FromStr;

use once_cell::sync::Lazy;

use crate::data::{DataType, TypeId};

pub static TEXTURE_DATA_TYPE: Lazy<DataType> = Lazy::new(||
    DataType::Custom {
        type_id: TypeId::from_str("4dea9e08-6ba2-4800-b931-a2d26d14b2b7").unwrap(),
        type_name: "Image".to_string(),
    }
);
