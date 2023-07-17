use std::str::FromStr;

use once_cell::sync::Lazy;

use imaginarium::image::ImageDesc;
use imaginarium::math::Transform2D;

use crate::data::{DataType, TypeId};

pub static IMAGE_DATA_TYPE: Lazy<DataType> = Lazy::new(||
    DataType::Custom {
        type_id: TypeId::from_str("9b21b096-caa3-4443-ad43-bf425fcc975e").unwrap(),
        type_name: "Image".to_string(),
    }
);

struct Image {
    transform: Transform2D,
    desc: ImageDesc,
    image: imaginarium::image::Image,
}


