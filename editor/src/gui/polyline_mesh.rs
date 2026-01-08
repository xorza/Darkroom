use std::sync::Arc;

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, Painter, Pos2, Shape};

#[derive(Debug, Clone)]
pub struct PolylineMesh {
    mesh: Arc<Mesh>,
    points: Vec<Pos2>,
}

impl PolylineMesh {
    pub fn with_point_capacity(points: usize) -> Self {
        Self {
            mesh: Arc::new(polyline_mesh_with_capacity(points)),
            points: Vec::with_capacity(points),
        }
    }

    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    pub fn points(&self) -> &[Pos2] {
        self.points.as_slice()
    }

    pub fn points_mut(&mut self) -> &mut Vec<Pos2> {
        &mut self.points
    }

    pub fn build_curve(
        &mut self,
        points: &[Pos2],
        start_color: Color32,
        end_color: Color32,
        width: f32,
        feather: f32,
    ) {
        let mesh = Arc::get_mut(&mut self.mesh).unwrap();
        mesh.clear();
        add_curve_to_mesh(mesh, points, start_color, end_color, width, feather);
    }

    pub fn build_curve_from_points(
        &mut self,
        start_color: Color32,
        end_color: Color32,
        width: f32,
        feather: f32,
    ) {
        let points = self.points.as_slice();
        let mesh = Arc::get_mut(&mut self.mesh).unwrap();
        mesh.clear();
        add_curve_to_mesh(mesh, points, start_color, end_color, width, feather);
    }

    pub fn render(&self, painter: &Painter) {
        painter.add(Shape::mesh(Arc::clone(&self.mesh)));
    }
}

pub fn add_curve_to_mesh(
    mesh: &mut Mesh,
    points: &[Pos2],
    start_color: Color32,
    end_color: Color32,
    width: f32,
    feather: f32,
) {
    assert!(points.len() >= 2);
    assert!(width > 0.0);
    assert!(feather >= 0.0);

    let segment_count = points.len() - 1;
    let half_width = width * 0.5;
    for (idx, segment) in points.windows(2).enumerate() {
        let a = segment[0];
        let b = segment[1];
        let dir = b - a;
        if dir.length_sq() <= common::EPSILON {
            continue;
        }
        let normal = dir.normalized().rot90();
        let outer = half_width + feather;
        let t0 = idx as f32 / segment_count as f32;
        let t1 = (idx + 1) as f32 / segment_count as f32;
        let color0 = lerp_color(start_color, end_color, t0);
        let color1 = lerp_color(start_color, end_color, t1);
        let color0_outer = set_alpha(color0, 0);
        let color1_outer = set_alpha(color1, 0);

        let inner_plus0 = a + normal * half_width;
        let inner_minus0 = a - normal * half_width;
        let inner_plus1 = b + normal * half_width;
        let inner_minus1 = b - normal * half_width;
        let outer_plus0 = a + normal * outer;
        let outer_minus0 = a - normal * outer;
        let outer_plus1 = b + normal * outer;
        let outer_minus1 = b - normal * outer;

        add_quad(
            mesh,
            [inner_plus0, inner_minus0, inner_minus1, inner_plus1],
            [color0, color0, color1, color1],
        );
        add_quad(
            mesh,
            [outer_plus0, inner_plus0, inner_plus1, outer_plus1],
            [color0_outer, color0, color1, color1_outer],
        );
        add_quad(
            mesh,
            [inner_minus0, outer_minus0, outer_minus1, inner_minus1],
            [color0, color0_outer, color1_outer, color1],
        );
    }
}

pub fn polyline_mesh_with_capacity(points: usize) -> Mesh {
    assert!(points >= 2, "bezier point count must be at least 2");
    let segments = points - 1;
    let quads_per_segment = 3;
    let vertices_per_quad = 4;
    let indices_per_quad = 6;
    let (vertex_capacity, index_capacity) = (
        segments * quads_per_segment * vertices_per_quad,
        segments * quads_per_segment * indices_per_quad,
    );

    let mut mesh = Mesh::default();
    mesh.vertices.reserve(vertex_capacity);
    mesh.indices.reserve(index_capacity);
    mesh
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |start: u8, end: u8| -> u8 {
        (start as f32 + (end as f32 - start as f32) * t).round() as u8
    };
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

fn set_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn add_quad(mesh: &mut Mesh, positions: [Pos2; 4], colors: [Color32; 4]) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex {
        pos: positions[0],
        uv: WHITE_UV,
        color: colors[0],
    });
    mesh.vertices.push(Vertex {
        pos: positions[1],
        uv: WHITE_UV,
        color: colors[1],
    });
    mesh.vertices.push(Vertex {
        pos: positions[2],
        uv: WHITE_UV,
        color: colors[2],
    });
    mesh.vertices.push(Vertex {
        pos: positions[3],
        uv: WHITE_UV,
        color: colors[3],
    });
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}
