pub(crate) mod buffer2;
pub(crate) mod color;
pub(crate) mod color_format;
pub(crate) mod error;
#[cfg(test)]
pub(crate) mod image_diff;
#[cfg(any(test, feature = "internals"))]
#[allow(dead_code)] // Some helpers only used by feature-gated test modules.
pub(crate) mod internals;
