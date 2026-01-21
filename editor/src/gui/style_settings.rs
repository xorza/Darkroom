use egui::{Color32, Vec2};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
            color_shadow_cached: Color32::from_rgb(100, 160, 255),
            color_shadow_missing: Color32::from_rgb(255, 180, 70),
            color_shadow_errored: Color32::from_rgb(238, 66, 66),
            color_dotted: Color32::from_rgb(48, 48, 48),
            corner_radius: 4.0,
            small_corner_radius: 2.0,
            default_bg_stroke_width: 1.0,
            big_padding: 5.0,
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
            shadow_blur: 6.0,
            shadow_spread: 2.0,
            cache_btn_width: 50.0,
            remove_btn_size: 10.0,
            port_radius: 5.0,
            port_activation_radius: 7.0,
            port_label_side_padding: 8.0,
            const_badge_offset: Vec2::new(-10.0, 0.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleSettings {
    #[serde(with = "color_hex")]
    pub color_bg_noninteractive: Color32,
    #[serde(with = "color_hex")]
    pub color_bg_inactive: Color32,
    #[serde(with = "color_hex")]
    pub color_bg_graph: Color32,
    #[serde(with = "color_hex")]
    pub color_bg_hover: Color32,
    #[serde(with = "color_hex")]
    pub color_bg_active: Color32,
    #[serde(with = "color_hex")]
    pub color_bg_checked: Color32,
    #[serde(with = "color_hex")]
    pub color_stroke_inactive: Color32,
    #[serde(with = "color_hex")]
    pub color_stroke_active: Color32,
    #[serde(with = "color_hex")]
    pub color_port_input: Color32,
    #[serde(with = "color_hex")]
    pub color_port_output: Color32,
    #[serde(with = "color_hex")]
    pub color_port_input_hover: Color32,
    #[serde(with = "color_hex")]
    pub color_port_output_hover: Color32,
    #[serde(with = "color_hex")]
    pub color_port_trigger: Color32,
    #[serde(with = "color_hex")]
    pub color_port_event: Color32,
    #[serde(with = "color_hex")]
    pub color_port_trigger_hover: Color32,
    #[serde(with = "color_hex")]
    pub color_port_event_hover: Color32,
    #[serde(with = "color_hex")]
    pub color_stroke_breaker: Color32,
    #[serde(with = "color_hex")]
    pub color_stroke_broke: Color32,
    #[serde(with = "color_hex")]
    pub color_text: Color32,
    #[serde(with = "color_hex")]
    pub color_text_noninteractive: Color32,
    #[serde(with = "color_hex")]
    pub color_text_checked: Color32,
    #[serde(with = "color_hex")]
    pub color_dot_impure: Color32,
    #[serde(with = "color_hex")]
    pub color_shadow_executed: Color32,
    #[serde(with = "color_hex")]
    pub color_shadow_cached: Color32,
    #[serde(with = "color_hex")]
    pub color_shadow_missing: Color32,
    #[serde(with = "color_hex")]
    pub color_shadow_errored: Color32,
    #[serde(with = "color_hex")]
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
    pub shadow_blur: f32,
    pub shadow_spread: f32,
    pub cache_btn_width: f32,
    pub remove_btn_size: f32,
    pub port_radius: f32,
    pub port_activation_radius: f32,
    pub port_label_side_padding: f32,
    #[serde(with = "vec2_array")]
    pub const_badge_offset: Vec2,
}

impl StyleSettings {
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let payload = std::fs::read_to_string(path).map_err(anyhow::Error::from)?;
        let settings = toml::from_str(&payload).map_err(anyhow::Error::from)?;
        Ok(settings)
    }

    pub fn to_file(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let payload = toml::to_string(self).map_err(anyhow::Error::from)?;
        std::fs::write(path, payload).map_err(anyhow::Error::from)
    }
}

mod color_hex {
    use egui::Color32;
    use serde::de::{self, SeqAccess, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
            formatter.write_str("a color hex string like #RRGGBBAA or a 4-element u8 array")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_hex_color(value).map_err(de::Error::custom)
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            self.visit_str(&value)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let r: u8 = seq
                .next_element()?
                .ok_or_else(|| de::Error::custom("missing red channel"))?;
            let g: u8 = seq
                .next_element()?
                .ok_or_else(|| de::Error::custom("missing green channel"))?;
            let b: u8 = seq
                .next_element()?
                .ok_or_else(|| de::Error::custom("missing blue channel"))?;
            let a: u8 = seq
                .next_element()?
                .ok_or_else(|| de::Error::custom("missing alpha channel"))?;
            Ok(Color32::from_rgba_unmultiplied(r, g, b, a))
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
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
            formatter.write_str("a 2-element float array or { x, y } object")
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

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: de::MapAccess<'de>,
        {
            let mut x: Option<f32> = None;
            let mut y: Option<f32> = None;
            while let Some(key) = map.next_key::<String>()? {
                match key.as_str() {
                    "x" => x = Some(map.next_value()?),
                    "y" => y = Some(map.next_value()?),
                    _ => {
                        let _: de::IgnoredAny = map.next_value()?;
                    }
                }
            }
            let x = x.ok_or_else(|| de::Error::custom("missing x value"))?;
            let y = y.ok_or_else(|| de::Error::custom("missing y value"))?;
            Ok(Vec2::new(x, y))
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
        assert!(
            serialized.contains("#EBC846FF"),
            "serialized colors should use hex format"
        );
        assert!(
            serialized.contains("const_badge_offset = ["),
            "serialized vec2 values should use array format"
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
