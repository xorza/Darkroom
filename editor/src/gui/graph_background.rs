use std::sync::Arc;

use eframe::egui;
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Pos2, Shape, Vec2};

use crate::common::UiEquals;
use crate::gui::Gui;
use crate::gui::graph_ctx::GraphContext;

#[derive(Debug, Default)]
pub struct GraphBackgroundRenderer {
    mesh: Arc<Mesh>,
    last_pan: Vec2,
    last_scale: f32,
    last_rect_size: Vec2,
    inited: bool,
}

impl GraphBackgroundRenderer {
    pub fn render(&mut self, gui: &Gui<'_>, ctx: &GraphContext<'_>) {
        let scale = ctx.view_graph.scale;
        let pan = ctx.view_graph.pan;
        let rect_size = gui.rect.size();

        assert!(scale > common::EPSILON, "view graph scale must be positive");

        if !self.inited
            || !self.last_scale.ui_equals(&scale)
            || !self.last_pan.ui_equals(&pan)
            || !self.last_rect_size.ui_equals(&rect_size)
        {
            self.rebuild_mesh(gui, ctx);
            self.last_scale = scale;
            self.last_pan = pan;
            self.last_rect_size = rect_size;
            self.inited = true;
        }

        gui.painter().add(Shape::mesh(Arc::clone(&self.mesh)));
    }

    fn rebuild_mesh(&mut self, gui: &Gui<'_>, ctx: &GraphContext<'_>) {
        let spacing = gui.style.graph_background.dotted_base_spacing;
        assert!(spacing > 0.0, "background spacing must be positive");

        let radius = (gui.style.graph_background.dotted_radius_base).clamp(
            gui.style.graph_background.dotted_radius_min,
            gui.style.graph_background.dotted_radius_max,
        );
        let color = gui.style.graph_background.dotted_color;
        let origin = gui.rect.min + ctx.view_graph.pan;
        let offset_x = (gui.rect.left() - origin.x).rem_euclid(spacing);
        let offset_y = (gui.rect.top() - origin.y).rem_euclid(spacing);
        let start_x = gui.rect.left() - offset_x - spacing;
        let start_y = gui.rect.top() - offset_y - spacing;

        let mesh = Arc::get_mut(&mut self.mesh).unwrap();
        mesh.clear();

        let segments = 5;
        let mut y = start_y;
        while y <= gui.rect.bottom() + spacing {
            let mut x = start_x;
            while x <= gui.rect.right() + spacing {
                add_circle_to_mesh(mesh, Pos2::new(x, y), radius, color, segments);
                x += spacing;
            }
            y += spacing;
        }
    }
}

fn add_circle_to_mesh(
    mesh: &mut Mesh,
    center: Pos2,
    radius: f32,
    color: egui::Color32,
    segments: usize,
) {
    assert!(segments >= 3, "circle mesh needs at least 3 segments");

    let base_index = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex {
        pos: center,
        uv: WHITE_UV,
        color,
    });

    let step = std::f32::consts::TAU / segments as f32;
    for i in 0..segments {
        let angle = step * i as f32;
        let pos = Pos2::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        );
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color,
        });
    }

    for i in 0..segments {
        let next = if i + 1 == segments { 0 } else { i + 1 };
        mesh.indices.push(base_index);
        mesh.indices.push(base_index + 1 + i as u32);
        mesh.indices.push(base_index + 1 + next as u32);
    }
}
