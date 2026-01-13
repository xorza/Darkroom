use eframe::egui;
use egui::{Color32, FontFamily, FontId, Shadow, Stroke, Vec2};

use crate::{common::connection_bezier::ConnectionBezierStyle, gui::style_settings::StyleSettings};

pub fn brighten(color: Color32, amount: f32) -> Color32 {
    let t = amount.clamp(0.0, 1.0);
    let lerp = |c: u8| -> u8 {
        let c = c as f32;
        (c + (255.0 - c) * t).round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_unmultiplied(lerp(color.r()), lerp(color.g()), lerp(color.b()), color.a())
}

#[derive(Debug, Clone, Default)]
pub struct Style {
    style_settings: StyleSettings,
    scale: f32,

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

    pub big_padding: f32,
    pub padding: f32,
    pub small_padding: f32,
    pub corner_radius: f32,
    pub small_corner_radius: f32,

    pub checked_bg_fill: Color32,
    pub dark_text_color: Color32,

    pub graph_background: GraphBackgroundStyle,
    pub connections: ConnectionStyle,
    pub node: NodeStyle,
    pub menu: MenuStyle,
}

#[derive(Debug, Clone, Default)]
pub struct GraphBackgroundStyle {
    pub bg_color: Color32,
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionStyle {
    pub feather: f32,
    pub stroke_width: f32,
    pub highlight_feather: f32,
    pub broke_clr: Color32,
    pub hover_detection_width: f32,
    pub breaker_stroke: Stroke,
}

#[derive(Debug, Clone, Default)]
pub struct NodeStyle {
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
    pub const_badge_offset: Vec2,

    pub input_port_color: Color32,
    pub output_port_color: Color32,
    pub input_hover_color: Color32,
    pub output_hover_color: Color32,

    pub trigger_port_color: Color32,
    pub event_port_color: Color32,
    pub trigger_hover_color: Color32,
    pub event_hover_color: Color32,

    pub const_bind_style: DragValueStyle,
}

#[derive(Debug, Clone, Default)]
pub struct MenuStyle {
    pub button_padding: Vec2,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DragValueStyle {
    pub(crate) fill: Color32,
    pub(crate) stroke: Stroke,
    pub(crate) radius: f32,
}

impl Style {
    pub fn new(settings: StyleSettings, scale: f32) -> Self {
        assert!(scale.is_finite(), "style scale must be finite");
        assert!(scale > 0.0, "style scale must be greater than 0");
        let mut result = Self {
            style_settings: settings,
            scale,
            ..Default::default()
        };

        result.apply();

        result
    }

    pub fn set_scale(&mut self, scale: f32) {
        assert!(scale.is_finite(), "style scale must be finite");
        assert!(scale > 0.0, "style scale must be greater than 0");

        self.scale = scale;
        self.apply();
    }

    pub fn apply(&mut self) {
        let scale = self.scale;
        assert!(scale.is_finite(), "style scale must be finite");
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

        let settings = &self.style_settings;
        let inactive_bg_stroke = Stroke::new(
            scaled(settings.default_bg_stroke_width),
            settings.color_stroke_inactive,
        );

        self.heading_font = FontId {
            size: scaled(18.0),
            family: FontFamily::Proportional,
        };
        self.body_font = FontId {
            size: scaled(15.0),
            family: FontFamily::Proportional,
        };
        self.sub_font = FontId {
            size: scaled(13.0),
            family: FontFamily::Proportional,
        };
        self.mono_font = FontId {
            size: scaled(13.0),
            family: FontFamily::Monospace,
        };

        self.text_color = settings.color_text;
        self.noninteractive_text_color = settings.color_text_noninteractive;

        self.noninteractive_bg_fill = settings.color_bg_noninteractive;
        self.hover_bg_fill = settings.color_bg_hover;
        self.inactive_bg_fill = settings.color_bg_inactive;
        self.inactive_bg_stroke = inactive_bg_stroke;
        self.active_bg_stroke = Stroke::new(
            scaled(settings.default_bg_stroke_width),
            settings.color_stroke_active,
        );
        self.active_bg_fill = settings.color_bg_active;

        self.dark_text_color = settings.color_text_checked;
        self.checked_bg_fill = settings.color_bg_checked;

        self.big_padding = scaled(settings.big_padding);
        self.padding = scaled(settings.padding);
        self.small_padding = scaled(settings.small_padding);
        self.corner_radius = scaled(settings.corner_radius);
        self.small_corner_radius = scaled(settings.small_corner_radius);

        self.graph_background = GraphBackgroundStyle {
            dotted_color: settings.color_dotted,
            dotted_base_spacing: scaled(settings.dotted_base_spacing),
            dotted_radius_base: scaled(settings.dotted_radius_base),
            dotted_radius_min: scaled(settings.dotted_radius_min),
            dotted_radius_max: scaled(settings.dotted_radius_max),
            bg_color: settings.color_bg_graph,
        };
        self.connections = ConnectionStyle {
            feather: settings.connection_feather,
            stroke_width: scaled(settings.connection_stroke_width),
            highlight_feather: scaled(settings.connection_highlight_feather),
            broke_clr: settings.color_stroke_broke,
            hover_detection_width: settings.connection_hover_detection_width,
            breaker_stroke: Stroke::new(
                scaled(settings.connection_breaker_stroke_width),
                settings.color_stroke_breaker,
            ),
        };
        self.node = NodeStyle {
            status_dot_radius: scaled(settings.status_dot_radius),
            status_impure_color: settings.color_dot_impure,

            executed_shadow: Shadow {
                color: settings.color_shadow_executed,
                offset: [0, 0],
                blur: scaled_u8(settings.shadow_blur),
                spread: scaled_u8(settings.shadow_spread),
            },
            cached_shadow: Shadow {
                color: settings.color_shadow_cached,
                offset: [0, 0],
                blur: scaled_u8(settings.shadow_blur),
                spread: scaled_u8(settings.shadow_spread),
            },
            missing_inputs_shadow: Shadow {
                color: settings.color_shadow_missing,
                offset: [0, 0],
                blur: scaled_u8(settings.shadow_blur),
                spread: scaled_u8(settings.shadow_spread),
            },

            cache_btn_width: scaled(settings.cache_btn_width),
            remove_btn_size: scaled(settings.remove_btn_size),

            port_radius: scaled(settings.port_radius),
            port_activation_radius: scaled(settings.port_activation_radius),
            port_label_side_padding: scaled(settings.port_label_side_padding),
            const_badge_offset: settings.const_badge_offset * scale,

            input_port_color: settings.color_port_input,
            output_port_color: settings.color_port_output,
            input_hover_color: settings.color_port_input_hover,
            output_hover_color: settings.color_port_output_hover,

            trigger_port_color: settings.color_port_trigger,
            event_port_color: settings.color_port_event,
            trigger_hover_color: settings.color_port_trigger_hover,
            event_hover_color: settings.color_port_event_hover,

            const_bind_style: DragValueStyle {
                fill: settings.color_bg_inactive,
                stroke: inactive_bg_stroke,
                radius: scaled(settings.small_corner_radius),
            },
        };
        self.menu = MenuStyle {
            button_padding: Vec2::new(scaled(12.0), scaled(3.0)),
        };
    }

    pub fn apply_to_egui(&self, egui_style: &mut egui::Style) {
        let visuals = &mut egui_style.visuals;
        visuals.dark_mode = true;
        visuals.override_text_color = Some(self.text_color);
        visuals.window_fill = self.noninteractive_bg_fill;
        visuals.panel_fill = self.noninteractive_bg_fill;
        visuals.faint_bg_color = self.inactive_bg_fill;
        visuals.extreme_bg_color = self.active_bg_fill;
        visuals.code_bg_color = self.inactive_bg_fill;
        visuals.text_edit_bg_color = Some(self.active_bg_fill);
        visuals.selection.bg_fill = self.checked_bg_fill;
        visuals.selection.stroke = Stroke::new(self.active_bg_stroke.width, self.dark_text_color);

        visuals.widgets.noninteractive.bg_fill = self.noninteractive_bg_fill;
        visuals.widgets.noninteractive.bg_stroke = self.inactive_bg_stroke;
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(
            self.inactive_bg_stroke.width,
            self.noninteractive_text_color,
        );

        visuals.widgets.inactive.bg_fill = self.inactive_bg_fill;
        visuals.widgets.inactive.bg_stroke = self.inactive_bg_stroke;
        visuals.widgets.inactive.fg_stroke =
            Stroke::new(self.inactive_bg_stroke.width, self.text_color);

        visuals.widgets.hovered.bg_fill = self.hover_bg_fill;
        visuals.widgets.hovered.bg_stroke = self.active_bg_stroke;
        visuals.widgets.hovered.fg_stroke =
            Stroke::new(self.active_bg_stroke.width, self.text_color);

        visuals.widgets.active.bg_fill = self.active_bg_fill;
        visuals.widgets.active.bg_stroke = self.active_bg_stroke;
        visuals.widgets.active.fg_stroke =
            Stroke::new(self.active_bg_stroke.width, self.text_color);

        visuals.widgets.open = visuals.widgets.active;
        visuals.hyperlink_color = self.text_color;
        visuals.window_stroke = self.inactive_bg_stroke;
        visuals.window_corner_radius = self.corner_radius.into();
        visuals.menu_corner_radius = self.small_corner_radius.into();
    }

    pub fn apply_menu_style(&self, ui: &mut egui::Ui) {
        let style = ui.style_mut();
        style.spacing.button_padding = self.menu.button_padding;
    }
}
