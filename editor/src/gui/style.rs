use eframe::egui;
use egui::{Color32, FontFamily, FontId, Shadow, Stroke, Vec2};

use crate::common::connection_bezier::ConnectionBezierStyle;

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
    pub highlight_feather: f32,
    pub broke_clr: Color32,
    pub hover_detection_width: f32,
    pub breaker_stroke: Stroke,
}

#[derive(Debug, Clone)]
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
        const COLOR_PORT_TRIGGER: Color32 = Color32::from_rgb(235, 200, 70);
        const COLOR_PORT_EVENT: Color32 = Color32::from_rgb(235, 140, 70);
        const COLOR_PORT_TRIGGER_HOVER: Color32 = Color32::from_rgb(255, 225, 120);
        const COLOR_PORT_EVENT_HOVER: Color32 = Color32::from_rgb(255, 175, 120);
        const COLOR_STROKE_BREAKER: Color32 = Color32::from_rgb(255, 120, 120);
        const COLOR_STROKE_BROKE: Color32 = Color32::from_rgb(255, 90, 90);
        const COLOR_TEXT: Color32 = Color32::from_rgb(192, 192, 192);
        const COLOR_TEXT_NONINTERACTIVE: Color32 = Color32::from_rgb(140, 140, 140);
        const COLOR_TEXT_CHECKED: Color32 = Color32::from_rgb(60, 50, 20);
        const COLOR_DOT_IMPURE: Color32 = Color32::from_rgb(255, 150, 70);
        const COLOR_SHADOW_EXECUTED: Color32 = Color32::from_rgb(66, 216, 130);
        const COLOR_SHADOW_CACHED: Color32 = Color32::from_rgb(248, 216, 75);
        const COLOR_SHADOW_MISSING: Color32 = Color32::from_rgb(238, 66, 66);
        const COLOR_DOTTED: Color32 = Color32::from_rgb(48, 48, 48);
        const CORNER_RADIUS: f32 = 4.0;
        const SMALL_CORNER_RADIUS: f32 = 2.0;
        const DEFAULT_BG_STROKE_WIDTH: f32 = 1.0;
        const BIG_PADDING: f32 = 6.0;
        const PADDING: f32 = 4.0;
        const SMALL_PADDING: f32 = 2.0;
        const DOTTED_BASE_SPACING: f32 = 24.0;
        const DOTTED_RADIUS_BASE: f32 = 1.2;
        const DOTTED_RADIUS_MIN: f32 = 0.6;
        const DOTTED_RADIUS_MAX: f32 = 2.4;
        const CONNECTION_FEATHER: f32 = 0.8;
        const CONNECTION_STROKE_WIDTH: f32 = 1.5;
        const CONNECTION_HIGHLIGHT_FEATHER: f32 = 3.6;
        const CONNECTION_HOVER_DETECTION_WIDTH: f32 = 6.0;
        const CONNECTION_BREAKER_STROKE_WIDTH: f32 = 2.0;
        const STATUS_DOT_RADIUS: f32 = 4.0;
        const SHADOW_BLUR: u8 = 5;
        const SHADOW_SPREAD: u8 = 2;
        const CACHE_BTN_WIDTH: f32 = 50.0;
        const REMOVE_BTN_SIZE: f32 = 10.0;
        const PORT_RADIUS: f32 = 5.0;
        const PORT_ACTIVATION_RADIUS: f32 = 7.0;
        const PORT_LABEL_SIDE_PADDING: f32 = 8.0;
        const CONST_BADGE_OFFSET: Vec2 = Vec2::new(-15.0, -15.0);

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

            big_padding: scaled(BIG_PADDING),
            padding: scaled(PADDING),
            small_padding: scaled(SMALL_PADDING),
            corner_radius: scaled(CORNER_RADIUS),
            small_corner_radius: scaled(SMALL_CORNER_RADIUS),

            graph_background: GraphBackgroundStyle {
                dotted_color: COLOR_DOTTED,
                dotted_base_spacing: scaled(DOTTED_BASE_SPACING),
                dotted_radius_base: scaled(DOTTED_RADIUS_BASE),
                dotted_radius_min: scaled(DOTTED_RADIUS_MIN),
                dotted_radius_max: scaled(DOTTED_RADIUS_MAX),
                bg_color: COLOR_BG_GRAPH,
            },
            connections: ConnectionStyle {
                feather: scaled(CONNECTION_FEATHER),
                stroke_width: scaled(CONNECTION_STROKE_WIDTH),
                highlight_feather: scaled(CONNECTION_HIGHLIGHT_FEATHER),
                broke_clr: COLOR_STROKE_BROKE,
                hover_detection_width: CONNECTION_HOVER_DETECTION_WIDTH,
                breaker_stroke: Stroke::new(
                    scaled(CONNECTION_BREAKER_STROKE_WIDTH),
                    COLOR_STROKE_BREAKER,
                ),
            },
            node: NodeStyle {
                status_dot_radius: scaled(STATUS_DOT_RADIUS),
                status_impure_color: COLOR_DOT_IMPURE,

                executed_shadow: Shadow {
                    color: COLOR_SHADOW_EXECUTED,
                    offset: [0, 0],
                    blur: scaled_u8(SHADOW_BLUR),
                    spread: scaled_u8(SHADOW_SPREAD),
                },
                cached_shadow: Shadow {
                    color: COLOR_SHADOW_CACHED,
                    offset: [0, 0],
                    blur: scaled_u8(SHADOW_BLUR),
                    spread: scaled_u8(SHADOW_SPREAD),
                },
                missing_inputs_shadow: Shadow {
                    color: COLOR_SHADOW_MISSING,
                    offset: [0, 0],
                    blur: scaled_u8(SHADOW_BLUR),
                    spread: scaled_u8(SHADOW_SPREAD),
                },

                cache_btn_width: scaled(CACHE_BTN_WIDTH),
                remove_btn_size: scaled(REMOVE_BTN_SIZE),

                port_radius: scaled(PORT_RADIUS),
                port_activation_radius: scaled(PORT_ACTIVATION_RADIUS),
                port_label_side_padding: scaled(PORT_LABEL_SIDE_PADDING),
                const_badge_offset: CONST_BADGE_OFFSET * scale,

                input_port_color: COLOR_PORT_INPUT,
                output_port_color: COLOR_PORT_OUTPUT,
                input_hover_color: COLOR_PORT_INPUT_HOVER,
                output_hover_color: COLOR_PORT_OUTPUT_HOVER,

                trigger_port_color: COLOR_PORT_TRIGGER,
                event_port_color: COLOR_PORT_EVENT,
                trigger_hover_color: COLOR_PORT_TRIGGER_HOVER,
                event_hover_color: COLOR_PORT_EVENT_HOVER,

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
