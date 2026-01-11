use eframe::egui;
use egui::{Color32, FontFamily, FontId, Shadow, Stroke, Vec2};

pub fn brighten(color: Color32, amount: f32) -> Color32 {
    let t = amount.clamp(0.0, 1.0);
    let lerp = |c: u8| -> u8 {
        let c = c as f32;
        (c + (255.0 - c) * t).round().clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_unmultiplied(lerp(color.r()), lerp(color.g()), lerp(color.b()), color.a())
}

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
}

#[derive(Debug, Clone)]
pub struct GraphBackgroundStyle {
    pub bg_color: Color32,
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
    pub dotted_radius_min: f32,
    pub dotted_radius_max: f32,
}

#[derive(Debug, Clone)]
pub struct ConnectionStyle {
    pub feather: f32,
    pub stroke_width: f32,
    pub broke_clr: Color32,
    pub hover_detection_width: f32,
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
    pub const_badge_offset: Vec2,

    pub input_port_color: Color32,
    pub output_port_color: Color32,
    pub input_hover_color: Color32,
    pub output_hover_color: Color32,

    pub const_stroke_width: f32,
    pub const_bind_style: DragValueStyle,
}

#[derive(Debug, Clone)]
pub(crate) struct DragValueStyle {
    pub(crate) fill: Color32,
    pub(crate) stroke: Stroke,
    pub(crate) radius: f32,
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

        const COLOR_BG_NONINTERACTIVE: Color32 = Color32::from_rgb(35, 35, 35);
        const COLOR_BG_INACTIVE: Color32 = Color32::from_rgb(40, 40, 40);
        const COLOR_BG_GRAPH: Color32 = Color32::from_rgb(16, 16, 16);
        const COLOR_BG_HOVER: Color32 = Color32::from_rgb(50, 50, 50);
        const COLOR_BG_ACTIVE: Color32 = Color32::from_rgb(60, 60, 60);
        const COLOR_BG_CHECKED: Color32 = Color32::from_rgb(240, 205, 90);
        const COLOR_STROKE_INACTIVE: Color32 = Color32::from_rgb(65, 65, 65);
        const COLOR_STROKE_ACTIVE: Color32 = Color32::from_rgb(128, 128, 128);
        const COLOR_PORT_INPUT: Color32 = Color32::from_rgb(70, 150, 255);
        const COLOR_PORT_OUTPUT: Color32 = Color32::from_rgb(70, 200, 200);
        const COLOR_PORT_INPUT_HOVER: Color32 = Color32::from_rgb(120, 190, 255);
        const COLOR_PORT_OUTPUT_HOVER: Color32 = Color32::from_rgb(110, 230, 210);
        const COLOR_STROKE_BREAKER: Color32 = Color32::from_rgb(255, 120, 120);
        const COLOR_STROKE_BROKE: Color32 = Color32::from_rgb(255, 90, 90);
        const COLOR_TEXT: Color32 = Color32::from_rgb(192, 192, 192);
        const COLOR_TEXT_NONINTERACTIVE: Color32 = Color32::from_rgb(140, 140, 140);
        const COLOR_TEXT_CHECKED: Color32 = Color32::from_rgb(60, 50, 20);
        const COLOR_DOT_TERMINAL: Color32 = Color32::from_rgb(128, 128, 128);
        const COLOR_DOT_IMPURE: Color32 = Color32::from_rgb(255, 150, 70);
        const COLOR_SHADOW_EXECUTED: Color32 = Color32::from_rgb(66, 216, 130);
        const COLOR_SHADOW_CACHED: Color32 = Color32::from_rgb(248, 216, 75);
        const COLOR_SHADOW_MISSING: Color32 = Color32::from_rgb(238, 66, 66);
        const COLOR_DOTTED: Color32 = Color32::from_rgb(48, 48, 48);
        const CORNER_RADIUS: f32 = 4.0;
        const SMALL_CORNER_RADIUS: f32 = 2.0;
        const DEFAULT_BG_STROKE_WIDTH: f32 = 1.0;

        let inactive_bg_stroke =
            Stroke::new(scaled(DEFAULT_BG_STROKE_WIDTH), COLOR_STROKE_INACTIVE);

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

            text_color: COLOR_TEXT,
            noninteractive_text_color: COLOR_TEXT_NONINTERACTIVE,

            noninteractive_bg_fill: COLOR_BG_NONINTERACTIVE,
            hover_bg_fill: COLOR_BG_HOVER,
            inactive_bg_fill: COLOR_BG_INACTIVE,
            inactive_bg_stroke,
            active_bg_stroke: Stroke::new(scaled(DEFAULT_BG_STROKE_WIDTH), COLOR_STROKE_ACTIVE),
            active_bg_fill: COLOR_BG_ACTIVE,

            dark_text_color: COLOR_TEXT_CHECKED,
            checked_bg_fill: COLOR_BG_CHECKED,

            big_padding: scaled(6.0),
            padding: scaled(4.0),
            small_padding: scaled(2.0),
            corner_radius: scaled(CORNER_RADIUS),
            small_corner_radius: scaled(SMALL_CORNER_RADIUS),

            graph_background: GraphBackgroundStyle {
                dotted_color: COLOR_DOTTED,
                dotted_base_spacing: scaled(24.0),
                dotted_radius_base: scaled(1.2),
                dotted_radius_min: scaled(0.6),
                dotted_radius_max: scaled(2.4),
                bg_color: COLOR_BG_GRAPH,
            },
            connections: ConnectionStyle {
                feather: scaled(0.8),
                stroke_width: scaled(1.5),
                broke_clr: COLOR_STROKE_BROKE,
                hover_detection_width: 6.0,
                breaker_stroke: Stroke::new(scaled(2.0), COLOR_STROKE_BREAKER),
            },
            node: NodeStyle {
                const_stroke_width: scaled(1.0),

                status_dot_radius: scaled(4.0),
                status_terminal_color: COLOR_DOT_TERMINAL,
                status_impure_color: COLOR_DOT_IMPURE,

                executed_shadow: Shadow {
                    color: COLOR_SHADOW_EXECUTED,
                    offset: [0, 0],
                    blur: scaled_u8(5),
                    spread: scaled_u8(2),
                },
                cached_shadow: Shadow {
                    color: COLOR_SHADOW_CACHED,
                    offset: [0, 0],
                    blur: scaled_u8(5),
                    spread: scaled_u8(2),
                },
                missing_inputs_shadow: Shadow {
                    color: COLOR_SHADOW_MISSING,
                    offset: [0, 0],
                    blur: scaled_u8(5),
                    spread: scaled_u8(2),
                },

                cache_btn_width: scaled(50.0),
                remove_btn_size: scaled(10.0),

                port_radius: scaled(18.0 * 0.3),
                port_activation_radius: scaled(18.0 * 0.3 * 1.3),
                port_label_side_padding: scaled(8.0),
                const_badge_offset: Vec2::new(scaled(-15.0), scaled(-25.0)),

                input_port_color: COLOR_PORT_INPUT,
                output_port_color: COLOR_PORT_OUTPUT,
                input_hover_color: COLOR_PORT_INPUT_HOVER,
                output_hover_color: COLOR_PORT_OUTPUT_HOVER,

                const_bind_style: DragValueStyle {
                    fill: COLOR_BG_INACTIVE,
                    stroke: inactive_bg_stroke,
                    radius: scaled(SMALL_CORNER_RADIUS),
                },
            },
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        *self = Self::new(scale);
    }

    pub fn apply(&self, egui_style: &mut egui::Style) {
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
}
