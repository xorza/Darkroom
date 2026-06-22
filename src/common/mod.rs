pub(crate) mod buffer2;
pub(crate) mod color;
pub(crate) mod color_format;
pub(crate) mod conversion;
pub(crate) mod error;
#[cfg(test)]
pub(crate) mod image_diff;
#[cfg(test)]
#[allow(dead_code)] // Some helpers only used by feature-gated test modules.
pub(crate) mod test_utils;
