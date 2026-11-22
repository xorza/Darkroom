//! Assembly for the standard image node library.

mod error;
mod io;
mod processing;

use imaginarium::{BlendMode, Clamp};
use scenarium::{DataType, EnumId, EnumType, LibraryBuilder};

use crate::image::IMAGE_TYPE_ID;
use crate::image::codec::image_type;
use crate::image::format::{CONVERSION_FORMAT_TYPE_ID, ConversionFormat};

const BLENDMODE_TYPE_ID: EnumId = EnumId::literal("54d531cf-d353-4e30-8ea7-8823a9b5305f");
const BLENDMODE_DATATYPE: DataType = DataType::Enum(BLENDMODE_TYPE_ID);
const CLAMP_TYPE_ID: EnumId = EnumId::literal("3360655d-16af-47d8-a1bb-7ef0b3f0ddf9");
const CLAMP_DATATYPE: DataType = DataType::Enum(CLAMP_TYPE_ID);

pub fn image_library() -> LibraryBuilder {
    let mut library = LibraryBuilder::default();
    io::register(&mut library);
    processing::register(&mut library);
    library.add_custom_type(IMAGE_TYPE_ID, image_type());
    library.add_enum(
        BLENDMODE_TYPE_ID,
        EnumType::new("Blend Mode", BlendMode::ALL.map(BlendMode::name)),
    );
    library.add_enum(
        CLAMP_TYPE_ID,
        EnumType::new("Clamp", Clamp::ALL.map(Clamp::name)),
    );
    library.add_enum(
        CONVERSION_FORMAT_TYPE_ID,
        EnumType::new("Conversion Format", ConversionFormat::NAMES),
    );
    library
}

#[cfg(test)]
mod tests;
