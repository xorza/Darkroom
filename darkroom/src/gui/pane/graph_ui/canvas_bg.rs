use glam::Vec2;
use palantir::widget::Shape;
use palantir::{Image, ImageFit, ImageHandle, SrgbaU8, UVec2, Ui};

use crate::core::document::viewport::Viewport;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::theme::Theme;

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

/// Dotted canvas backdrop, drawn as one tiled [`Shape::image`](palantir::widget::Shape::image). A small
/// dot tile is generated once, registered into palantir's image cache,
/// and stamped across the whole canvas by a single tiled image whose UV
/// transform carries the pan/zoom — so the grid pans and zooms for the
/// cost of one draw call.
#[derive(Default, Debug)]
pub(super) struct CanvasBg {
    // Reuse the tile until its inputs change; constructing its pixels allocates.
    tile: Option<DotTile>,
}

/// The registered dot tile and the theme inputs it was drawn from.
#[derive(Debug)]
struct DotTile {
    key: DotKey,
    handle: ImageHandle,
}

// Float bits preserve exact equality for cache invalidation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DotKey {
    color: SrgbaU8,
    radius_bits: u32,
    spacing_bits: u32,
}

impl DotKey {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            color: theme.canvas.dot.to_srgba(),
            radius_bits: theme.canvas.dot_radius.to_bits(),
            spacing_bits: theme.canvas.dot_spacing.to_bits(),
        }
    }
}

impl CanvasBg {
    /// Draw the backdrop in the outer-canvas (screen-local) frame. Call
    /// inside the outer canvas, before the inner transformed canvas, so
    /// it sits beneath every wire and node. `viewport` is the live
    /// camera; a screen point `s` maps to grid coord
    /// `(s - pan) / tile_px`, which is exactly the tile UV.
    pub(super) fn draw(&mut self, ui: &mut Ui, theme: &Theme, viewport: Viewport) {
        let Viewport { pan, zoom } = viewport;
        let spacing = theme.canvas.dot_spacing;
        if zoom <= f32::EPSILON || spacing <= f32::EPSILON {
            return;
        }
        // Last frame's canvas size — the grid's repeat count. A 1-frame
        // stale count on window resize is invisible. Absent on frame 1.
        let Some(size) = ui
            .response_for(canvas::outer_wid())
            .layout_rect
            .map(|r| r.size)
        else {
            return;
        };
        let tile_px = spacing * zoom * wrap_multiplier(zoom);
        if tile_px <= f32::EPSILON {
            return;
        }
        let handle = self.tile_handle(ui, theme);
        ui.add_shape(Shape::image(handle).fit(ImageFit::Tile {
            offset: -pan / tile_px,
            scale: Vec2::new(size.w, size.h) / tile_px,
        }));
    }

    fn tile_handle(&mut self, ui: &Ui, theme: &Theme) -> ImageHandle {
        let key = DotKey::from_theme(theme);
        if let Some(tile) = &self.tile
            && tile.key == key
        {
            return tile.handle.clone();
        }
        let handle = ui
            .load_image(&build_tile(theme))
            .expect("canvas tile fits every supported GPU");
        self.tile = Some(DotTile {
            key,
            handle: handle.clone(),
        });
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
fn build_tile(theme: &Theme) -> Image {
    let n = TILE_PX;
    let radius = (theme.canvas.dot_radius * n as f32 / theme.canvas.dot_spacing).max(0.5);
    let c = theme.canvas.dot.to_srgba();
    let center = n as f32 * 0.5;
    let r2 = radius * radius;
    let mut tile = Image::blank(UVec2::splat(n));
    tile.fill_with(|x, y| {
        let dx = x as f32 + 0.5 - center;
        let dy = y as f32 + 0.5 - center;
        if dx * dx + dy * dy <= r2 {
            c
        } else {
            SrgbaU8::new(0, 0, 0, 0)
        }
    });
    tile
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both halves of the multiplier's contract, on one sweep: it is always a
    /// power of two, so a zoom change re-tiles onto the same lattice rather
    /// than sliding the grid under the nodes; and zoom·m lands in
    /// `[MIN_WRAP, MAX_WRAP]`, so the on-screen tile is spacing·[1, 2] px.
    #[test]
    fn wrap_multiplier_is_a_power_of_two_holding_screen_spacing_in_band() {
        // zoom 1 → want zoom·m ∈ [1, 2]; m = 1 (k = 0).
        assert_eq!(wrap_multiplier(1.0), 1.0);

        for zoom in [0.1, 0.25, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0] {
            let m = wrap_multiplier(zoom);
            assert_eq!(
                m.log2().fract().abs(),
                0.0,
                "zoom={zoom}: m={m} is not a power of two"
            );
            let normalized = zoom * m;
            assert!(
                (MIN_WRAP..=MAX_WRAP).contains(&normalized),
                "zoom={zoom} normalized={normalized} out of band",
            );
        }
    }
}
