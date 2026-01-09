use eframe::egui;
use egui::{Color32, FontFamily, FontId, Shadow, Stroke};

#[derive(Debug, Clone)]
pub struct Style {
    pub heading_font: FontId,
    pub body_font: FontId,
    pub sub_font: FontId,
    pub mono_font: FontId,

    pub text_color: Color32,
    pub noninteractive_text_color: Color32,

    pub noninteractive_bg_fill: Color32,
    pub active_bg_fill: Color32,
    pub hover_bg_fill: Color32,
    pub inactive_bg_fill: Color32,
    pub inactive_bg_stroke: Stroke,
    pub active_bg_stroke: Stroke,

    pub padding: f32,
    pub small_padding: f32,
    pub corner_radius: f32,
    pub small_corner_radius: f32,

    pub checked_bg_fill: Color32,
    pub checked_text_color: Color32,

    pub background: GraphBackgroundStyle,
    pub connections: ConnectionStyle,
    pub node: NodeStyle,
}

#[derive(Debug, Clone)]
pub struct GraphBackgroundStyle {
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
}

#[derive(Debug, Clone)]
pub struct ConnectionStyle {
    pub stroke_width: f32,
    pub highlight_stroke: Stroke,
    pub breaker_stroke: Stroke,
}

#[derive(Debug, Clone)]
pub struct NodeStyle {
    pub status_terminal_color: Color32,
    pub status_impure_color: Color32,
    pub status_dot_radius: f32,

    pub executed_shadow: Shadow,
    pub cached_shadow: Shadow,
    pub missing_inputs_shadow: Shadow,

    pub cache_btn_width: f32,
    pub remove_btn_size: f32,

    pub port_radius: f32,
    pub port_activation_radius: f32,
    pub port_label_side_padding: f32,

    pub input_port_color: Color32,
    pub output_port_color: Color32,
    pub input_hover_color: Color32,
    pub output_hover_color: Color32,

    pub const_stroke: Stroke,
}

impl Style {
    pub fn new(scale: f32) -> Self {
        assert!(scale > 0.0, "style scale must be greater than 0");
        let scaled = |value: f32| value * scale;
        let scaled_u8 = |value: u8| {
            let scaled_value = (f32::from(value) * scale).round();
            assert!(
                scaled_value <= u8::MAX as f32,
                "style scale too large for shadow values"
            );
            scaled_value as u8
        };

        Self {
            heading_font: FontId {
                size: scaled(18.0),
                family: FontFamily::Proportional,
            },
            body_font: FontId {
                size: scaled(15.0),
                family: FontFamily::Proportional,
            },
            sub_font: FontId {
                size: scaled(13.0),
                family: FontFamily::Proportional,
            },
            mono_font: FontId {
                size: scaled(13.0),
                family: FontFamily::Monospace,
            },

            text_color: Color32::from_rgb(192, 192, 192),
            noninteractive_text_color: Color32::from_rgb(140, 140, 140),
            checked_text_color: Color32::from_rgb(60, 50, 20),

            noninteractive_bg_fill: Color32::from_rgb(35, 35, 35),
            hover_bg_fill: Color32::from_rgb(50, 50, 50),
            inactive_bg_fill: Color32::from_rgb(40, 40, 40),
            inactive_bg_stroke: Stroke::new(scaled(1.0), Color32::from_rgb(65, 65, 65)),
            active_bg_stroke: Stroke::new(scaled(1.0), Color32::from_rgb(128, 128, 128)),
            active_bg_fill: Color32::from_rgb(60, 60, 60),
            checked_bg_fill: Color32::from_rgb(240, 205, 90),

            padding: scaled(4.0),
            small_padding: scaled(2.0),
            corner_radius: scaled(4.0),
            small_corner_radius: scaled(2.0),

            background: GraphBackgroundStyle {
                dotted_color: Color32::from_rgb(48, 48, 48),
                dotted_base_spacing: scaled(24.0),
                dotted_radius_base: scaled(1.2),
                dotted_radius_min: scaled(0.6),
                dotted_radius_max: scaled(2.4),
            },
            connections: ConnectionStyle {
                stroke_width: scaled(1.5),
                highlight_stroke: Stroke::new(scaled(1.5), Color32::from_rgb(255, 90, 90)),
                breaker_stroke: Stroke::new(scaled(1.5), Color32::from_rgb(255, 120, 120)),
            },
            node: NodeStyle {
                const_stroke: Stroke::new(scaled(1.0), Color32::from_rgb(70, 150, 255)),

                status_dot_radius: scaled(4.0),
                status_terminal_color: Color32::from_rgb(128, 128, 128),
                status_impure_color: Color32::from_rgb(255, 150, 70),

                executed_shadow: Shadow {
                    color: Color32::from_rgb(66, 216, 130),
                    offset: [0, 0],
                    blur: scaled_u8(5),
                    spread: scaled_u8(2),
                },
                cached_shadow: Shadow {
                    color: Color32::from_rgb(248, 216, 75),
                    offset: [0, 0],
                    blur: scaled_u8(5),
                    spread: scaled_u8(2),
                },
                missing_inputs_shadow: Shadow {
                    color: Color32::from_rgb(238, 66, 66),
                    offset: [0, 0],
                    blur: scaled_u8(5),
                    spread: scaled_u8(2),
                },

                cache_btn_width: scaled(50.0),
                remove_btn_size: scaled(10.0),

                port_radius: scaled(18.0 * 0.3),
                port_activation_radius: scaled(18.0 * 0.3 * 1.3),
                port_label_side_padding: scaled(8.0),

                input_port_color: Color32::from_rgb(70, 150, 255),
                output_port_color: Color32::from_rgb(70, 200, 200),
                input_hover_color: Color32::from_rgb(120, 190, 255),
                output_hover_color: Color32::from_rgb(110, 230, 210),
            },
        }
    }
}
