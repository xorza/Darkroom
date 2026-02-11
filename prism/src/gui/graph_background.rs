use std::sync::Arc;

use eframe::egui;
use egui::epaint::{ColorImage, Mesh, Vertex};
use egui::{Color32, Pos2, Shape, TextureFilter, TextureHandle, TextureOptions, Vec2};

use crate::common::UiEquals;
use crate::gui::Gui;
use crate::gui::graph_ctx::GraphContext;

#[derive(Default)]
pub struct GraphBackgroundRenderer {
    texture: Option<TextureHandle>,
    quad_mesh: Option<Arc<Mesh>>,
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
            self.rebuild_texture(gui, ctx);
        }
        if self.quad_mesh.is_none() {
            let mut mesh = Mesh::default();
            mesh.vertices.resize(4, Vertex::default());
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            let arc = Arc::new(mesh);
            self.quad_mesh = Some(arc);
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

    fn rebuild_texture(&mut self, gui: &mut Gui<'_>, _ctx: &GraphContext<'_>) {
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
        let handle = gui.ui().ctx().load_texture("graph_dots", image, options);
        self.texture = Some(handle);
    }

    fn draw_tiled(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>) {
        let texture = self.texture.as_ref().unwrap();

        let min = 0.5;
        let max = 3.0;
        let view_scale = ctx.view_graph.scale;
        let scale_multiplier = Self::wrap_scale_multiplier(view_scale, min, max);

        let base_spacing = gui.style.graph_background.dotted_base_spacing;
        assert!(base_spacing > common::EPSILON);
        let world_spacing = base_spacing * scale_multiplier;

        let origin = gui.rect.min + ctx.view_graph.pan;

        let uv = |p: Pos2| {
            let graph_pos = (p - origin) / view_scale;
            let offset = graph_pos / world_spacing;
            Pos2::new(offset.x, offset.y)
        };

        {
            let mesh = Arc::make_mut(self.quad_mesh.as_mut().unwrap());
            mesh.texture_id = texture.id();

            mesh.vertices[0].pos = gui.rect.left_top();
            mesh.vertices[0].uv = uv(gui.rect.left_top());
            mesh.vertices[0].color = Color32::WHITE;

            mesh.vertices[1].pos = gui.rect.right_top();
            mesh.vertices[1].uv = uv(gui.rect.right_top());
            mesh.vertices[1].color = Color32::WHITE;

            mesh.vertices[2].pos = gui.rect.right_bottom();
            mesh.vertices[2].uv = uv(gui.rect.right_bottom());
            mesh.vertices[2].color = Color32::WHITE;

            mesh.vertices[3].pos = gui.rect.left_bottom();
            mesh.vertices[3].uv = uv(gui.rect.left_bottom());
            mesh.vertices[3].color = Color32::WHITE;
        }

        gui.painter()
            .add(Shape::mesh(Arc::clone(self.quad_mesh.as_ref().unwrap())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_scale_multiplier_identity_at_scale_1() {
        let m = GraphBackgroundRenderer::wrap_scale_multiplier(1.0, 0.5, 3.0);
        assert_eq!(m, 1.0);
    }

    #[test]
    fn wrap_scale_multiplier_result_in_bounds() {
        let min = 0.5;
        let max = 3.0;
        for &scale in &[0.2, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0] {
            let m = GraphBackgroundRenderer::wrap_scale_multiplier(scale, min, max);
            let normalized = scale * m;
            assert!(
                normalized >= min && normalized <= max,
                "scale={scale} m={m} normalized={normalized}"
            );
        }
    }

    #[test]
    fn wrap_scale_multiplier_is_power_of_two() {
        for &scale in &[0.2, 0.5, 1.0, 2.0, 4.0] {
            let m = GraphBackgroundRenderer::wrap_scale_multiplier(scale, 0.5, 3.0);
            assert!(
                m.log2().fract().abs() < f32::EPSILON,
                "m={m} not power of 2"
            );
        }
    }
}
