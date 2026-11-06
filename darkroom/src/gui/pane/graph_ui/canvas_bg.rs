use glam::Vec2;
use palantir::widget::{Mesh, Shape};
use palantir::{Configure, Panel, RgbaF32, Sizing, TranslateScale, Ui};

use crate::core::document::viewport::Viewport;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::theme::Theme;

/// Target on-screen dot spacing range, as a multiple of the theme's
/// base spacing. A power-of-2 multiplier wraps the world spacing into
/// `[base, 2·base]` px so the dot field stays a stable density instead
/// of collapsing into noise (zoomed out) or drifting apart (zoomed in).
/// `MAX = 2·MIN` keeps the valid-`k` interval exactly one wide, so an
/// integer always lands in it.
const MIN_WRAP: f32 = 1.0;
const MAX_WRAP: f32 = 2.0;

/// A dot's rim: enough for a dot a few pixels wide to read as round.
const RIM_VERTICES: u32 = 6;

/// Logical px past a dot's radius where its alpha reaches zero: the one edge a mesh has, as it
/// draws with no antialiasing.
const FRINGE: f32 = 0.5;

/// Dotted canvas backdrop, one mesh of the whole lattice at the theme's spacing. It is the densest
/// the camera ever shows, so a pan or a zoom only moves and scales it, and a frame copies it once.
///
/// Refused: one tiled image over the canvas, which blends every pixel of it for the few the dots
/// cover, a `pan` frame at 1.04 ms against the mesh's 0.76 ms on an Apple laptop, and a `zoom` frame
/// at 1.15 ms against 0.97 ms; a shape per dot, 0.89 ms and 0.96 ms, which also took a still frame
/// from 84 to 110 µs, where the mesh takes it to 89 µs, as each dot is one more shape to record.
#[derive(Default, Debug)]
pub(super) struct CanvasBg {
    mesh: Mesh,
    /// What [`Self::mesh`] was built from; `None` before the first build.
    built: Option<Lattice>,
}

/// What the dot mesh is built from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lattice {
    cols: u32,
    rows: u32,
    spacing: f32,
    radius: f32,
    color: RgbaF32,
}

impl CanvasBg {
    /// Draw the backdrop in the outer-canvas (screen-local) frame. Call inside the outer canvas,
    /// before the inner transformed canvas, so it sits beneath every wire and node. `viewport` is
    /// the live camera.
    pub(super) fn draw(&mut self, ui: &mut Ui, theme: &Theme, viewport: Viewport) {
        let Viewport { pan, zoom } = viewport;
        let spacing = theme.canvas.dot_spacing;
        if zoom <= f32::EPSILON || spacing <= f32::EPSILON {
            return;
        }
        // Last frame's canvas size. A 1-frame stale count on window resize is invisible. Absent on
        // frame 1.
        let Some(size) = ui
            .response_for(canvas::outer_wid())
            .layout_rect
            .map(|r| Vec2::new(r.size.w, r.size.h))
        else {
            return;
        };
        let scale = zoom * wrap_multiplier(zoom);
        let wanted = Lattice {
            cols: dots_across(size.x, spacing),
            rows: dots_across(size.y, spacing),
            spacing,
            radius: theme.canvas.dot_radius,
            color: theme.canvas.dot,
        };
        // A smaller window keeps the larger mesh, which the canvas clips, so a resize drag builds
        // only as it grows.
        if self.built.is_none_or(|built| !built.covers(wanted)) {
            wanted.build(&mut self.mesh);
            self.built = Some(wanted);
        }
        let mesh = &self.mesh;
        Panel::canvas()
            .id(canvas::dots_wid())
            .size(Sizing::FILL)
            .transform(TranslateScale::new(
                lattice_origin(pan, spacing * scale),
                scale,
            ))
            .show(ui, |ui| ui.add_shape(Shape::mesh(mesh)));
    }
}

impl Lattice {
    /// Whether a mesh of `self` draws what `wanted` asks: the same dots, as many or more.
    fn covers(self, wanted: Self) -> bool {
        self.cols >= wanted.cols
            && self.rows >= wanted.rows
            && self.spacing == wanted.spacing
            && self.radius == wanted.radius
            && self.color == wanted.color
    }

    /// Fill `mesh` with the lattice: a dot at `(col + 0.5, row + 0.5) · spacing`, its color at the
    /// center and clear at the rim.
    fn build(self, mesh: &mut Mesh) {
        let Self {
            cols,
            rows,
            spacing,
            radius,
            color,
        } = self;
        mesh.clear();
        let rim: Vec<Vec2> = (0..RIM_VERTICES)
            .map(|i| {
                let angle = std::f32::consts::TAU * i as f32 / RIM_VERTICES as f32;
                Vec2::from_angle(angle) * (radius + FRINGE)
            })
            .collect();
        let clear = color.with_alpha(0.0);
        for row in 0..rows {
            for col in 0..cols {
                let center_pos = (Vec2::new(col as f32, row as f32) + 0.5) * spacing;
                let center = mesh.vertex(center_pos, color);
                for &offset in &rim {
                    mesh.vertex(center_pos + offset, clear);
                }
                for i in 0..RIM_VERTICES {
                    mesh.triangle(center, center + 1 + i, center + 1 + (i + 1) % RIM_VERTICES);
                }
            }
        }
    }
}

/// The dots a lattice lays across a canvas `length` long, whatever the camera: the most `length`
/// holds at `spacing`, the densest pitch, and one past each end.
fn dots_across(length: f32, spacing: f32) -> u32 {
    #[expect(clippy::cast_sign_loss, reason = "a canvas's length is not negative")]
    let dots = (length / spacing).ceil() as u32;
    dots + 2
}

/// Where the lattice's first column and row go on screen for a dot at `pan + pitch / 2`: one pitch
/// or less before the canvas, so the lattice covers it from its first dot.
fn lattice_origin(pan: Vec2, pitch: f32) -> Vec2 {
    (-pan / pitch - 0.5).floor() * pitch + pan
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A dot sits at `(k + 0.5) · pitch + pan`. At pan (10, −5) and pitch 20 the first column is
    /// `k = floor(−10 / 20 − 0.5) = −1`, so the origin is x = −10 and its dot x = 0, and the first
    /// row `k = floor(5 / 20 − 0.5) = −1`, y = −25 and its dot y = −15. A pan on the lattice's own
    /// phase, 10 at pitch 20, gives the same dot, one pitch on.
    #[test]
    fn lattice_origin_puts_the_first_dot_before_the_canvas() {
        assert_eq!(
            lattice_origin(Vec2::new(10.0, -5.0), 20.0),
            Vec2::new(-10.0, -25.0),
        );
        assert_eq!(lattice_origin(Vec2::ZERO, 16.0), Vec2::splat(-16.0));
        assert_eq!(lattice_origin(Vec2::new(30.0, 0.0), 20.0).x, -10.0);
        assert_eq!(lattice_origin(Vec2::new(10.0, 0.0), 40.0).x, -30.0);
    }

    /// The lattice reaches both ends of the canvas at every pan and at both ends of the wrap band:
    /// its first dot is at or before 0, and its last, by `dots_across` at the densest pitch, at or
    /// past the length.
    #[test]
    fn the_lattice_covers_the_canvas_at_every_pan_and_pitch() {
        let (spacing, length) = (16.0, 771.4);
        let dots = dots_across(length, spacing) as f32;
        for pitch in [16.0, 24.0, 32.0] {
            for pan in [-1000.3, -7.0, 0.0, 9.5, 400.0] {
                let origin = lattice_origin(Vec2::splat(pan), pitch).x;
                assert!(origin + 0.5 * pitch <= 0.0, "{pitch} {pan}");
                assert!(origin + (dots - 0.5) * pitch >= length, "{pitch} {pan}");
            }
        }
    }

    /// A canvas 100 long at spacing 16 holds `ceil(6.25) = 7` dots, and 96 holds exactly 6: each
    /// plus one past each end. An empty canvas takes the two ends alone.
    #[test]
    fn dots_across_covers_the_canvas_at_the_densest_pitch() {
        assert_eq!(dots_across(100.0, 16.0), 9);
        assert_eq!(dots_across(96.0, 16.0), 8);
        assert_eq!(dots_across(0.0, 16.0), 2);
    }

    /// One dot at spacing 10 is a hexagon around (5, 5): its color at the center and clear on a rim
    /// of radius 1 + 0.5, its corners on the x axis at 5 ± 1.5. 2 columns and 3 rows reach from
    /// (5, 5) to (15, 25), so x runs from 5 − 1.5 to 15 + 1.5.
    #[test]
    fn a_lattice_builds_a_hexagon_per_dot() {
        let lattice = Lattice {
            cols: 1,
            rows: 1,
            spacing: 10.0,
            radius: 1.0,
            color: RgbaF32::WHITE,
        };
        let mut mesh = Mesh::new();
        lattice.build(&mut mesh);

        let center = Vec2::splat(5.0);
        let mut want = Mesh::new();
        want.vertex(center, RgbaF32::WHITE);
        for i in 0..6 {
            let at = Vec2::from_angle(std::f32::consts::TAU * i as f32 / 6.0) * 1.5;
            want.vertex(center + at, RgbaF32::WHITE.with_alpha(0.0));
        }
        for i in 0..6 {
            want.triangle(0, 1 + i, 1 + (i + 1) % 6);
        }
        assert_eq!(mesh.content_hash(), want.content_hash());

        let lattice = Lattice {
            cols: 2,
            rows: 3,
            ..lattice
        };
        lattice.build(&mut mesh);
        let bbox = mesh.bbox();
        assert!((bbox.min.x - 3.5).abs() < 1e-5, "{bbox:?}");
        assert!((bbox.max().x - 16.5).abs() < 1e-5, "{bbox:?}");

        let more = Lattice { cols: 3, ..lattice };
        assert!(more.covers(lattice));
        assert!(!lattice.covers(more));
        assert!(!lattice.covers(Lattice {
            radius: 2.0,
            ..lattice
        }));
        assert!(!lattice.covers(Lattice {
            spacing: 5.0,
            ..lattice
        }));
        assert!(!lattice.covers(Lattice {
            color: RgbaF32::BLACK,
            ..lattice
        }));
    }

    /// Both halves of the multiplier's contract, on one sweep: it is always a
    /// power of two, so a zoom change lands on the same lattice rather
    /// than sliding the grid under the nodes; and zoom·m lands in
    /// `[MIN_WRAP, MAX_WRAP]`, so the on-screen pitch is spacing·[1, 2] px.
    #[test]
    fn wrap_multiplier_is_a_power_of_two_holding_screen_spacing_in_band() {
        // m = 2^k with k the clamp of 0 into [ceil(log2(1 / zoom)), floor(log2(2 / zoom))]: a
        // zoom on a band edge keeps the smaller power, as 2 → 1 and 4 → 1/2 show.
        for (zoom, m) in [
            (0.1, 16.0),
            (0.25, 4.0),
            (0.3, 4.0),
            (0.5, 2.0),
            (0.75, 2.0),
            (1.0, 1.0),
            (1.5, 1.0),
            (2.0, 1.0),
            (3.0, 0.5),
            (4.0, 0.5),
            (5.0, 0.25),
        ] {
            assert_eq!(wrap_multiplier(zoom), m, "zoom {zoom}");
            assert!(
                (MIN_WRAP..=MAX_WRAP).contains(&(zoom * m)),
                "zoom {zoom}: {} out of band",
                zoom * m
            );
        }
    }
}
