use egui::{Color32, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleSettings {
    pub color_bg_noninteractive: Color32,
    pub color_bg_inactive: Color32,
    pub color_bg_graph: Color32,
    pub color_bg_hover: Color32,
    pub color_bg_active: Color32,
    pub color_bg_checked: Color32,
    pub color_stroke_inactive: Color32,
    pub color_stroke_active: Color32,
    pub color_port_input: Color32,
    pub color_port_output: Color32,
    pub color_port_input_hover: Color32,
    pub color_port_output_hover: Color32,
    pub color_port_trigger: Color32,
    pub color_port_event: Color32,
    pub color_port_trigger_hover: Color32,
    pub color_port_event_hover: Color32,
    pub color_stroke_breaker: Color32,
    pub color_stroke_broke: Color32,
    pub color_text: Color32,
    pub color_text_noninteractive: Color32,
    pub color_text_checked: Color32,
    pub color_dot_impure: Color32,
    pub color_shadow_executed: Color32,
    pub color_shadow_cached: Color32,
    pub color_shadow_missing: Color32,
    pub color_dotted: Color32,
    pub corner_radius: f32,
    pub small_corner_radius: f32,
    pub default_bg_stroke_width: f32,
    pub big_padding: f32,
    pub padding: f32,
    pub small_padding: f32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
    pub connection_feather: f32,
    pub connection_stroke_width: f32,
    pub connection_highlight_feather: f32,
    pub connection_hover_detection_width: f32,
    pub connection_breaker_stroke_width: f32,
    pub status_dot_radius: f32,
    pub shadow_blur: u8,
    pub shadow_spread: u8,
    pub cache_btn_width: f32,
    pub remove_btn_size: f32,
    pub port_radius: f32,
    pub port_activation_radius: f32,
    pub port_label_side_padding: f32,
    pub const_badge_offset: Vec2,
}

impl Default for StyleSettings {
    fn default() -> Self {
        Self {
            color_bg_noninteractive: Color32::from_rgb(35, 35, 35),
            color_bg_inactive: Color32::from_rgb(40, 40, 40),
            color_bg_graph: Color32::from_rgb(16, 16, 16),
            color_bg_hover: Color32::from_rgb(50, 50, 50),
            color_bg_active: Color32::from_rgb(60, 60, 60),
            color_bg_checked: Color32::from_rgb(240, 205, 90),
            color_stroke_inactive: Color32::from_rgb(65, 65, 65),
            color_stroke_active: Color32::from_rgb(128, 128, 128),
            color_port_input: Color32::from_rgb(70, 150, 255),
            color_port_output: Color32::from_rgb(70, 200, 200),
            color_port_input_hover: Color32::from_rgb(120, 190, 255),
            color_port_output_hover: Color32::from_rgb(110, 230, 210),
            color_port_trigger: Color32::from_rgb(235, 200, 70),
            color_port_event: Color32::from_rgb(235, 140, 70),
            color_port_trigger_hover: Color32::from_rgb(255, 225, 120),
            color_port_event_hover: Color32::from_rgb(255, 175, 120),
            color_stroke_breaker: Color32::from_rgb(255, 120, 120),
            color_stroke_broke: Color32::from_rgb(255, 90, 90),
            color_text: Color32::from_rgb(192, 192, 192),
            color_text_noninteractive: Color32::from_rgb(140, 140, 140),
            color_text_checked: Color32::from_rgb(60, 50, 20),
            color_dot_impure: Color32::from_rgb(255, 150, 70),
            color_shadow_executed: Color32::from_rgb(66, 216, 130),
            color_shadow_cached: Color32::from_rgb(248, 216, 75),
            color_shadow_missing: Color32::from_rgb(238, 66, 66),
            color_dotted: Color32::from_rgb(48, 48, 48),
            corner_radius: 4.0,
            small_corner_radius: 2.0,
            default_bg_stroke_width: 1.0,
            big_padding: 6.0,
            padding: 4.0,
            small_padding: 2.0,
            dotted_base_spacing: 24.0,
            dotted_radius_base: 1.2,
            dotted_radius_min: 0.6,
            dotted_radius_max: 2.4,
            connection_feather: 0.8,
            connection_stroke_width: 1.5,
            connection_highlight_feather: 3.6,
            connection_hover_detection_width: 6.0,
            connection_breaker_stroke_width: 2.0,
            status_dot_radius: 4.0,
            shadow_blur: 5,
            shadow_spread: 2,
            cache_btn_width: 50.0,
            remove_btn_size: 10.0,
            port_radius: 5.0,
            port_activation_radius: 7.0,
            port_label_side_padding: 8.0,
            const_badge_offset: Vec2::new(-15.0, -15.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StyleSettings;

    #[test]
    fn style_settings_toml_roundtrip() {
        let settings = StyleSettings::default();
        let serialized =
            toml::to_string(&settings).expect("style settings should serialize to TOML");
        assert!(
            !serialized.trim().is_empty(),
            "serialized style settings should not be empty"
        );

        let deserialized: StyleSettings =
            toml::from_str(&serialized).expect("style settings should deserialize from TOML");

        assert_eq!(
            settings.color_bg_noninteractive, deserialized.color_bg_noninteractive,
            "background noninteractive color should round-trip"
        );
        assert_eq!(
            settings.color_port_input, deserialized.color_port_input,
            "input port color should round-trip"
        );
        assert_eq!(
            settings.color_shadow_cached, deserialized.color_shadow_cached,
            "cached shadow color should round-trip"
        );
        assert_eq!(
            settings.corner_radius, deserialized.corner_radius,
            "corner radius should round-trip"
        );
        assert_eq!(
            settings.dotted_base_spacing, deserialized.dotted_base_spacing,
            "dotted base spacing should round-trip"
        );
        assert_eq!(
            settings.connection_stroke_width, deserialized.connection_stroke_width,
            "connection stroke width should round-trip"
        );
        assert_eq!(
            settings.shadow_blur, deserialized.shadow_blur,
            "shadow blur should round-trip"
        );
        assert_eq!(
            settings.port_activation_radius, deserialized.port_activation_radius,
            "port activation radius should round-trip"
        );
        assert_eq!(
            settings.const_badge_offset, deserialized.const_badge_offset,
            "const badge offset should round-trip"
        );
    }
}
