use egui::Pos2;

#[derive(Debug)]
pub struct ConnectionBezier;

impl ConnectionBezier {
    pub const STEPS: usize = 24;

    pub fn sample(points: &mut Vec<Pos2>, start: Pos2, end: Pos2, scale: f32) {
        let steps = Self::STEPS;
        assert!(steps > 2, "bezier steps must be greater than 2");

        let p0 = start;
        let p3 = end;

        let control_offset = control_offset(p0, p3, scale);
        let p1 = p0 + egui::vec2(-control_offset, 0.0);
        let p2 = p3 + egui::vec2(control_offset, 0.0);

        points.clear();
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let one_minus = 1.0 - t;
            let a = one_minus * one_minus * one_minus;
            let b = 3.0 * one_minus * one_minus * t;
            let c = 3.0 * one_minus * t * t;
            let d = t * t * t;
            let x = a * p0.x + b * p1.x + c * p2.x + d * p3.x;
            let y = a * p0.y + b * p1.y + c * p2.y + d * p3.y;
            points.push(Pos2::new(x, y));
        }
    }

    pub fn segments_intersect(a1: Pos2, a2: Pos2, b1: Pos2, b2: Pos2) -> bool {
        let o1 = orient(a1, a2, b1);
        let o2 = orient(a1, a2, b2);
        let o3 = orient(b1, b2, a1);
        let o4 = orient(b1, b2, a2);
        let eps = 1e-6;

        if o1.abs() < eps && on_segment(a1, a2, b1) {
            return true;
        }
        if o2.abs() < eps && on_segment(a1, a2, b2) {
            return true;
        }
        if o3.abs() < eps && on_segment(b1, b2, a1) {
            return true;
        }
        if o4.abs() < eps && on_segment(b1, b2, a2) {
            return true;
        }

        (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0)
    }
}

fn control_offset(start: Pos2, end: Pos2, scale: f32) -> f32 {
    let dx = (end.x - start.x).abs();
    (dx * 0.5).max(10.0 * scale)
}

fn orient(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn on_segment(a: Pos2, b: Pos2, p: Pos2) -> bool {
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);
    p.x >= min_x - 1e-6 && p.x <= max_x + 1e-6 && p.y >= min_y - 1e-6 && p.y <= max_y + 1e-6
}
