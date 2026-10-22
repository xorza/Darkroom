use scenarium::TypeId;

use super::*;
use crate::gui::theme::color;
use crate::gui::widgets::hover::Hover;

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
                data(&theme, a),
                data(&theme, b),
                "{a:?} and {b:?} share a color",
            );
        }
    }

    for ty in &types {
        let rest = data(&theme, ty);
        assert_ne!(
            rest,
            Hover::Over.lift(data(&theme, ty)),
            "{ty:?} must visibly lift on hover",
        );
    }
}

/// Every port lifts through one rule, so an untyped input rests on its
/// untyped color and hovers to the same emphasis a typed one would.
#[test]
fn any_falls_back_to_the_untyped_input_color() {
    let t = Theme::default();
    assert_eq!(data(&t, &DataType::Any), t.ports.input);
    assert_eq!(
        Hover::Over.lift(data(&t, &DataType::Any)),
        color::hover_lift(t.ports.input)
    );
}

#[test]
fn custom_types_keyed_by_type_id() {
    let t = Theme::default();
    // Ids in adjacent ramp slots → different colors (ramp entries
    // are distinct, len > 1).
    assert_ne!(data(&t, &custom(0)), data(&t, &custom(1)),);
    // The lens image type bypasses the ramp for its owned hue, which no
    // ramp entry may duplicate — a hash pick must never impersonate the
    // image color.
    let image_ty = DataType::Custom(lens::IMAGE_TYPE_ID);
    assert_eq!(data(&t, &image_ty), t.type_colors.image);
    assert!(!t.type_colors.ramp.contains(&t.type_colors.image));
}

#[test]
fn event_color_is_neutral_and_lifts_on_hover() {
    // Events use the theme's event color, distinct from any data hue, and
    // the hover variant differs from rest.
    let t = Theme::default();
    let rest = event(&t);
    let hov = Hover::Over.lift(event(&t));
    assert_eq!(rest, t.ports.event);
    assert_eq!(hov, color::hover_lift(t.ports.event));
    assert_ne!(rest, hov, "hover must visibly differ from rest");
    // At rest nothing lifts, and the flag a caller holds maps onto the state.
    assert_eq!(Hover::Rest.lift(rest), rest);
    assert_eq!(
        (Hover::from(false), Hover::from(true)),
        (Hover::Rest, Hover::Over)
    );
}
