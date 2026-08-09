//! What a test file needs before it can assert anything.
//!
//! Test files carried 907 `use` lines between them, and the top of the list was the same handful
//! every time — `Size2us` in 74 files, `Vec2us` in 27, `Buffer2` in 24. This is the universal
//! set: geometry, image and buffer types, the RNG, the float assertions, and the fixture
//! builders. Domain-specific items stay out; a subsystem with its own `tests/mod.rs` puts them
//! there and its leaf files pick them up from it.
//!
//! This is the crate's one deliberate re-export module. It exists because a glob import is the
//! only way to collapse those 907 lines, and because unlike a production re-export it cannot
//! create a second path to an item that ships — `testing` is `#[cfg(test)]`.

pub(crate) use crate::io::image::image_dimensions::ImageDimensions;
pub(crate) use crate::io::image::linear::LinearImage;
pub(crate) use crate::math::size2us::Size2us;
pub(crate) use crate::math::vec2us::Vec2us;
pub(crate) use crate::testing::TestRng;
pub(crate) use crate::testing::assertions::{assert_close, assert_close_slice, is_close};
pub(crate) use crate::testing::images::{gray_image, rgb_image};
pub(crate) use crate::testing::synthetic::background_map;
pub(crate) use common::CancelToken;
pub(crate) use glam::{DVec2, Vec2};
pub(crate) use imaginarium::Buffer2;
