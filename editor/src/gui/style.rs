use eframe::egui;

#[derive(Debug, Clone)]
pub struct GraphStyle {
    pub scale: f32,
    pub header_text_offset: f32,
    pub cache_button_width_factor: f32,
    pub cache_button_vertical_pad_factor: f32,
    pub cache_button_text_pad_factor: f32,
    pub cache_active_color: egui::Color32,
    pub cache_checked_text_color: egui::Color32,
    pub status_dot_radius: f32,
    pub status_item_gap: f32,
    pub input_port_color: egui::Color32,
    pub output_port_color: egui::Color32,
    pub input_hover_color: egui::Color32,
    pub output_hover_color: egui::Color32,
    pub connection_stroke: egui::Stroke,
    pub connection_highlight_stroke: egui::Stroke,
    pub temp_connection_stroke: egui::Stroke,
    pub breaker_stroke: egui::Stroke,
    pub dotted_color: egui::Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
    pub node_fill: egui::Color32,
    pub node_stroke: egui::Stroke,
    pub selected_stroke: egui::Stroke,
}

impl GraphStyle {
    pub fn new(ui: &egui::Ui, scale: f32) -> Self {
        assert!(scale.is_finite(), "style scale must be finite");
        assert!(scale > 0.0, "style scale must be positive");

        let visuals = ui.visuals();
        let node_stroke = visuals.widgets.noninteractive.bg_stroke;
        let selected_stroke =
            egui::Stroke::new(node_stroke.width.max(2.0), visuals.selection.stroke.color);

        Self {
            scale,
            header_text_offset: 4.0 * scale,
            cache_button_width_factor: 3.1,
            cache_button_vertical_pad_factor: 0.4,
            cache_button_text_pad_factor: 0.5,
            cache_active_color: egui::Color32::from_rgb(240, 205, 90),
            cache_checked_text_color: egui::Color32::from_rgb(60, 50, 20),
            status_dot_radius: 4.0 * scale,
            status_item_gap: 6.0 * scale,
            input_port_color: egui::Color32::from_rgb(70, 150, 255),
            output_port_color: egui::Color32::from_rgb(70, 200, 200),
            input_hover_color: egui::Color32::from_rgb(120, 190, 255),
            output_hover_color: egui::Color32::from_rgb(110, 230, 210),
            connection_stroke: egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 160, 255)),
            connection_highlight_stroke: egui::Stroke::new(
                2.5,
                egui::Color32::from_rgb(255, 90, 90),
            ),
            temp_connection_stroke: egui::Stroke::new(2.0, egui::Color32::from_rgb(170, 200, 255)),
            breaker_stroke: egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 120, 120)),
            dotted_color: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 28),
            dotted_base_spacing: 24.0,
            dotted_radius_base: 1.2,
            dotted_radius_min: 0.6,
            dotted_radius_max: 2.4,
            node_fill: visuals.widgets.noninteractive.bg_fill,
            node_stroke,
            selected_stroke,
        }
    }

    pub fn validate(&self) {
        assert!(self.scale.is_finite(), "style scale must be finite");
        assert!(self.scale > 0.0, "style scale must be positive");
        assert!(
            self.header_text_offset.is_finite(),
            "header text offset must be finite"
        );
        assert!(
            self.cache_button_width_factor.is_finite(),
            "cache button width factor must be finite"
        );
        assert!(
            self.cache_button_width_factor > 0.0,
            "cache button width factor must be positive"
        );
        assert!(
            self.cache_button_vertical_pad_factor.is_finite(),
            "cache button vertical padding factor must be finite"
        );
        assert!(
            self.cache_button_vertical_pad_factor >= 0.0,
            "cache button vertical padding factor must be non-negative"
        );
        assert!(
            self.cache_button_text_pad_factor.is_finite(),
            "cache button text padding factor must be finite"
        );
        assert!(
            self.cache_button_text_pad_factor >= 0.0,
            "cache button text padding factor must be non-negative"
        );
        assert!(
            self.status_dot_radius.is_finite(),
            "status dot radius must be finite"
        );
        assert!(
            self.status_dot_radius >= 0.0,
            "status dot radius must be non-negative"
        );
        assert!(
            self.status_item_gap.is_finite(),
            "status item gap must be finite"
        );
        assert!(
            self.status_item_gap >= 0.0,
            "status item gap must be non-negative"
        );
        assert!(
            self.dotted_base_spacing.is_finite(),
            "dot spacing base must be finite"
        );
        assert!(
            self.dotted_base_spacing > 0.0,
            "dot spacing base must be positive"
        );
        assert!(
            self.dotted_radius_base.is_finite(),
            "dot radius base must be finite"
        );
        assert!(
            self.dotted_radius_base > 0.0,
            "dot radius base must be positive"
        );
        assert!(
            self.dotted_radius_min.is_finite(),
            "dot radius min must be finite"
        );
        assert!(
            self.dotted_radius_min >= 0.0,
            "dot radius min must be non-negative"
        );
        assert!(
            self.dotted_radius_max.is_finite(),
            "dot radius max must be finite"
        );
        assert!(
            self.dotted_radius_max >= self.dotted_radius_min,
            "dot radius max must be >= min"
        );
        assert!(
            self.connection_stroke.width.is_finite(),
            "connection stroke width must be finite"
        );
        assert!(
            self.connection_stroke.width >= 0.0,
            "connection stroke width must be non-negative"
        );
        assert!(
            self.connection_highlight_stroke.width.is_finite(),
            "connection highlight stroke width must be finite"
        );
        assert!(
            self.connection_highlight_stroke.width >= 0.0,
            "connection highlight stroke width must be non-negative"
        );
        assert!(
            self.temp_connection_stroke.width.is_finite(),
            "temp connection stroke width must be finite"
        );
        assert!(
            self.temp_connection_stroke.width >= 0.0,
            "temp connection stroke width must be non-negative"
        );
        assert!(
            self.breaker_stroke.width.is_finite(),
            "breaker stroke width must be finite"
        );
        assert!(
            self.breaker_stroke.width >= 0.0,
            "breaker stroke width must be non-negative"
        );
    }
}
