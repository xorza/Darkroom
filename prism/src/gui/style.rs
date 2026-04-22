//! Project-wide visual styling. A single `Style` type serves both as
//! the serialized reference (loaded from `style.toml` at scale=1.0)
//! and as the runtime view at the current scale. Scale-dependent
//! values are baked into the `Style` instance; to render at a
//! different scale, [`Style::at_scale`] produces a fresh `Rc<Style>`
//! cloned from the reference with scale-dependent fields multiplied.
//!
//! The runtime `Style` carries a back-link to its reference
//! ([`Style::reference`]) so subsequent `at_scale` calls always
//! multiply from the canonical scale=1.0 values — no float drift from
//! chained scalings.
//!
//! Fields are grouped to match how callers access them:
//! - top-level: fonts, palette, common paddings/radii;
//! - sub-structs (`GraphBackgroundStyle`, `ConnectionStyle`,
//!   `NodeStyle`, `PopupStyle`, `ButtonStyle`) for
//!   cluster-specific concerns.

use std::path::Path;
use std::rc::Rc;

use eframe::egui;
use egui::{Color32, FontFamily, FontId, Margin, Shadow, Stroke, Vec2};
use serde::{Deserialize, Serialize};

use crate::common::UiEquals;
use crate::gui::connection_ui::PortKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub heading_font: FontId,
    pub body_font: FontId,
    pub sub_font: FontId,
    pub mono_font: FontId,

    #[serde(with = "color_hex")]
    pub text_color: Color32,
    #[serde(with = "color_hex")]
    pub noninteractive_text_color: Color32,
    #[serde(with = "color_hex")]
    pub dark_text_color: Color32,

    #[serde(with = "color_hex")]
    pub noninteractive_bg_fill: Color32,
    #[serde(with = "color_hex")]
    pub active_bg_fill: Color32,
    #[serde(with = "color_hex")]
    pub hover_bg_fill: Color32,
    #[serde(with = "color_hex")]
    pub inactive_bg_fill: Color32,
    #[serde(with = "color_hex")]
    pub checked_bg_fill: Color32,

    pub inactive_bg_stroke: Stroke,
    pub active_bg_stroke: Stroke,

    pub big_padding: f32,
    pub padding: f32,
    pub small_padding: f32,
    pub corner_radius: f32,
    pub small_corner_radius: f32,

    pub graph_background: GraphBackgroundStyle,
    pub connections: ConnectionStyle,
    pub node: NodeStyle,
    pub menu: MenuStyle,
    pub popup: PopupStyle,
    pub list_button: ButtonStyle,

    /// Back-link to the reference copy at scale=1.0. `None` when
    /// `self` *is* the reference. Runtime instances produced by
    /// [`Style::at_scale`] carry `Some` so subsequent rescales
    /// always multiply from canonical values (no chained float
    /// drift, no lossy round-trips through `Shadow`'s `u8`).
    #[serde(skip)]
    reference: Option<Rc<Style>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphBackgroundStyle {
    #[serde(with = "color_hex")]
    pub bg_color: Color32,
    #[serde(with = "color_hex")]
    pub dotted_color: Color32,
    pub dotted_base_spacing: f32,
    pub dotted_radius_base: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionStyle {
    pub feather: f32,
    pub stroke_width: f32,
    pub highlight_feather: f32,
    #[serde(with = "color_hex")]
    pub broke_clr: Color32,
    pub hover_detection_width: f32,
    pub breaker_stroke: Stroke,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeStyle {
    #[serde(with = "color_hex")]
    pub status_impure_color: Color32,
    pub status_dot_radius: f32,

    pub shadow: Shadow,
    pub executed_shadow: Shadow,
    pub cached_shadow: Shadow,
    pub missing_inputs_shadow: Shadow,
    pub errored_shadow: Shadow,

    pub cache_btn_width: f32,
    pub remove_btn_size: f32,

    pub port_radius: f32,
    pub port_activation_radius: f32,
    pub port_label_side_padding: f32,
    #[serde(with = "vec2_array")]
    pub const_badge_offset: Vec2,

    #[serde(with = "color_hex")]
    pub input_port_color: Color32,
    #[serde(with = "color_hex")]
    pub output_port_color: Color32,
    #[serde(with = "color_hex")]
    pub input_hover_color: Color32,
    #[serde(with = "color_hex")]
    pub output_hover_color: Color32,

    #[serde(with = "color_hex")]
    pub trigger_port_color: Color32,
    #[serde(with = "color_hex")]
    pub event_port_color: Color32,
    #[serde(with = "color_hex")]
    pub trigger_hover_color: Color32,
    #[serde(with = "color_hex")]
    pub event_hover_color: Color32,

    pub const_bind_style: DragValueStyle,
}

/// Styling for the top-level menu bar + its dropdown entries: font,
/// per-entry padding, popup width, and the button preset shared
/// between the trigger button and each entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuStyle {
    pub font: FontId,
    #[serde(with = "vec2_array")]
    pub padding: Vec2,
    pub popup_min_width: f32,
    pub button: ButtonStyle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PopupStyle {
    #[serde(with = "color_hex")]
    pub fill: Color32,
    pub stroke: Stroke,
    pub corner_radius: f32,
    pub padding: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonStyle {
    #[serde(with = "color_hex")]
    pub disabled_fill: Color32,
    #[serde(with = "color_hex")]
    pub idle_fill: Color32,
    #[serde(with = "color_hex")]
    pub hover_fill: Color32,
    #[serde(with = "color_hex")]
    pub active_fill: Color32,
    #[serde(with = "color_hex")]
    pub checked_fill: Color32,
    pub inactive_stroke: Stroke,
    pub hovered_stroke: Stroke,
    pub radius: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DragValueStyle {
    #[serde(with = "color_hex")]
    pub(crate) fill: Color32,
    pub(crate) stroke: Stroke,
    pub(crate) radius: f32,
}

/// Port colors for a specific port kind (base and hover variants).
#[derive(Debug, Clone, Copy)]
pub struct PortColors {
    pub base: Color32,
    pub hover: Color32,
}

impl PortColors {
    pub fn select(self, hovered: bool) -> Color32 {
        if hovered { self.hover } else { self.base }
    }
}

impl NodeStyle {
    pub fn port_colors(&self, kind: PortKind) -> PortColors {
        match kind {
            PortKind::Input => PortColors {
                base: self.input_port_color,
                hover: self.input_hover_color,
            },
            PortKind::Output => PortColors {
                base: self.output_port_color,
                hover: self.output_hover_color,
            },
            PortKind::Trigger => PortColors {
                base: self.trigger_port_color,
                hover: self.trigger_hover_color,
            },
            PortKind::Event => PortColors {
                base: self.event_port_color,
                hover: self.event_hover_color,
            },
        }
    }
}

impl Style {
    /// Load a `Style` reference from a TOML file. The loaded values
    /// are at scale=1.0. Returns a fresh reference (no back-link).
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let payload = std::fs::read_to_string(path)?;
        let style: Style = toml::from_str(&payload)?;
        Ok(style)
    }

    /// Produce a runtime `Style` at the given scale, cloned from the
    /// reference. `self` can be either a reference (no back-link) or
    /// an already-scaled runtime instance — either way the multiply
    /// starts from the canonical scale=1.0 values.
    pub fn at_scale(self: &Rc<Self>, scale: f32) -> Rc<Self> {
        assert!(scale.is_finite(), "style scale must be finite");
        assert!(scale > 0.0, "style scale must be greater than 0");

        // Always multiply from the reference. If `self` is itself the
        // reference, use it; otherwise follow the back-link.
        let reference: &Rc<Self> = self.reference.as_ref().unwrap_or(self);

        // Fast-path: at scale=1.0 the reference already holds every
        // value at the right magnitude — no multiplication needed.
        if scale.ui_equals(1.0) {
            return Rc::clone(reference);
        }

        let r = reference.as_ref();

        Rc::new(Style {
            heading_font: scale_font(&r.heading_font, scale),
            body_font: scale_font(&r.body_font, scale),
            sub_font: scale_font(&r.sub_font, scale),
            mono_font: scale_font(&r.mono_font, scale),

            text_color: r.text_color,
            noninteractive_text_color: r.noninteractive_text_color,
            dark_text_color: r.dark_text_color,

            noninteractive_bg_fill: r.noninteractive_bg_fill,
            active_bg_fill: r.active_bg_fill,
            hover_bg_fill: r.hover_bg_fill,
            inactive_bg_fill: r.inactive_bg_fill,
            checked_bg_fill: r.checked_bg_fill,

            inactive_bg_stroke: scale_stroke(&r.inactive_bg_stroke, scale),
            active_bg_stroke: scale_stroke(&r.active_bg_stroke, scale),

            big_padding: r.big_padding * scale,
            padding: r.padding * scale,
            small_padding: r.small_padding * scale,
            corner_radius: r.corner_radius * scale,
            small_corner_radius: r.small_corner_radius * scale,

            graph_background: GraphBackgroundStyle {
                bg_color: r.graph_background.bg_color,
                dotted_color: r.graph_background.dotted_color,
                dotted_base_spacing: r.graph_background.dotted_base_spacing,
                dotted_radius_base: r.graph_background.dotted_radius_base,
            },
            connections: ConnectionStyle {
                feather: r.connections.feather,
                stroke_width: r.connections.stroke_width * scale,
                highlight_feather: r.connections.highlight_feather * scale,
                broke_clr: r.connections.broke_clr,
                hover_detection_width: r.connections.hover_detection_width,
                breaker_stroke: scale_stroke(&r.connections.breaker_stroke, scale),
            },
            node: NodeStyle {
                status_impure_color: r.node.status_impure_color,
                status_dot_radius: r.node.status_dot_radius * scale,
                shadow: scale_shadow(&r.node.shadow, scale),
                executed_shadow: scale_shadow(&r.node.executed_shadow, scale),
                cached_shadow: scale_shadow(&r.node.cached_shadow, scale),
                missing_inputs_shadow: scale_shadow(&r.node.missing_inputs_shadow, scale),
                errored_shadow: scale_shadow(&r.node.errored_shadow, scale),
                cache_btn_width: r.node.cache_btn_width * scale,
                remove_btn_size: r.node.remove_btn_size * scale,
                port_radius: r.node.port_radius * scale,
                port_activation_radius: r.node.port_activation_radius * scale,
                port_label_side_padding: r.node.port_label_side_padding * scale,
                const_badge_offset: r.node.const_badge_offset * scale,
                input_port_color: r.node.input_port_color,
                output_port_color: r.node.output_port_color,
                input_hover_color: r.node.input_hover_color,
                output_hover_color: r.node.output_hover_color,
                trigger_port_color: r.node.trigger_port_color,
                event_port_color: r.node.event_port_color,
                trigger_hover_color: r.node.trigger_hover_color,
                event_hover_color: r.node.event_hover_color,
                const_bind_style: DragValueStyle {
                    fill: r.node.const_bind_style.fill,
                    stroke: scale_stroke(&r.node.const_bind_style.stroke, scale),
                    radius: r.node.const_bind_style.radius * scale,
                },
            },
            menu: MenuStyle {
                font: scale_font(&r.menu.font, scale),
                padding: r.menu.padding * scale,
                popup_min_width: r.menu.popup_min_width * scale,
                button: scale_button_style(&r.menu.button, scale),
            },
            popup: PopupStyle {
                fill: r.popup.fill,
                stroke: scale_stroke(&r.popup.stroke, scale),
                corner_radius: r.popup.corner_radius * scale,
                padding: r.popup.padding * scale,
            },
            list_button: scale_button_style(&r.list_button, scale),

            reference: Some(Rc::clone(reference)),
        })
    }

    pub fn apply_to_egui(&self, egui_style: &mut egui::Style) {
        egui_style.spacing.item_spacing = Vec2::splat(self.padding);
        egui_style.spacing.button_padding = Vec2::new(self.padding, self.small_padding);
        egui_style.spacing.indent = self.padding;
        egui_style.spacing.window_margin = Margin::same(self.padding as i8);

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

fn scale_button_style(button: &ButtonStyle, scale: f32) -> ButtonStyle {
    ButtonStyle {
        disabled_fill: button.disabled_fill,
        idle_fill: button.idle_fill,
        hover_fill: button.hover_fill,
        active_fill: button.active_fill,
        checked_fill: button.checked_fill,
        inactive_stroke: scale_stroke(&button.inactive_stroke, scale),
        hovered_stroke: scale_stroke(&button.hovered_stroke, scale),
        radius: button.radius * scale,
    }
}

fn scale_font(font: &FontId, scale: f32) -> FontId {
    FontId {
        size: font.size * scale,
        family: font.family.clone(),
    }
}

fn scale_stroke(stroke: &Stroke, scale: f32) -> Stroke {
    Stroke::new(stroke.width * scale, stroke.color)
}

fn scale_shadow(shadow: &Shadow, scale: f32) -> Shadow {
    Shadow {
        offset: [
            (shadow.offset[0] as f32 * scale).ceil() as i8,
            (shadow.offset[1] as f32 * scale).ceil() as i8,
        ],
        blur: (shadow.blur as f32 * scale).ceil() as u8,
        spread: (shadow.spread as f32 * scale).ceil() as u8,
        color: shadow.color,
    }
}

// Shared default values used by multiple sub-struct `Default` impls.
const SMALL_CORNER_RADIUS: f32 = 2.0;
fn default_inactive_stroke() -> Stroke {
    Stroke::new(1.0, Color32::from_rgb(65, 65, 65))
}
fn default_active_stroke() -> Stroke {
    Stroke::new(1.0, Color32::from_rgb(128, 128, 128))
}

impl Default for Style {
    fn default() -> Self {
        Self {
            heading_font: FontId {
                size: 18.0,
                family: FontFamily::Proportional,
            },
            body_font: FontId {
                size: 15.0,
                family: FontFamily::Proportional,
            },
            sub_font: FontId {
                size: 13.0,
                family: FontFamily::Proportional,
            },
            mono_font: FontId {
                size: 12.0,
                family: FontFamily::Monospace,
            },

            text_color: Color32::from_rgb(192, 192, 192),
            noninteractive_text_color: Color32::from_rgb(140, 140, 140),
            dark_text_color: Color32::from_rgb(60, 50, 20),

            noninteractive_bg_fill: Color32::from_rgb(35, 35, 35),
            active_bg_fill: Color32::from_rgb(60, 60, 60),
            hover_bg_fill: Color32::from_rgb(50, 50, 50),
            inactive_bg_fill: Color32::from_rgb(40, 40, 40),
            checked_bg_fill: Color32::from_rgb(240, 205, 90),

            inactive_bg_stroke: default_inactive_stroke(),
            active_bg_stroke: default_active_stroke(),

            big_padding: 5.0,
            padding: 4.0,
            small_padding: 2.0,
            corner_radius: 4.0,
            small_corner_radius: SMALL_CORNER_RADIUS,

            graph_background: GraphBackgroundStyle::default(),
            connections: ConnectionStyle::default(),
            node: NodeStyle::default(),
            menu: MenuStyle::default(),
            popup: PopupStyle::default(),
            list_button: ButtonStyle::default(),

            reference: None,
        }
    }
}

impl Default for GraphBackgroundStyle {
    fn default() -> Self {
        Self {
            bg_color: Color32::from_rgb(16, 16, 16),
            dotted_color: Color32::from_rgb(48, 48, 48),
            dotted_base_spacing: 24.0,
            dotted_radius_base: 1.2,
        }
    }
}

impl Default for ConnectionStyle {
    fn default() -> Self {
        Self {
            feather: 0.8,
            stroke_width: 1.5,
            highlight_feather: 3.6,
            broke_clr: Color32::from_rgb(255, 90, 90),
            hover_detection_width: 6.0,
            breaker_stroke: Stroke::new(2.0, Color32::from_rgb(255, 120, 120)),
        }
    }
}

impl Default for MenuStyle {
    fn default() -> Self {
        Self {
            font: FontId {
                size: 15.0,
                family: FontFamily::Proportional,
            },
            padding: Vec2::new(12.0, 3.0),
            popup_min_width: 100.0,
            button: ButtonStyle::default(),
        }
    }
}

impl Default for NodeStyle {
    fn default() -> Self {
        let status_shadow = |color: Color32| Shadow {
            color,
            offset: [0, 0],
            blur: 6,
            spread: 2,
        };
        Self {
            status_impure_color: Color32::from_rgb(255, 150, 70),
            status_dot_radius: 4.0,
            shadow: Shadow {
                offset: [3, 4],
                blur: 10,
                spread: 5,
                color: Color32::from_black_alpha(96),
            },
            executed_shadow: status_shadow(Color32::from_rgb(66, 216, 130)),
            cached_shadow: status_shadow(Color32::from_rgb(100, 160, 255)),
            missing_inputs_shadow: status_shadow(Color32::from_rgb(255, 180, 70)),
            errored_shadow: status_shadow(Color32::from_rgb(238, 66, 66)),
            cache_btn_width: 50.0,
            remove_btn_size: 10.0,
            port_radius: 5.0,
            port_activation_radius: 7.0,
            port_label_side_padding: 8.0,
            const_badge_offset: Vec2::new(-10.0, 0.0),
            input_port_color: Color32::from_rgb(70, 150, 255),
            output_port_color: Color32::from_rgb(70, 200, 200),
            input_hover_color: Color32::from_rgb(120, 190, 255),
            output_hover_color: Color32::from_rgb(110, 230, 210),
            trigger_port_color: Color32::from_rgb(235, 200, 70),
            event_port_color: Color32::from_rgb(235, 140, 70),
            trigger_hover_color: Color32::from_rgb(255, 225, 120),
            event_hover_color: Color32::from_rgb(255, 175, 120),
            const_bind_style: DragValueStyle::default(),
        }
    }
}

impl Default for PopupStyle {
    fn default() -> Self {
        Self {
            fill: Color32::from_rgb(35, 35, 35),
            stroke: default_inactive_stroke(),
            corner_radius: 4.0,
            padding: 4.0,
        }
    }
}

impl Default for ButtonStyle {
    fn default() -> Self {
        // The list-button variant (transparent fills, no strokes).
        Self {
            disabled_fill: Color32::TRANSPARENT,
            idle_fill: Color32::TRANSPARENT,
            hover_fill: Color32::from_rgb(50, 50, 50),
            active_fill: Color32::from_rgb(60, 60, 60),
            checked_fill: Color32::TRANSPARENT,
            inactive_stroke: Stroke::NONE,
            hovered_stroke: Stroke::NONE,
            radius: 0.0,
        }
    }
}

impl Default for DragValueStyle {
    fn default() -> Self {
        Self {
            fill: Color32::from_rgb(40, 40, 40),
            stroke: default_inactive_stroke(),
            radius: SMALL_CORNER_RADIUS,
        }
    }
}

mod color_hex {
    use egui::Color32;
    use serde::de::{self, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S>(color: &Color32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex = format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            color.r(),
            color.g(),
            color.b(),
            color.a()
        );
        serializer.serialize_str(&hex)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Color32, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorVisitor)
    }

    struct ColorVisitor;

    impl<'de> Visitor<'de> for ColorVisitor {
        type Value = Color32;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a color hex string like #RRGGBBAA")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            parse_hex_color(value).map_err(de::Error::custom)
        }

        fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
            self.visit_str(&value)
        }
    }

    fn parse_hex_color(value: &str) -> Result<Color32, String> {
        let value = value.trim();
        let hex = value.strip_prefix('#').unwrap_or(value);
        match hex.len() {
            6 => {
                let r = parse_pair(&hex[0..2])?;
                let g = parse_pair(&hex[2..4])?;
                let b = parse_pair(&hex[4..6])?;
                Ok(Color32::from_rgba_unmultiplied(r, g, b, 255))
            }
            8 => {
                let r = parse_pair(&hex[0..2])?;
                let g = parse_pair(&hex[2..4])?;
                let b = parse_pair(&hex[4..6])?;
                let a = parse_pair(&hex[6..8])?;
                Ok(Color32::from_rgba_unmultiplied(r, g, b, a))
            }
            _ => Err(format!(
                "expected 6 or 8 hex digits for color, got {}",
                hex.len()
            )),
        }
    }

    fn parse_pair(value: &str) -> Result<u8, String> {
        u8::from_str_radix(value, 16).map_err(|err| err.to_string())
    }
}

mod vec2_array {
    use egui::Vec2;
    use serde::de::{self, SeqAccess, Visitor};
    use serde::{Deserializer, Serialize, Serializer};
    use std::fmt;

    pub fn serialize<S>(value: &Vec2, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        [value.x, value.y].serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec2, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(Vec2Visitor)
    }

    struct Vec2Visitor;

    impl<'de> Visitor<'de> for Vec2Visitor {
        type Value = Vec2;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a 2-element float array [x, y]")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let x: f32 = seq
                .next_element()?
                .ok_or_else(|| de::Error::custom("missing x value"))?;
            let y: f32 = seq
                .next_element()?
                .ok_or_else(|| de::Error::custom("missing y value"))?;
            Ok(Vec2::new(x, y))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Style;

    #[test]
    fn style_toml_roundtrip() {
        let style = Style::default();
        let serialized = toml::to_string(&style).expect("should serialize");
        assert!(!serialized.trim().is_empty());
        let deserialized: Style = toml::from_str(&serialized).expect("should deserialize");
        assert_eq!(style.padding, deserialized.padding);
        assert_eq!(style.text_color, deserialized.text_color);
        assert_eq!(style.node.port_radius, deserialized.node.port_radius);
        assert_eq!(
            style.node.const_badge_offset,
            deserialized.node.const_badge_offset
        );
    }
}
