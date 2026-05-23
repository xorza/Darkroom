use palantir::Color;

/// Visual palette for darkroom's UI. Owned by `MainWindow`, handed to
/// every UI subtree through [`crate::app::AppContext`] so call sites read colors
/// off a single source instead of hard-coded constants.
#[derive(Clone, Debug)]
pub struct Theme {
    pub canvas_bg: Color,
    pub connection_broken: Color,
    pub connection_width: f32,
    pub breaker_stroke: Color,
    pub breaker_stroke_width: f32,

    pub node_fill: Color,
    pub node_border: Color,
    pub node_border_width: f32,
    pub node_corner_radius: f32,

    pub header_fill: Color,
    pub header_corner_radius: f32,

    pub input_port: Color,
    pub output_port: Color,
    pub input_port_hover: Color,
    pub output_port_hover: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            canvas_bg: Color::hex(0x1e1e1e),
            connection_broken: Color::hex(0xff5a55),
            connection_width: 2.0,
            breaker_stroke: Color::hex(0xff5a55),
            breaker_stroke_width: 2.0,

            node_fill: Color::hex(0x2d2d33),
            node_border: Color::hex(0x5a5a66),
            node_border_width: 1.0,
            node_corner_radius: 6.0,

            header_fill: Color::hex(0x3a3a44),
            header_corner_radius: 6.0,

            input_port: Color::hex(0x77c97a),
            output_port: Color::hex(0xe39a4a),
            input_port_hover: Color::hex(0xb3e8b6),
            output_port_hover: Color::hex(0xffc878),
        }
    }
}
