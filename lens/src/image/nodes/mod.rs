//! Assembly for the standard image node library.

mod io;
mod processing;

use imaginarium::BlendMode;
use scenarium::{DataType, Library, TypeEntry, TypeId};

use crate::image::IMAGE_TYPE_ID;
use crate::image::codec::image_type_entry;
use crate::image::format::{CONVERSION_FORMAT_TYPE_ID, ConversionFormat};

const BLENDMODE_TYPE_ID: TypeId = TypeId::literal("54d531cf-d353-4e30-8ea7-8823a9b5305f");
const BLENDMODE_DATATYPE: DataType = DataType::Enum(BLENDMODE_TYPE_ID);

pub fn image_library() -> Library {
    let mut library = Library::default();
    io::register(&mut library);
    processing::register(&mut library);
    library.register_type(IMAGE_TYPE_ID, image_type_entry());
    library.register_type(
        BLENDMODE_TYPE_ID,
        TypeEntry::enum_of::<BlendMode>("BlendMode"),
    );
    library.register_type(
        CONVERSION_FORMAT_TYPE_ID,
        TypeEntry::enum_of::<ConversionFormat>("ConversionFormat"),
    );
    library
}

#[cfg(test)]
mod tests;
