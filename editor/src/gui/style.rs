use eframe::egui;
use egui::{Color32, Stroke};

const INPUT_PORT_COLOR: Color32 = Color32::from_rgb(70, 150, 255);

#[derive(Debug, Clone)]
pub struct GraphStyle {
    pub scale: f32,
    pub header_text_offset: f32,
    pub cache_button_width_factor: f32,
    pub cache_button_vertical_pad_factor: f32,
    pub cache_button_text_pad_factor: f32,
    pub cache_active_color: Color32,
    pub cache_checked_text_color: Color32,
    pub status_dot_radius: f32,
    pub status_item_gap: f32,
    pub input_port_color: Color32,
    pub output_port_color: Color32,
    pub input_hover_color: Color32,
    pub output_hover_color: Color32,
    pub connection_stroke: Stroke,
    pub connection_highlight_stroke: Stroke,
    pub temp_connection_stroke: Stroke,
    pub breaker_stroke: Stroke,
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
    pub node_fill: Color32,
    pub node_stroke: Stroke,
    pub const_stroke: Stroke,
    pub selected_stroke: Stroke,
}

impl GraphStyle {
    pub fn new(ui: &egui::Ui, scale: f32) -> Self {
        assert!(scale.is_finite(), "style scale must be finite");
        assert!(scale > 0.0, "style scale must be positive");

        let visuals = ui.visuals();

        let node_stroke = visuals.widgets.noninteractive.bg_stroke;
        let selected_stroke =
            Stroke::new(node_stroke.width.max(1.2), visuals.selection.stroke.color);
        let const_stroke = Stroke::new(node_stroke.width, INPUT_PORT_COLOR.gamma_multiply(0.7));

        Self {
            scale,
            header_text_offset: 4.0 * scale,
            cache_button_width_factor: 3.1,
            cache_button_vertical_pad_factor: 0.4,
            cache_button_text_pad_factor: 0.5,
            cache_active_color: Color32::from_rgb(240, 205, 90),
            cache_checked_text_color: Color32::from_rgb(60, 50, 20),
            status_dot_radius: 4.0 * scale,
            status_item_gap: 6.0 * scale,
            input_port_color: INPUT_PORT_COLOR,
            output_port_color: Color32::from_rgb(70, 200, 200),
            input_hover_color: Color32::from_rgb(120, 190, 255),
            output_hover_color: Color32::from_rgb(110, 230, 210),
            connection_stroke: Stroke::new(2.0, INPUT_PORT_COLOR),
            connection_highlight_stroke: Stroke::new(2.5, Color32::from_rgb(255, 90, 90)),
            temp_connection_stroke: Stroke::new(2.0, Color32::from_rgb(170, 200, 255)),
            breaker_stroke: Stroke::new(2.5, Color32::from_rgb(255, 120, 120)),
            dotted_color: Color32::from_rgba_unmultiplied(255, 255, 255, 28),
            dotted_base_spacing: 24.0,
            dotted_radius_base: 1.2,
            dotted_radius_min: 0.6,
            dotted_radius_max: 2.4,
            node_fill: visuals.widgets.noninteractive.bg_fill,
            node_stroke,
            selected_stroke,
            const_stroke,
        }
    }
}
