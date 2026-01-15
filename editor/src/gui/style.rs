use std::rc::Rc;

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

#[derive(Debug, Clone)]
pub struct Style {
    style_settings: Rc<StyleSettings>,
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
    pub popup: PopupStyle,
    pub list_button: ButtonStyle,
}

#[derive(Debug, Clone)]
pub struct GraphBackgroundStyle {
    pub bg_color: Color32,
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
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
pub struct MenuStyle {
    pub button_padding: Vec2,
}

#[derive(Debug, Clone)]
pub struct PopupStyle {
    pub fill: Color32,
    pub stroke: Stroke,
    pub corner_radius: f32,
    pub padding: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct ButtonStyle {
    pub disabled_fill: Color32,
    pub idle_fill: Color32,
    pub hover_fill: Color32,
    pub active_fill: Color32,
    pub checked_fill: Color32,
    pub inactive_stroke: Stroke,
    pub hovered_stroke: Stroke,
    pub radius: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct DragValueStyle {
    pub(crate) fill: Color32,
    pub(crate) stroke: Stroke,
    pub(crate) radius: f32,
}

impl Style {
    pub fn new(style_settings: Rc<StyleSettings>, scale: f32) -> Self {
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

        let inactive_bg_stroke = Stroke::new(
            scaled(style_settings.default_bg_stroke_width),
            style_settings.color_stroke_inactive,
        );

        Self {
            scale,
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
            text_color: style_settings.color_text,
            noninteractive_text_color: style_settings.color_text_noninteractive,
            noninteractive_bg_fill: style_settings.color_bg_noninteractive,
            hover_bg_fill: style_settings.color_bg_hover,
            inactive_bg_fill: style_settings.color_bg_inactive,
            inactive_bg_stroke,
            active_bg_stroke: Stroke::new(
                scaled(style_settings.default_bg_stroke_width),
                style_settings.color_stroke_active,
            ),
            active_bg_fill: style_settings.color_bg_active,
            dark_text_color: style_settings.color_text_checked,
            checked_bg_fill: style_settings.color_bg_checked,
            big_padding: scaled(style_settings.big_padding),
            padding: scaled(style_settings.padding),
            small_padding: scaled(style_settings.small_padding),
            corner_radius: scaled(style_settings.corner_radius),
            small_corner_radius: scaled(style_settings.small_corner_radius),
            graph_background: GraphBackgroundStyle {
                dotted_color: style_settings.color_dotted,
                dotted_base_spacing: style_settings.dotted_base_spacing,
                dotted_radius_base: style_settings.dotted_radius_base,
                bg_color: style_settings.color_bg_graph,
            },
            connections: ConnectionStyle {
                feather: style_settings.connection_feather,
                stroke_width: scaled(style_settings.connection_stroke_width),
                highlight_feather: scaled(style_settings.connection_highlight_feather),
                broke_clr: style_settings.color_stroke_broke,
                hover_detection_width: style_settings.connection_hover_detection_width,
                breaker_stroke: Stroke::new(
                    scaled(style_settings.connection_breaker_stroke_width),
                    style_settings.color_stroke_breaker,
                ),
            },
            node: NodeStyle {
                status_dot_radius: scaled(style_settings.status_dot_radius),
                status_impure_color: style_settings.color_dot_impure,
                executed_shadow: Shadow {
                    color: style_settings.color_shadow_executed,
                    offset: [0, 0],
                    blur: scaled_u8(style_settings.shadow_blur),
                    spread: scaled_u8(style_settings.shadow_spread),
                },
                cached_shadow: Shadow {
                    color: style_settings.color_shadow_cached,
                    offset: [0, 0],
                    blur: scaled_u8(style_settings.shadow_blur),
                    spread: scaled_u8(style_settings.shadow_spread),
                },
                missing_inputs_shadow: Shadow {
                    color: style_settings.color_shadow_missing,
                    offset: [0, 0],
                    blur: scaled_u8(style_settings.shadow_blur),
                    spread: scaled_u8(style_settings.shadow_spread),
                },
                cache_btn_width: scaled(style_settings.cache_btn_width),
                remove_btn_size: scaled(style_settings.remove_btn_size),
                port_radius: scaled(style_settings.port_radius),
                port_activation_radius: scaled(style_settings.port_activation_radius),
                port_label_side_padding: scaled(style_settings.port_label_side_padding),
                const_badge_offset: style_settings.const_badge_offset * scale,
                input_port_color: style_settings.color_port_input,
                output_port_color: style_settings.color_port_output,
                input_hover_color: style_settings.color_port_input_hover,
                output_hover_color: style_settings.color_port_output_hover,
                trigger_port_color: style_settings.color_port_trigger,
                event_port_color: style_settings.color_port_event,
                trigger_hover_color: style_settings.color_port_trigger_hover,
                event_hover_color: style_settings.color_port_event_hover,
                const_bind_style: DragValueStyle {
                    fill: style_settings.color_bg_inactive,
                    stroke: inactive_bg_stroke,
                    radius: scaled(style_settings.small_corner_radius),
                },
            },
            menu: MenuStyle {
                button_padding: Vec2::new(scaled(12.0), scaled(3.0)),
            },
            popup: PopupStyle {
                fill: style_settings.color_bg_noninteractive,
                stroke: inactive_bg_stroke,
                corner_radius: scaled(style_settings.corner_radius),
                padding: scaled(style_settings.padding),
            },
            list_button: ButtonStyle {
                disabled_fill: Color32::TRANSPARENT,
                idle_fill: Color32::TRANSPARENT,
                hover_fill: style_settings.color_bg_hover,
                active_fill: style_settings.color_bg_active,
                checked_fill: Color32::TRANSPARENT,
                inactive_stroke: Stroke::NONE,
                hovered_stroke: Stroke::NONE,
                radius: 0.0,
            },
            style_settings,
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        *self = Self::new(Rc::clone(&self.style_settings), scale);
    }

    pub fn apply_to_egui(&self, egui_style: &mut egui::Style) {
        egui_style.spacing.item_spacing = Vec2::splat(self.padding);
        egui_style.spacing.button_padding = Vec2::new(self.padding, self.small_padding);
        egui_style.spacing.indent = self.padding;
        egui_style.spacing.window_margin = egui::Margin::same(self.padding as i8);

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
