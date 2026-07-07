use aperture::{Color, ColorU8, Image, ImageFit, ImageHandle, Shape, Ui};
use glam::Vec2;

use crate::gui::app::AppContext;
use crate::gui::canvas::outer_canvas_widget_id;

/// Target on-screen tile spacing range, as a multiple of the theme's
/// base spacing. A power-of-2 multiplier wraps the world spacing into
/// `[base, 2·base]` px so the dot field stays a stable density instead
/// of collapsing into noise (zoomed out) or drifting apart (zoomed in).
/// `MAX = 2·MIN` keeps the valid-`k` interval exactly one wide, so an
/// integer always lands in it.
const MIN_WRAP: f32 = 1.0;
const MAX_WRAP: f32 = 2.0;

/// Texture resolution of the single-dot tile. The dot is centered and
/// repeats via `ImageFit::Tile`; 64 px gives a crisp dot under linear
/// filtering across the zoom-wrap range.
const TILE_PX: u32 = 64;

/// Dotted canvas backdrop, drawn as one tiled [`Shape::Image`]. A small
/// dot tile is generated once, registered into aperture's image cache,
/// and stamped across the whole canvas by a single tiled image whose UV
/// transform carries the pan/zoom — so the grid pans and zooms for the
/// cost of one draw call.
#[derive(Default, Debug)]
pub(crate) struct CanvasBackground {
    /// `(params, handle)` of the registered dot tile. Reused while the
    /// params are unchanged so we don't synthesize a 64×64 image every
    /// frame (the registry's `register` takes the `Image` by value, so
    /// even a cache hit would allocate the tile to hand in).
    tile: Option<(DotKey, ImageHandle)>,
}

/// The theme inputs `build_tile` reads. The per-frame "did the theme
/// change?" compare: an unchanged key reuses the cached tile handle
/// instead of re-synthesizing and re-uploading. Floats stored as raw
/// bits for exact `Eq`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DotKey {
    color: ColorU8,
    radius_bits: u32,
    spacing_bits: u32,
}

impl DotKey {
    fn from_theme(ctx: &AppContext<'_>) -> Self {
        Self {
            color: ctx.theme.colors.canvas_dot.to_srgb_u8(),
            radius_bits: ctx.theme.canvas_dot_radius.to_bits(),
            spacing_bits: ctx.theme.canvas_dot_spacing.to_bits(),
        }
    }
}

impl CanvasBackground {
    /// Draw the backdrop in the outer-canvas (screen-local) frame. Call
    /// inside the outer canvas, before the inner transformed canvas, so
    /// it sits beneath every connection and node. `pan`/`zoom` are the
    /// live viewport; a screen point `s` maps to grid coord
    /// `(s - pan) / tile_px`, which is exactly the tile UV.
    pub(crate) fn draw(&mut self, ui: &mut Ui, ctx: &AppContext<'_>, pan: Vec2, zoom: f32) {
        let spacing = ctx.theme.canvas_dot_spacing;
        if zoom <= f32::EPSILON || spacing <= f32::EPSILON {
            return;
        }
        // Last frame's canvas size — the grid's repeat count. A 1-frame
        // stale count on window resize is invisible. Absent on frame 1.
        let Some(size) = ui
            .response_for(outer_canvas_widget_id())
            .layout_rect
            .map(|r| r.size)
        else {
            return;
        };
        let tile_px = spacing * zoom * wrap_multiplier(zoom);
        if tile_px <= f32::EPSILON {
            return;
        }
        let handle = self.tile_handle(ui, ctx);
        ui.add_shape(Shape::Image {
            handle,
            local_rect: None,
            fit: ImageFit::Tile {
                offset: -pan / tile_px,
                scale: Vec2::new(size.w, size.h) / tile_px,
            },
            tint: Color::WHITE,
        });
    }

    fn tile_handle(&mut self, ui: &Ui, ctx: &AppContext<'_>) -> ImageHandle {
        let key = DotKey::from_theme(ctx);
        if let Some((cached, handle)) = &self.tile
            && *cached == key
        {
            return handle.clone();
        }
        // A changed `key` means a theme swap: register a fresh tile and
        // drop the old handle, freeing the previous tile's GPU texture.
        let handle = ui.register_image(build_tile(ctx));
        self.tile = Some((key, handle.clone()));
        handle
    }
}

/// Power-of-2 multiplier `m` such that `zoom · m` lands in
/// `[MIN_WRAP, MAX_WRAP]` (so `spacing · zoom · m ∈ [spacing, 2·spacing]`
/// px on screen). `min`/`max` on the bounds keep `clamp` panic-free even
/// if rounding inverts them.
fn wrap_multiplier(zoom: f32) -> f32 {
    let k_low = (MIN_WRAP / zoom).log2().ceil() as i32;
    let k_high = (MAX_WRAP / zoom).log2().floor() as i32;
    let k = 0i32.clamp(k_low.min(k_high), k_low.max(k_high));
    2.0_f32.powi(k)
}

/// Synthesize the single-dot tile: a transparent `TILE_PX²` square with
/// one centered filled dot in the theme's dot color. The dot's texel
/// radius is scaled so that at the base zoom (tile = `spacing` px) its
/// on-screen radius matches `canvas_dot_radius`.
fn build_tile(ctx: &AppContext<'_>) -> Image {
    let n = TILE_PX;
    let radius = (ctx.theme.canvas_dot_radius * n as f32 / ctx.theme.canvas_dot_spacing).max(0.5);
    let c = ctx.theme.colors.canvas_dot.to_srgb_u8();
    let center = n as f32 * 0.5;
    let r2 = radius * radius;
    let mut pixels = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            if dx * dx + dy * dy <= r2 {
                pixels.extend_from_slice(&[c.r, c.g, c.b, c.a]);
            } else {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Image::from_rgba8(n, n, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_multiplier_identity_at_scale_1() {
        // zoom 1 → want zoom·m ∈ [1, 2]; m = 1 (k = 0).
        assert_eq!(wrap_multiplier(1.0), 1.0);
    }

    #[test]
    fn wrap_multiplier_keeps_screen_spacing_in_band() {
        // For any zoom, zoom·m must land in [MIN_WRAP, MAX_WRAP] so the
        // on-screen tile is spacing·[1, 2] px.
        for &zoom in &[0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0] {
            let normalized = zoom * wrap_multiplier(zoom);
            assert!(
                (MIN_WRAP..=MAX_WRAP).contains(&normalized),
                "zoom={zoom} normalized={normalized} out of band",
            );
        }
    }

    #[test]
    fn wrap_multiplier_is_power_of_two() {
        for &zoom in &[0.1, 0.3, 1.0, 2.0, 5.0] {
            let m = wrap_multiplier(zoom);
            assert_eq!(m.log2().fract().abs(), 0.0, "m={m} not a power of two");
        }
    }
}
