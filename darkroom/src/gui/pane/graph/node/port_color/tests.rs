use scenarium::TypeId;

use super::*;

fn custom(id: u128) -> DataType {
    DataType::Custom(TypeId::from_u128(id))
}

#[test]
fn distinct_builtin_types_get_distinct_colors() {
    let t = Theme::dark();
    let f = port_color(&t, &DataType::Float, PortKind::Input, false);
    let i = port_color(&t, &DataType::Int, PortKind::Input, false);
    let b = port_color(&t, &DataType::Bool, PortKind::Input, false);
    let s = port_color(&t, &DataType::String, PortKind::Input, false);
    assert_ne!(f, i);
    assert_ne!(i, b);
    assert_ne!(b, s);
    assert_ne!(f, s);
}

#[test]
fn type_color_independent_of_kind() {
    let t = Theme::dark();
    assert_eq!(
        port_color(&t, &DataType::Float, PortKind::Input, false),
        port_color(&t, &DataType::Float, PortKind::Output, false),
    );
}

#[test]
fn null_falls_back_to_positional_port_color() {
    let t = Theme::dark();
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Input, false),
        t.colors.input_port.rest
    );
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Output, false),
        t.colors.output_port.rest
    );
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Input, true),
        t.colors.input_port.hover
    );
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Output, true),
        t.colors.output_port.hover
    );
}

#[test]
fn hover_changes_typed_color() {
    let t = Theme::dark();
    let base = port_color(&t, &DataType::Float, PortKind::Input, false);
    let hov = port_color(&t, &DataType::Float, PortKind::Input, true);
    assert_ne!(base, hov);
}

#[test]
fn custom_types_keyed_by_type_id() {
    let t = Theme::dark();
    // Same id → same color.
    assert_eq!(
        port_color(&t, &custom(7), PortKind::Input, false),
        port_color(&t, &custom(7), PortKind::Input, false),
    );
    // Ids in adjacent ramp slots → different colors (ramp entries
    // are distinct, len > 1).
    assert_ne!(
        port_color(&t, &custom(0), PortKind::Input, false),
        port_color(&t, &custom(1), PortKind::Input, false),
    );
    // The lens image type bypasses the ramp for its owned hue, which no
    // ramp entry (in either palette) may duplicate — a hash pick must
    // never impersonate the image color.
    let image_ty = DataType::Custom(*lens::IMAGE_TYPE_ID);
    assert_eq!(
        port_color(&t, &image_ty, PortKind::Input, false),
        t.type_colors.image
    );
    let light = Theme::light();
    assert_eq!(
        port_color(&light, &image_ty, PortKind::Input, false),
        light.type_colors.image
    );
    for tc in [&t.type_colors, &light.type_colors] {
        assert!(!tc.ramp.contains(&tc.image));
    }
}

#[test]
fn event_color_is_neutral_and_lifts_on_hover() {
    // Events use the theme's neutral event swatch, distinct from any data
    // hue, and the hover variant differs from rest on both presets.
    for t in [Theme::dark(), Theme::light()] {
        let rest = event_color(&t, false);
        let hov = event_color(&t, true);
        assert_eq!(rest, t.colors.event_port.rest);
        assert_eq!(hov, t.colors.event_port.hover);
        assert_ne!(rest, hov, "hover must visibly differ from rest");
    }
}

#[test]
fn light_and_dark_palettes_differ() {
    let dark = Theme::dark();
    let light = Theme::light();
    assert_ne!(
        port_color(&dark, &DataType::Float, PortKind::Input, false),
        port_color(&light, &DataType::Float, PortKind::Input, false),
    );
}
