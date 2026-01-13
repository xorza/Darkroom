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
        let scale = ctx.view_graph.scale;

        assert!(scale > common::EPSILON, "view graph scale must be positive");

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

        const MIN_SPACING: f32 = 8.0;
        const MAX_SPACING: f32 = 60.0;
        let spacing = (gui.style.graph_background.dotted_base_spacing * gui.scale)
            .clamp(MIN_SPACING, MAX_SPACING);
        let origin = gui.rect.min + ctx.view_graph.pan;

        let uv = |p: Pos2| {
            let offset = (p - origin) / spacing;
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
            .add(Shape::mesh(Arc::clone(&self.quad_mesh.as_ref().unwrap())));
    }
}
