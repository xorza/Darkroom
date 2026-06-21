//! The `astro` domain — `lumos`-backed nodes and types. This module is also the
//! home of [`AstroFrame`] (a [`lumos::AstroImage`] wrapped as a scenarium
//! [`CustomValue`] so astronomical frames flow on graph wires).
//!
//! `AstroFrame` mirrors [`crate::image::Image`], but the preview is generated on
//! the CPU (lumos has no GPU backend): a downscaled RGBA_U8 thumbnail is
//! computed synchronously in [`AstroFrame::gen_preview`] and parked in a
//! [`Slot`] for the editor to pick up via [`AstroFrame::take_preview`].

mod configs;
pub(crate) mod funclib;
mod masters;
mod presets;

use std::any::Any;
use std::sync::{Arc, LazyLock};

use common::{Buffer2, Slot};
use imaginarium::{ColorFormat, Image as RawImage, ImageDesc};
use lumos::AstroImage;
use scenarium::context::ContextManager;
use scenarium::data::{CustomValue, DataType, PendingPreview, TypeDef};

pub static ASTRO_FRAME_TYPE_DEF: LazyLock<Arc<TypeDef>> = LazyLock::new(|| {
    Arc::new(TypeDef {
        type_id: "8b8cdfd0-e98a-4067-870c-9b078d7f34d1".into(),
        display_name: "AstroFrame".to_string(),
    })
});

pub static ASTRO_FRAME_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Custom(ASTRO_FRAME_TYPE_DEF.clone()));

/// Longest edge (px) of the generated preview thumbnail.
const PREVIEW_SIZE: usize = 256;

/// A `lumos::AstroImage` carried through the node graph, with a cached
/// preview thumbnail.
pub struct AstroFrame {
    pub image: AstroImage,
    preview: Slot<RawImage>,
}

impl std::fmt::Debug for AstroFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AstroFrame")
            .field("dimensions", &self.image.dimensions())
            .finish_non_exhaustive()
    }
}

impl AstroFrame {
    pub fn new(image: AstroImage) -> Self {
        Self {
            image,
            preview: Slot::default(),
        }
    }

    pub fn take_preview(&self) -> Option<RawImage> {
        self.preview.take()
    }
}

impl CustomValue for AstroFrame {
    fn type_def(&self) -> Arc<TypeDef> {
        ASTRO_FRAME_TYPE_DEF.clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn gen_preview(&self, _ctx_manager: &mut ContextManager) -> Option<Box<dyn PendingPreview>> {
        // CPU-only: compute synchronously and park the result. Returning
        // `None` means "no async work to await" — the editor reads the
        // parked thumbnail via `take_preview` off the same shared value.
        if let Some(thumb) = render_thumbnail(&self.image) {
            self.preview.send(thumb);
        }
        None
    }
}

impl std::fmt::Display for AstroFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let d = self.image.dimensions();
        write!(f, "{}x{} ({} ch)", d.size.x, d.size.y, d.channels)
    }
}

impl From<AstroImage> for AstroFrame {
    fn from(image: AstroImage) -> Self {
        AstroFrame::new(image)
    }
}

/// Build a downscaled RGBA_U8 thumbnail straight from the planar frame.
/// No display stretch: pixels are quantized as-is, so an already-stretched
/// or standard image looks right while a *linear* FITS/RAW light previews
/// dark. `None` only for a degenerate (zero-area) image.
fn render_thumbnail(image: &AstroImage) -> Option<RawImage> {
    let (w, h, ch) = (image.width(), image.height(), image.channels());
    if w == 0 || h == 0 {
        return None;
    }
    let max_dim = w.max(h);
    let scale = if max_dim <= PREVIEW_SIZE {
        1.0
    } else {
        PREVIEW_SIZE as f32 / max_dim as f32
    };
    let tw = ((w as f32 * scale).round() as usize).max(1);
    let th = ((h as f32 * scale).round() as usize).max(1);

    // Read the planar channels at nearest-neighbour decimated coordinates and
    // quantize straight to RGBA_U8 — one pass over the *output* pixels, no
    // full-resolution intermediate. (Going via `imaginarium::Image` would
    // interleave the whole frame to `RGB_F32`, then convert, then still need a
    // CPU resize imaginarium doesn't have.) Grayscale replicates one plane
    // across RGB so the inner loop stays branchless.
    let planes: [&Buffer2<f32>; 3] = if ch == 1 {
        let l = image.channel(0);
        [l, l, l]
    } else {
        [image.channel(0), image.channel(1), image.channel(2)]
    };

    let mut rgba = vec![0u8; tw * th * 4];
    for y in 0..th {
        let sy = (y * h) / th;
        for x in 0..tw {
            let sx = (x * w) / tw;
            let o = (y * tw + x) * 4;
            rgba[o] = to_u8(*planes[0].get(sx, sy));
            rgba[o + 1] = to_u8(*planes[1].get(sx, sy));
            rgba[o + 2] = to_u8(*planes[2].get(sx, sy));
            rgba[o + 3] = 255;
        }
    }
    let desc = ImageDesc::new_with_stride(tw, th, ColorFormat::RGBA_U8);
    RawImage::new_with_data(desc, rgba).ok()
}

/// Quantize a linear `[0,1]` sample to 8-bit, clamping out-of-range values.
fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use lumos::ImageDimensions;

    use super::*;

    /// Interleaved RGB image with a per-pixel gradient.
    fn rgb_ramp(w: usize, h: usize) -> AstroImage {
        let ch = 3;
        let mut px = vec![0.0f32; w * h * ch];
        for i in 0..w * h {
            let t = i as f32 / (w * h) as f32;
            px[i * ch] = 0.4 * t;
            px[i * ch + 1] = 0.2 * t;
            px[i * ch + 2] = 0.1 * t;
        }
        AstroImage::from_pixels(ImageDimensions::new((w, h), ch), px)
    }

    #[test]
    fn thumbnail_downscales_to_preview_box_and_is_rgba8() {
        let img = rgb_ramp(512, 256);
        let thumb = render_thumbnail(&img).expect("thumbnail");
        assert_eq!(thumb.desc.color_format, ColorFormat::RGBA_U8);
        // Longest edge clamped to PREVIEW_SIZE, aspect preserved.
        assert_eq!(thumb.desc.width, 256);
        assert_eq!(thumb.desc.height, 128);
        let bytes = thumb.bytes();
        assert_eq!(bytes.len(), 256 * 128 * 4);
        // Alpha is fully opaque everywhere.
        assert!(bytes.iter().skip(3).step_by(4).all(|&a| a == 255));
        // The ramp's non-zero samples survive quantization.
        assert!(bytes.iter().any(|&b| b > 0));
    }

    #[test]
    fn rgb_channels_map_to_rgba_exactly() {
        // 2x1 RGB, no downscale: px0 = (0.0, 0.5, 1.0), px1 = (1.0, 0.0, 0.5).
        let px = vec![0.0, 0.5, 1.0, 1.0, 0.0, 0.5];
        let img = AstroImage::from_pixels(ImageDimensions::new((2, 1), 3), px);
        let thumb = render_thumbnail(&img).unwrap();
        assert_eq!((thumb.desc.width, thumb.desc.height), (2, 1));
        // Linear quantize (to_u8(0.5) = 128), channel order R,G,B,A=255.
        assert_eq!(
            thumb.bytes(),
            [0, 128, 255, 255, 255, 0, 128, 255].as_slice()
        );
    }

    #[test]
    fn small_image_is_not_upscaled() {
        let img = rgb_ramp(8, 4);
        let thumb = render_thumbnail(&img).unwrap();
        assert_eq!((thumb.desc.width, thumb.desc.height), (8, 4));
    }

    #[test]
    fn grayscale_thumbnail_replicates_across_rgb() {
        let (w, h) = (16, 8);
        let mut px = vec![0.0f32; w * h];
        for (i, p) in px.iter_mut().enumerate() {
            *p = i as f32 / (w * h) as f32;
        }
        let img = AstroImage::from_pixels(ImageDimensions::new((w, h), 1), px);
        let thumb = render_thumbnail(&img).unwrap();
        for px in thumb.bytes().chunks_exact(4) {
            assert_eq!(px[0], px[1], "r == g for grayscale");
            assert_eq!(px[1], px[2], "g == b for grayscale");
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn to_u8_clamps_out_of_range() {
        assert_eq!(to_u8(-1.0), 0);
        assert_eq!(to_u8(0.0), 0);
        assert_eq!(to_u8(0.5), 128);
        assert_eq!(to_u8(1.0), 255);
        assert_eq!(to_u8(2.0), 255);
    }

    #[test]
    fn preview_slot_fills_once_and_is_consumed() {
        let frame = AstroFrame::from(rgb_ramp(32, 16));
        assert!(frame.take_preview().is_none(), "empty before generation");
        if let Some(thumb) = render_thumbnail(&frame.image) {
            frame.preview.send(thumb);
        }
        assert!(frame.take_preview().is_some(), "filled after generation");
        assert!(frame.take_preview().is_none(), "take consumes the slot");
    }

    #[test]
    fn display_reports_dimensions() {
        let frame = AstroFrame::from(rgb_ramp(640, 480));
        assert_eq!(frame.to_string(), "640x480 (3 ch)");
    }
}
