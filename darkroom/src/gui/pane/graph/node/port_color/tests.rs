use scenarium::TypeId;

use super::*;

fn custom(id: u128) -> DataType {
    DataType::Custom(TypeId::from_u128(id))
}

/// What a typed port's color varies with — the declared type, the hover flag,
/// and the preset — and what it must not vary with: the column it sits in.
#[test]
fn a_typed_ports_color_varies_by_type_hover_and_preset_but_not_by_column() {
    let (dark, light) = (Theme::dark(), Theme::light());
    let types = [
        DataType::Float,
        DataType::Int,
        DataType::Bool,
        DataType::String,
    ];

    // Pairwise distinct, so no two declared types read as the same wire.
    for (i, a) in types.iter().enumerate() {
        for b in &types[i + 1..] {
            assert_ne!(
                port_color(&dark, a, PortKind::Input, false),
                port_color(&dark, b, PortKind::Input, false),
                "{a:?} and {b:?} share a color",
            );
        }
    }

    for ty in &types {
        let rest = port_color(&dark, ty, PortKind::Input, false);
        assert_eq!(
            rest,
            port_color(&dark, ty, PortKind::Output, false),
            "{ty:?} is the type's hue, not the column's",
        );
        assert_ne!(
            rest,
            port_color(&dark, ty, PortKind::Input, true),
            "{ty:?} must visibly lift on hover",
        );
        assert_ne!(
            rest,
            port_color(&light, ty, PortKind::Input, false),
            "{ty:?} is picked out of the preset's own palette",
        );
    }
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
