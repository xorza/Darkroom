use scenarium::TypeId;

use super::*;

fn custom(id: u128) -> DataType {
    DataType::Custom(TypeId::from_u128(id))
}

/// What a typed port's color varies with — the declared type and the hover
/// flag — and what it must not vary with: the column it sits in.
#[test]
fn a_typed_ports_color_varies_by_type_and_hover_but_not_by_column() {
    let theme = Theme::default();
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
                port_color(&theme, a, PortKind::Input, false),
                port_color(&theme, b, PortKind::Input, false),
                "{a:?} and {b:?} share a color",
            );
        }
    }

    for ty in &types {
        let rest = port_color(&theme, ty, PortKind::Input, false);
        assert_eq!(
            rest,
            port_color(&theme, ty, PortKind::Output, false),
            "{ty:?} is the type's hue, not the column's",
        );
        assert_ne!(
            rest,
            port_color(&theme, ty, PortKind::Input, true),
            "{ty:?} must visibly lift on hover",
        );
    }
}

/// Every port lifts through one rule now, so an untyped port rests on its
/// positional colour and hovers to the same emphasis a typed one would.
#[test]
fn null_falls_back_to_positional_port_color() {
    let t = Theme::default();
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Input, false),
        t.ports.input
    );
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Output, false),
        t.ports.output
    );
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Input, true),
        emphasize(t.ports.input)
    );
    assert_eq!(
        port_color(&t, &DataType::Any, PortKind::Output, true),
        emphasize(t.ports.output)
    );
}

#[test]
fn custom_types_keyed_by_type_id() {
    let t = Theme::default();
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
    // ramp entry may duplicate — a hash pick must never impersonate the
    // image color.
    let image_ty = DataType::Custom(*lens::IMAGE_TYPE_ID);
    assert_eq!(
        port_color(&t, &image_ty, PortKind::Input, false),
        t.type_colors.image
    );
    assert!(!t.type_colors.ramp.contains(&t.type_colors.image));
}

#[test]
fn event_color_is_neutral_and_lifts_on_hover() {
    // Events use the theme's event colour, distinct from any data hue, and
    // the hover variant differs from rest.
    let t = Theme::default();
    let rest = event_color(&t, false);
    let hov = event_color(&t, true);
    assert_eq!(rest, t.ports.event);
    assert_eq!(hov, emphasize(t.ports.event));
    assert_ne!(rest, hov, "hover must visibly differ from rest");
}
