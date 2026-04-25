use std::sync::Arc;

use eframe::egui;
use egui::epaint::{ColorImage, Mesh, Vertex};
use egui::{Color32, Pos2, Shape, TextureFilter, TextureHandle, TextureOptions};

use crate::gui::Gui;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::widgets::Texture;

/// Target on-screen spacing range (in tile-widths) the dot pattern is
/// wrapped to as the user zooms. `wrap_scale_multiplier` picks a
/// power-of-2 multiplier that lands in this range.
const MIN_WRAP_SPACING: f32 = 0.5;
const MAX_WRAP_SPACING: f32 = 3.0;

#[derive(Default)]
pub struct GraphBackgroundRenderer {
    texture: Option<TextureHandle>,
}

impl std::fmt::Debug for GraphBackgroundRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphBackgroundRenderer")
            .field("has_texture", &self.texture.is_some())
            .finish()
    }
}

impl GraphBackgroundRenderer {
    pub fn render(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>) {
        assert!(ctx.view_graph.scale > common::EPSILON);

        if self.texture.is_none() {
            self.rebuild_texture(gui);
        }
        self.draw_tiled(gui, ctx);
    }

    fn wrap_scale_multiplier(view_scale: f32, min: f32, max: f32) -> f32 {
        assert!(view_scale.is_finite() && view_scale > common::EPSILON);
        assert!(min.is_finite() && min > common::EPSILON);
        assert!(max.is_finite() && max > min);

        let ratio_min = (min as f64) / (view_scale as f64);
        let ratio_max = (max as f64) / (view_scale as f64);

        let k_low = ratio_min.log2().ceil() as i32;
        let k_high = ratio_max.log2().floor() as i32;
        let k = 0_i32.clamp(k_low, k_high);

        2.0_f32.powi(k)
    }

    fn rebuild_texture(&mut self, gui: &mut Gui<'_>) {
        let radius = gui.style.graph_background.dotted_radius_base;
        let color = gui.style.graph_background.dotted_color;

        let tile = gui
            .style
            .graph_background
            .dotted_base_spacing
            .ceil()
            .max(radius * 2.0 + 2.0);
        let size = tile as usize;
        let mut image = ColorImage::new([size, size], vec![Color32::TRANSPARENT; size * size]);

        let center = (tile * 0.5, tile * 0.5);
        let r_sq = radius * radius;
        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 + 0.5 - center.0;
                let dy = y as f32 + 0.5 - center.1;
                if dx * dx + dy * dy <= r_sq {
                    image[(x, y)] = color;
                }
            }
        }

        let options = TextureOptions {
            magnification: TextureFilter::Linear,
            minification: TextureFilter::Linear,
            mipmap_mode: Some(TextureFilter::Linear),
            wrap_mode: egui::TextureWrapMode::Repeat,
        };
        let handle = Texture::new("graph_dots", image).options(options).load(gui);
        self.texture = Some(handle);
    }

    fn draw_tiled(&self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>) {
        let texture = self.texture.as_ref().unwrap();

        let view_scale = ctx.view_graph.scale;
        let scale_multiplier =
            Self::wrap_scale_multiplier(view_scale, MIN_WRAP_SPACING, MAX_WRAP_SPACING);

        let base_spacing = gui.style.graph_background.dotted_base_spacing;
        assert!(base_spacing > common::EPSILON);
        let world_spacing = base_spacing * scale_multiplier;

        let origin = gui.rect.min + ctx.view_graph.pan;

        let uv = |p: Pos2| {
            let graph_pos = (p - origin) / view_scale;
            Pos2::new(graph_pos.x / world_spacing, graph_pos.y / world_spacing)
        };

        let rect = gui.rect;
        let vertex = |pos: Pos2| Vertex {
            pos,
            uv: uv(pos),
            color: Color32::WHITE,
        };
        let mut mesh = Mesh::with_texture(texture.id());
        mesh.vertices = vec![
            vertex(rect.left_top()),
            vertex(rect.right_top()),
            vertex(rect.right_bottom()),
            vertex(rect.left_bottom()),
        ];
        mesh.indices = vec![0, 1, 2, 0, 2, 3];

        gui.painter().add(Shape::mesh(Arc::new(mesh)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_scale_multiplier_identity_at_scale_1() {
        let m =
            GraphBackgroundRenderer::wrap_scale_multiplier(1.0, MIN_WRAP_SPACING, MAX_WRAP_SPACING);
        assert_eq!(m, 1.0);
    }

    #[test]
    fn wrap_scale_multiplier_result_in_bounds() {
        for &scale in &[0.2, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0] {
            let m = GraphBackgroundRenderer::wrap_scale_multiplier(
                scale,
                MIN_WRAP_SPACING,
                MAX_WRAP_SPACING,
            );
            let normalized = scale * m;
            assert!(
                (MIN_WRAP_SPACING..=MAX_WRAP_SPACING).contains(&normalized),
                "scale={scale} m={m} normalized={normalized}"
            );
        }
    }

    #[test]
    fn wrap_scale_multiplier_is_power_of_two() {
        for &scale in &[0.2, 0.5, 1.0, 2.0, 4.0] {
            let m = GraphBackgroundRenderer::wrap_scale_multiplier(
                scale,
                MIN_WRAP_SPACING,
                MAX_WRAP_SPACING,
            );
            assert!(
                m.log2().fract().abs() < f32::EPSILON,
                "m={m} not power of 2"
            );
        }
    }
}
