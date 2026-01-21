use egui::epaint::{Mesh, Shape, Vertex, WHITE_UV};
use egui::{Color32, Pos2, vec2};

/// Draws a circle with a radial gradient shadow that fades from `shadow_color` to transparent.
///
/// The circle is filled with `shadow_color` up to `inner_radius`, then fades to transparent
/// at `outer_radius`.
pub fn draw_circle_with_gradient_shadow(
    painter: &egui::Painter,
    center: Pos2,
    inner_radius: f32,
    outer_radius: f32,
    shadow_color: Color32,
) {
    const SEGMENTS: usize = 12;

    let mut mesh = Mesh::default();

    // Center vertex with full shadow color
    let center_idx = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex {
        pos: center,
        uv: WHITE_UV,
        color: shadow_color,
    });

    // Inner ring vertices (full shadow color)
    let inner_start_idx = mesh.vertices.len() as u32;
    for i in 0..SEGMENTS {
        let angle = (i as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
        let pos = center + vec2(angle.cos(), angle.sin()) * inner_radius;
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color: shadow_color,
        });
    }

    // Outer ring vertices (transparent)
    let outer_start_idx = mesh.vertices.len() as u32;
    let transparent =
        Color32::from_rgba_unmultiplied(shadow_color.r(), shadow_color.g(), shadow_color.b(), 0);
    for i in 0..SEGMENTS {
        let angle = (i as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
        let pos = center + vec2(angle.cos(), angle.sin()) * outer_radius;
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color: transparent,
        });
    }

    // Triangles for inner circle (center to inner ring)
    for i in 0..SEGMENTS as u32 {
        let next = (i + 1) % SEGMENTS as u32;
        mesh.indices.push(center_idx);
        mesh.indices.push(inner_start_idx + i);
        mesh.indices.push(inner_start_idx + next);
    }

    // Triangles for gradient ring (inner to outer)
    for i in 0..SEGMENTS as u32 {
        let next = (i + 1) % SEGMENTS as u32;
        // First triangle
        mesh.indices.push(inner_start_idx + i);
        mesh.indices.push(outer_start_idx + i);
        mesh.indices.push(inner_start_idx + next);
        // Second triangle
        mesh.indices.push(inner_start_idx + next);
        mesh.indices.push(outer_start_idx + i);
        mesh.indices.push(outer_start_idx + next);
    }

    painter.add(Shape::mesh(mesh));
}
