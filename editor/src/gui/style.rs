use eframe::egui;
use egui::{Color32, FontId, Stroke};

#[derive(Debug)]
pub struct Style {
    pub heading_font: FontId,
    pub body_font: FontId,
    pub sub_font: FontId,

    pub text_color: Color32,
    pub widget_noninteractive_text_color: Color32,
    pub widget_noninteractive_bg_fill: Color32,
    pub widget_active_bg_fill: Color32,
    pub widget_hover_bg_fill: Color32,
    pub widget_inactive_bg_fill: Color32,
    pub widget_inactive_bg_stroke: Stroke,

    pub cache_active_color: Color32,
    pub cache_checked_text_color: Color32,
    
    pub corner_radius: f32,
    pub connection_stroke: Stroke,
    pub connection_highlight_stroke: Stroke,
    pub temp_connection_stroke: Stroke,
    pub breaker_stroke: Stroke,
    
    pub background: GraphBackgroundStyle,
    pub node: NodeStyle,
}

#[derive(Debug)]
pub struct GraphBackgroundStyle {
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
}

#[derive(Debug)]
pub struct NodeStyle {
    pub padding: f32,
    pub body_fill: Color32,
    pub body_stroke: Stroke,
    pub selected_body_stroke: Stroke,

    pub status_terminal_color: Color32,
    pub status_impure_color: Color32,
    pub status_dot_radius: f32,

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
    pub fn new() -> Self {
        Self {
            heading_font: FontId {
                size: 18.0,
                family: egui::FontFamily::Proportional,
            },
            body_font: FontId {
                size: 15.0,
                family: egui::FontFamily::Proportional,
            },
            sub_font: FontId {
                size: 13.0,
                family: egui::FontFamily::Proportional,
            },
            text_color: Color32::from_rgb(192, 192, 192),

            widget_noninteractive_text_color: Color32::from_rgb(140, 140, 140),
            widget_noninteractive_bg_fill: Color32::from_rgb(35, 35, 35),
            widget_active_bg_fill: Color32::from_rgb(60, 60, 60),
            widget_hover_bg_fill: Color32::from_rgb(50, 50, 50),
            widget_inactive_bg_fill: Color32::from_rgb(40, 40, 40),
            widget_inactive_bg_stroke: Stroke::new(1.0, Color32::from_rgb(65, 65, 65)),

            cache_active_color: Color32::from_rgb(240, 205, 90),
            cache_checked_text_color: Color32::from_rgb(60, 50, 20),
            corner_radius: 4.0,
            connection_stroke: Stroke::new(2.0, Color32::from_rgb(70, 150, 255)),
            connection_highlight_stroke: Stroke::new(2.0, Color32::from_rgb(255, 90, 90)),
            temp_connection_stroke: Stroke::new(2.0, Color32::from_rgb(170, 200, 255)),
            breaker_stroke: Stroke::new(2.0, Color32::from_rgb(255, 120, 120)),
            background: GraphBackgroundStyle {
                dotted_color: Color32::from_rgb(48, 48, 48),
                dotted_base_spacing: 24.0,
                dotted_radius_base: 1.2,
                dotted_radius_min: 0.6,
                dotted_radius_max: 2.4,
            },
            node: NodeStyle {
                padding: 4.0,

                body_fill: Color32::from_rgb(27, 27, 27),
                body_stroke: Stroke::new(1.0, Color32::from_rgb(60, 60, 60)),
                selected_body_stroke: Stroke::new(1.0, Color32::from_rgb(128, 128, 128)),

                const_stroke: Stroke::new(1.0, Color32::from_rgb(70, 150, 255)),

                status_terminal_color: Color32::from_rgb(128, 128, 128),
                status_impure_color: Color32::from_rgb(255, 150, 70),
                status_dot_radius: 4.0,

                port_radius: 18.0 * 0.3,
                port_activation_radius: 18.0 * 0.3 * 1.5,
                port_label_side_padding: 8.0,
                input_port_color: Color32::from_rgb(70, 150, 255),
                output_port_color: Color32::from_rgb(70, 200, 200),
                input_hover_color: Color32::from_rgb(120, 190, 255),
                output_hover_color: Color32::from_rgb(110, 230, 210),
            },
        }
    }
}
