//! Image data model and on-disk ingestion: the in-memory [`astro_image::LinearImage`]
//! container plus all decoding (FITS, camera RAW, standard formats) into it.

pub(crate) mod astro_image;
pub(crate) mod raw;
