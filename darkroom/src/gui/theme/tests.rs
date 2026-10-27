use glam::Vec2;
use palantir::{Chrome, CloseShown, Spacing, SrgbaU8};
use scenarium::TypeId;
use static_assertions::assert_not_impl_any;

use super::*;
use crate::gui::widgets::hover::Hover;

assert_not_impl_any!(Theme: Copy);
assert_not_impl_any!(AppColors: Copy);
assert_not_impl_any!(CanvasTheme: Copy);
assert_not_impl_any!(CardTheme: Copy);
assert_not_impl_any!(PortTheme: Copy);
assert_not_impl_any!(StatusColors: Copy);
assert_not_impl_any!(TypeColors: Copy);
assert_not_impl_any!(ConstEditorTheme: Copy);
assert_not_impl_any!(InlineRenameTheme: Copy);

/// Pin which palette role reaches which field, plus the non-trivial
/// palantir tweak, so a regression in `Theme::build`'s wiring, in
/// `palantir_bridge::theme`, or in `palantir_bridge::menu_button` fails
/// loudly. Against the loaded palette rather than hex literals: the values
/// are the palette's to choose, but landing `header_fill` in `canvas.bg` is
/// still a bug.
#[test]
fn default_wiring_and_menu_tweak() {
    let p = Palette::load();
    let theme = Theme::default();
    assert_eq!(theme.canvas.bg, p.canvas_bg);
    assert_eq!(theme.card.header_fill, p.header_fill);
    assert_eq!(theme.ports.input, p.input_port);
    assert_eq!(theme.colors.badge_cache, p.badge_cache);
    assert_eq!(theme.colors.badge_kind, p.badge_kind);
    assert_eq!(theme.status.busy, p.status_busy);
    // Each of those is a distinct role, so a palette that collapsed them
    // onto one color would pass every assertion above.
    assert_ne!(theme.canvas.bg, theme.card.header_fill);
    // The palantir half is the same palette, not palantir's own default.
    assert_eq!(theme.palantir.window_clear, p.canvas_bg);
    assert!(theme.palantir.tooltip.max_size.h.is_infinite());

    // The open menu's trigger rests lit, as the hovered one does; a shut one rests flat.
    let (shut, open) = (&theme.menu_button.looks, &theme.menu_button_open.looks);
    assert_eq!(open.normal, shut.hovered);
    assert_eq!(open.hovered, shut.hovered);
    assert_ne!(open.normal, shut.normal);
    // A menu's rows and panel as the review drew them.
    let menu = &theme.palantir.context_menu;
    assert_eq!(
        menu.item.defaults.padding,
        Spacing::xy(theme.space.md, theme.space.sm)
    );
    assert_eq!(menu.item.gap, theme.space.xl);
    assert_eq!(menu.panel.shadow.offset, Vec2::new(0.0, theme.space.sm));
    assert_eq!(menu.panel.shadow.blur, theme.space.lg);
}

/// Every palantir widget resolves its text to the type scale's body, not to palantir's 16 px
/// default: the ambient size is the body tier, and no look pins a size of its own.
#[test]
fn every_palantir_widget_resolves_to_the_body_tier() {
    let theme = Theme::default();
    let ui = &theme.palantir;
    assert_eq!(ui.text.font_size, theme.text.body);
    let looks = [
        ("button", &ui.button.looks),
        ("menu button", &theme.menu_button.looks),
        ("menu item", &ui.context_menu.item.looks),
        ("selected tab", &ui.tabs.selected),
        ("unselected tab", &ui.tabs.unselected),
        ("text edit", &ui.text_edit.looks),
        ("unchecked checkbox", &ui.checkbox.unchecked),
        ("checked checkbox", &ui.checkbox.checked),
    ];
    for (widget, look) in looks {
        for state in [&look.normal, &look.hovered, &look.active, &look.disabled] {
            assert_eq!(
                state.text.apply(&ui.text).font_size,
                theme.text.body,
                "{widget}"
            );
        }
    }
    assert_eq!(ui.tooltip.text.apply(&ui.text).font_size, theme.text.body);
}

/// Every badge is legible against every other, because a node's head
/// can carry several at once; and the kind mark wears no status hue, as a
/// kind is not a status.
///
/// A badge sharing a hue with a wire type or a status is fine and
/// common — the palette resolves both against one upstream semantic
/// layer, and the two never share a surface. Two badges do, so this is
/// the pair the roster cannot be allowed to collapse. Like
/// [`surfaces_stack_darkest_first`], it is a rule a table of
/// colors cannot state about itself.
#[test]
fn badges_are_told_apart_from_each_other() {
    let c = Theme::default().colors;
    let badges = [
        ("badge_graph", c.badge_graph),
        ("badge_cache", c.badge_cache),
        ("badge_kind", c.badge_kind),
    ];
    for (i, (name, color)) in badges.iter().enumerate() {
        for (other_name, other) in &badges[i + 1..] {
            assert_ne!(
                color, other,
                "{name} and {other_name} share a hue, and a head can show both",
            );
        }
    }
    let s = Theme::default().status;
    let statuses = [s.success, s.info, s.busy, s.warning, s.error];
    assert!(
        !statuses.contains(&c.badge_kind),
        "the kind mark wears a status hue"
    );
}

/// The five surfaces stack in one view — a graph, the bar around
/// it, a node, a hovered control, a pressed one — so
/// each must be lighter than the one under it. The palette's generator
/// checks this before it writes the file; this is the half that would
/// catch a hand-edited asset, and it is the one rule a table of colors
/// cannot state about itself.
#[test]
fn surfaces_stack_darkest_first() {
    let p = Palette::load();
    let ladder = [
        ("canvas_bg", p.canvas_bg),
        ("surround_fill", p.surround_fill),
        ("node_fill", p.node_fill),
        ("elem_hover", p.elem_hover),
        ("header_fill", p.header_fill),
    ];
    for pair in ladder.windows(2) {
        let [(under, lower), (over, upper)] = pair else {
            unreachable!("windows(2) yields pairs")
        };
        assert!(
            luminance(*lower) < luminance(*upper),
            "{under} is not darker than {over} — a surface on it disappears",
        );
    }
}

/// A disabled menu row reads dimmer than a live row's shortcut hint on the menu's surface, so a
/// menu whose rows are all disabled does not read as live. The two were one grey.
#[test]
fn a_disabled_row_is_dimmer_than_a_hint() {
    let p = Palette::load();
    let ground = luminance(p.node_fill) + 0.05;
    let ratio = |ink: RgbaF32| (luminance(ink) + 0.05) / ground;
    assert!(
        ratio(p.text_disabled) < ratio(p.text_muted),
        "disabled {:.2}:1 against muted {:.2}:1",
        ratio(p.text_disabled),
        ratio(p.text_muted),
    );
}

/// Relative luminance, the WCAG definition, of the linear channels an `RgbaF32` holds. Local
/// to the test: nothing darkroom draws needs it, and the palette it checks is generated by a
/// tool that computes the same number.
fn luminance(c: RgbaF32) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}

/// The broken-path look is palantir's text edit with every state's border in
/// the error color, at the width each state already had.
#[test]
fn the_error_path_field_recolors_every_border_and_keeps_its_width() {
    let theme = Theme::default();
    let (base, error) = (&theme.palantir.text_edit, &theme.path_field_error);
    for (base, error) in [
        (&base.looks.normal, &error.looks.normal),
        (&base.looks.hovered, &error.looks.hovered),
        (&base.looks.active, &error.looks.active),
        (&base.looks.disabled, &error.looks.disabled),
    ] {
        assert_eq!(error.chrome.border.color, theme.status.error);
        assert_eq!(error.chrome.border.width, base.chrome.border.width);
        assert_ne!(base.chrome.border.color, theme.status.error);
    }
}

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
                theme.port_color(a),
                theme.port_color(b),
                "{a:?} and {b:?} share a color",
            );
        }
    }

    for data_type in &types {
        let rest = theme.port_color(data_type);
        assert_ne!(
            rest,
            Hover::Over.lift(theme.port_color(data_type)),
            "{data_type:?} must visibly lift on hover",
        );
    }
}

/// Every port lifts through one rule, so an untyped input rests on its
/// untyped color and hovers to the same emphasis a typed one would.
#[test]
fn any_falls_back_to_the_untyped_input_color() {
    let t = Theme::default();
    assert_eq!(t.port_color(&DataType::Any), t.ports.input);
    assert_eq!(
        Hover::Over.lift(t.port_color(&DataType::Any)),
        color::hover_lift(t.ports.input)
    );
}

#[test]
fn custom_types_keyed_by_type_id() {
    let t = Theme::default();
    // Ids in adjacent ramp slots → different colors (ramp entries
    // are distinct, len > 1).
    assert_ne!(t.port_color(&custom(0)), t.port_color(&custom(1)),);
    // The lens image type bypasses the ramp for its owned hue, which no
    // ramp entry may duplicate — a hash pick must never impersonate the
    // image color.
    let image_type = DataType::Custom(lens::IMAGE_TYPE_ID);
    assert_eq!(t.port_color(&image_type), t.type_colors.image);
    assert!(!t.type_colors.ramp.contains(&t.type_colors.image));
}

#[test]
fn the_event_color_is_the_themes_and_lifts_on_hover() {
    // Events use the theme's event color, distinct from any data hue, and
    // the hover variant differs from rest.
    let t = Theme::default();
    let rest = t.event_color();
    let hov = Hover::Over.lift(t.event_color());
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

/// Every padding and corner radius the bridge gives palantir's widgets is a step of the theme's
/// scales, or zero: a palantir control lines up with a darkroom one beside it.
#[test]
fn every_palantir_padding_and_radius_is_on_the_scale() {
    let theme = Theme::default();
    let ui = &theme.palantir;
    let SpaceScale {
        xs,
        sm,
        md,
        lg,
        xl,
        xxl,
    } = theme.space.clone();
    let RadiusScale {
        control,
        card,
        float,
    } = theme.radius.clone();
    let paddings = [
        ("button", ui.button.defaults.padding),
        ("text edit", ui.text_edit.defaults.padding),
        ("menu item", ui.context_menu.item.defaults.padding),
        ("menu", ui.context_menu.padding),
        ("tooltip", ui.tooltip.padding),
        ("tab chip", ui.tabs.chip_padding),
        ("tab strip", ui.tabs.strip_padding),
        ("modal", ui.modal.padding),
    ];
    for (widget, padding) in paddings {
        for side in padding.as_array() {
            assert!(
                [0.0, xs, sm, md, lg, xl, xxl].contains(&side),
                "{widget} padding {side}"
            );
        }
    }
    let gaps = [
        ("tab strip gap", ui.tabs.gap),
        ("tab label gap", ui.tabs.label_gap),
        ("tab trailing inset", ui.tabs.trailing_inset),
    ];
    for (widget, gap) in gaps {
        assert!([xs, sm, md, lg, xl, xxl].contains(&gap), "{widget} {gap}");
    }
    let looks = [
        ("button", &ui.button.looks),
        ("text edit", &ui.text_edit.looks),
        ("menu item", &ui.context_menu.item.looks),
        ("tab close", &ui.tabs.close),
    ];
    let mut corners = vec![
        ("tooltip", ui.tooltip.panel.corners),
        ("menu", ui.context_menu.panel.corners),
        ("modal", ui.modal.panel.corners),
        ("dock ghost", ui.dock.ghost.chrome.corners),
    ];
    for (widget, look) in looks {
        for state in [&look.normal, &look.hovered, &look.active, &look.disabled] {
            corners.push((widget, state.chrome.corners));
        }
    }
    for (widget, corners) in corners {
        for radius in corners.as_array() {
            assert!(
                [0.0, control, card, float].contains(&radius),
                "{widget} radius {radius}"
            );
        }
    }
    // The values the bridge picks, not only that each is a step: a field and the button beside
    // it share a height, and a menu rounds like a floating surface.
    assert_eq!(ui.button.defaults.padding.as_array(), [md, xs, md, xs]);
    assert_eq!(ui.text_edit.defaults.padding.as_array(), [sm, xs, sm, xs]);
    assert_eq!(ui.context_menu.panel.corners.as_array(), [float; 4]);
    assert_eq!(ui.tabs.strip_padding.as_array(), [sm, xs, sm, 0.0]);
    assert_eq!(ui.tabs.chip_padding, ui.context_menu.item.defaults.padding);
    assert_eq!(ui.tabs.chip_padding.as_array(), [md, sm, md, sm]);
    assert_eq!(
        (ui.tabs.gap, ui.tabs.label_gap, ui.tabs.trailing_inset),
        (xs, sm, xs)
    );
    assert_eq!(
        ui.tabs.close.hovered.chrome.corners.as_array(),
        [control; 4]
    );
}

/// A tab chip is a ghost with a menu title's fills, as the two share the surround: no fill at
/// rest, `element_mid` under the pointer and `element_strong` pressed; its close button lifts on
/// `element_strong`, the one fill that shows on a hovered chip. Its close button is the icon set's `Cross` in a 12 px
/// box, 12 of 24 grid units across at the set's 2-unit stroke: a 6 px cross of 1 px strokes, shown
/// on the selected chip and the one under the pointer.
#[test]
fn a_tab_chip_and_its_close_button_are_ghosts() {
    let theme = Theme::default();
    let tabs = &theme.palantir.tabs;
    let looks = &tabs.unselected;
    assert_eq!(looks.normal.chrome, Chrome::NONE);
    assert_eq!(looks.disabled.chrome, Chrome::NONE);
    let titles = &theme.menu_button.looks;
    assert_eq!(looks.hovered.chrome.fill, titles.hovered.chrome.fill);
    assert_eq!(looks.active.chrome.fill, titles.active.chrome.fill);
    assert_eq!(tabs.close.hovered.chrome.fill, titles.active.chrome.fill);
    assert_ne!(tabs.close.hovered.chrome.fill, looks.hovered.chrome.fill);
    assert_eq!(tabs.close.normal.chrome, Chrome::NONE);
    assert_eq!(
        (tabs.close_size, tabs.close_glyph, tabs.close_stroke),
        (18.0, 6.0, 1.0)
    );
    assert_eq!(tabs.close_shown, CloseShown::SelectedOrHovered);
}

/// Only a status that asks for attention frames the node: one in progress, one the run could
/// not feed, and the one that failed. A good run, a cache hit and a node a failure stopped keep
/// the resting shadow; their inks still tell them apart in the footer.
#[test]
fn only_a_status_that_asks_for_attention_frames_the_node() {
    let theme = Theme::default();
    let s = &theme.status;
    let cases = [
        (ExecStatus::None, None, None),
        (ExecStatus::Cached, None, Some(s.info)),
        (ExecStatus::Executed(0.5), None, Some(s.success)),
        (ExecStatus::Skipped, None, None),
        (
            ExecStatus::Running(std::time::Instant::now()),
            Some(s.busy),
            Some(s.busy),
        ),
        (
            ExecStatus::MissingInputs(vec![0]),
            Some(s.warning),
            Some(s.warning),
        ),
        (ExecStatus::Errored(None), Some(s.error), Some(s.error)),
    ];
    for (status, frame, ink) in cases {
        assert_eq!(theme.exec_frame(&status), frame, "{status:?} frame");
        assert_eq!(theme.exec_color(&status), ink, "{status:?} ink");
    }
}

/// No untyped port takes a hue a run status or a data type spends: an event is not the Bool and
/// error red, and an untyped input is not the String and success green, so a port on a framed
/// node reads as a port, and an untyped input as no type.
#[test]
fn no_untyped_port_borrows_a_status_or_a_type_hue() {
    let t = Theme::default();
    let s = &t.status;
    let statuses = [s.success, s.info, s.warning, s.error];
    let hues = &t.type_colors;
    let types = [
        hues.boolean,
        hues.int,
        hues.float,
        hues.string,
        hues.path,
        hues.image,
    ];
    for (name, port) in [("event", t.event_color()), ("untyped input", t.ports.input)] {
        assert!(
            !statuses.contains(&port),
            "the {name} port wears a status hue"
        );
        assert!(!types.contains(&port), "the {name} port wears a type hue");
        assert!(
            !hues.ramp.contains(&port),
            "the {name} port wears a ramp hue"
        );
    }
}

/// A wash composites on the encoded values, as the review drew it: the error ink `#ff8b8c` at
/// 14 % over the canvas `#141414` is `0.14·255 + 0.86·20 = 52.9`, `0.14·139 + 0.86·20 = 36.66`
/// and `0.14·140 + 0.86·20 = 36.8`, so `#352525`, where a linear blend lands near `#653c3c`.
#[test]
fn a_wash_composites_on_the_encoded_values() {
    let theme = Theme::default();
    assert_eq!(
        theme.status.error.to_srgba(),
        SrgbaU8::hex(0xff8b8c),
        "premise"
    );
    assert_eq!(
        theme.canvas.bg.to_srgba(),
        SrgbaU8::hex(0x141414),
        "premise"
    );
    assert_eq!(
        theme.wash(theme.status.error, 0.14).to_srgba(),
        SrgbaU8::hex(0x352525)
    );
    assert_eq!(
        theme.wash(theme.status.error, 0.0).to_srgba(),
        SrgbaU8::hex(0x141414)
    );
}
