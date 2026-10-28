use palantir::{CloseShown, RadiusSteps, SpaceSteps, SrgbaU8, StrokeSteps};
use scenarium::TypeId;
use static_assertions::assert_not_impl_any;

use super::*;
use crate::gui::widgets::hover::Hover;
use crate::gui::widgets::port_glyph;

assert_not_impl_any!(Theme: Copy);
assert_not_impl_any!(AppColors: Copy);
assert_not_impl_any!(CanvasTheme: Copy);
assert_not_impl_any!(CardTheme: Copy);
assert_not_impl_any!(PortTheme: Copy);
assert_not_impl_any!(StatusColors: Copy);
assert_not_impl_any!(TypeColors: Copy);
assert_not_impl_any!(ConstEditorLooks: Copy);
assert_not_impl_any!(FlatTextEdit: Copy);

/// Pin which palette role reaches which field, plus the non-trivial
/// palantir tweak, so a regression in `Theme::build`'s wiring, in
/// `palantir_bridge::theme`, or in `palantir_bridge::menu_button` fails
/// loudly. Against the loaded palette rather than hex literals: the values
/// are the palette's to choose, but landing `element_strong` in `canvas.bg` is
/// still a bug.
#[test]
fn default_wiring_and_menu_tweak() {
    let p = Palette::load();
    let theme = Theme::default();
    assert_eq!(theme.canvas.bg, p.canvas_bg);
    assert_eq!(theme.card.lift, p.element_strong);
    assert_eq!(theme.port.input, p.input_port);
    assert_eq!(theme.badge.cache, p.badge_cache);
    assert_eq!(theme.badge.kind, p.badge_kind);
    assert_eq!(theme.status.busy, p.status_busy);
    // Each of those is a distinct role, so a palette that collapsed them
    // onto one color would pass every assertion above.
    assert_ne!(theme.canvas.bg, theme.card.lift);
    // The palantir half is the same palette, not palantir's own default.
    assert_eq!(theme.palantir.window_clear, p.canvas_bg);
    assert!(theme.palantir.tooltip.max_size.h.is_infinite());

    // The open menu's trigger rests lit, as the hovered one does; a shut one rests flat.
    let (shut, open) = (&theme.menu_button.shut.looks, &theme.menu_button.open.looks);
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
    // A header's icon button and a port row are one height: 16 + 2 · 6 = 28 = 14 · 2.
    assert_eq!(theme.icon.control.side(), theme.row_height());
    assert_eq!(theme.row_height(), 28.0);
    // Every floating surface sits on one tier.
    let floating = theme.elevation.floating;
    assert_eq!(menu.panel.shadow, floating);
    assert_eq!(theme.palantir.tooltip.panel.shadow, floating);
    assert_eq!(theme.palantir.modal.panel.shadow, floating);

    // Every light-dismiss popup is frosted glass under its own color at 0.72 alpha; a modal
    // stays opaque under its scrim. The glass's ceiling holds the muted ink, a popup's dimmest,
    // at WCAG's 4.5:1 over the brightest backdrop: #a4a4a4 is `Y = 0.371238` and the #383838
    // panel `Y = 0.039546`, so the brightest ground is `(0.371238 + 0.05) / 4.5 − 0.05 =
    // 0.043608` and the ceiling `(0.043608 − 0.72 · 0.039546) / 0.28 = 0.05405387`. Over white,
    // the tint alone left the ground at `0.28 + 0.72 · 0.039546 = 0.308473`, 1.18:1.
    let ui = &theme.palantir;
    for (name, panel) in [
        ("menu", &menu.panel),
        ("tooltip", &ui.tooltip.panel),
        ("color picker", &ui.color_picker.popup),
    ] {
        let ceiling = panel.backdrop_filter.max_luminance.expect(name);
        assert_eq!(
            panel.backdrop_filter,
            palantir::BackdropFilter::frosted(20.0).with_max_luminance(ceiling),
            "{name}"
        );
        // The hand value's eighth digit, and the derivation's f32 rounding, a few ulps of 4e-9.
        assert!((ceiling - 0.05405387).abs() < 1e-7, "{name}: {ceiling}");
        assert_eq!(panel.fill, p.node_bg.with_alpha(0.72).into(), "{name}");
        let ground = 0.28 * ceiling + 0.72 * color::luminance(p.node_bg);
        let ratio = (color::luminance(p.text_muted) + 0.05) / (ground + 0.05);
        // The same f32 rounding, carried through the ratio: a few ulps of 5e-7.
        assert!((ratio - 4.5).abs() < 2e-6, "{name}: {ratio:.7}:1");
    }
    assert!(ui.modal.panel.backdrop_filter.is_noop());
    assert_eq!(ui.modal.panel.fill, p.element_mid.into());
}

/// Every palantir widget resolves its text to the type scale's body, not to palantir's 16 px
/// default: the ambient size is the body tier, and no look pins a size of its own.
#[test]
fn every_palantir_widget_resolves_to_the_body_tier() {
    let theme = Theme::default();
    let ui = &theme.palantir;
    assert_eq!(ui.text.font_size, theme.font.body);
    let looks = [
        ("button", &ui.button.looks),
        ("menu button", &theme.menu_button.shut.looks),
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
                theme.font.body,
                "{widget}"
            );
        }
    }
    assert_eq!(ui.tooltip.text.apply(&ui.text).font_size, theme.font.body);
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
    let theme = Theme::default();
    let (c, b) = (&theme.colors, &theme.badge);
    let badges = [
        ("accent", c.accent),
        ("badge_cache", b.cache),
        ("badge_kind", b.kind),
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
        !statuses.contains(&b.kind),
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
        ("surround_bg", p.surround_bg),
        ("node_bg", p.node_bg),
        ("element_mid", p.element_mid),
        ("element_strong", p.element_strong),
    ];
    for pair in ladder.windows(2) {
        let [(under, lower), (over, upper)] = pair else {
            unreachable!("windows(2) yields pairs")
        };
        assert!(
            color::luminance(*lower) < color::luminance(*upper),
            "{under} is not darker than {over} — a surface on it disappears",
        );
    }
}

/// A disabled menu row reads dimmer than a live row's shortcut hint on the menu's surface, so a
/// menu whose rows are all disabled does not read as live. The two were one grey.
#[test]
fn a_disabled_row_is_dimmer_than_a_hint() {
    let p = Palette::load();
    let ground = color::luminance(p.node_bg) + 0.05;
    let ratio = |ink: RgbaF32| (color::luminance(ink) + 0.05) / ground;
    assert!(
        ratio(p.text_disabled) < ratio(p.text_muted),
        "disabled {:.2}:1 against muted {:.2}:1",
        ratio(p.text_disabled),
        ratio(p.text_muted),
    );
}

/// The broken-path look is palantir's text edit with every state's border in
/// the error color, at the width each state already had.
#[test]
fn the_error_path_field_recolors_every_border_and_keeps_its_width() {
    let theme = Theme::default();
    let (base, error) = (&theme.palantir.text_edit, &theme.field_error);
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
    assert_eq!(t.port_color(&DataType::Any), t.port.input);
    assert_eq!(
        Hover::Over.lift(t.port_color(&DataType::Any)),
        color::hover_lift(t.port.input)
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
    let rest = t.port.event;
    let hov = Hover::Over.lift(t.port.event);
    assert_eq!(rest, t.port.event);
    assert_eq!(hov, color::hover_lift(t.port.event));
    assert_ne!(rest, hov, "hover must visibly differ from rest");
    // At rest nothing lifts, and the flag a caller holds maps onto the state.
    assert_eq!(Hover::Rest.lift(rest), rest);
    assert_eq!(
        (Hover::from(false), Hover::from(true)),
        (Hover::Rest, Hover::Over)
    );
}

/// The bridge hands palantir every step of the theme's scales, and palantir's
/// `every_recipe_reads_its_geometry_from_the_metrics` holds each of its recipes to its step, so
/// every padding, radius, line and floating shadow palantir's widgets wear is darkroom's: a
/// palantir control lines up with a darkroom one beside it.
#[test]
fn every_palantir_padding_and_radius_is_on_the_scale() {
    let theme = Theme::default();
    let ui = &theme.palantir;
    let SpaceScale {
        xxs: _,
        xs,
        sm,
        md,
        lg,
        xl,
    } = theme.space.clone();
    let RadiusScale {
        mark,
        control,
        card,
        float,
    } = theme.radius.clone();
    let StrokeScale {
        hairline,
        fine,
        line,
        wire: _,
    } = theme.stroke.clone();
    let metrics = BridgeRoles {
        surround_bg: theme.colors.surround_bg,
        font: &theme.font,
        space: &theme.space,
        radius: &theme.radius,
        stroke: &theme.stroke,
        icon: &theme.icon,
        elevation: &theme.elevation,
    }
    .metrics();
    assert_eq!(metrics.space, SpaceSteps { xs, sm, md, lg, xl });
    assert_eq!(
        metrics.radius,
        RadiusSteps {
            mark,
            control,
            card,
            float
        }
    );
    assert_eq!(
        metrics.stroke,
        StrokeSteps {
            hairline,
            fine,
            line
        }
    );
    assert_eq!(metrics.float_shadow, theme.elevation.floating);
    // The tab strip starts its tabs at the surround's inset, as the menu bar starts its titles.
    assert_eq!(
        theme.palantir.tabs.strip_padding.to_array()[0],
        theme.surround_inset()
    );
    assert_eq!(ui.dock.preview_radius, mark);
    // A dragged tab's ghost is a tab lifted off its strip; a value's box is a field's.
    assert_eq!(ui.dock.ghost_padding, ui.tabs.chip_padding);
    assert_eq!(
        theme.const_editor.rest.drag_value.chip.defaults.padding,
        ui.text_edit.defaults.padding
    );
    // The values the bridge picks, not only that each is a step: a field and the button beside
    // it share a height, and a menu rounds like a floating surface.
    assert_eq!(ui.button.defaults.padding.to_array(), [md, xs, md, xs]);
    assert_eq!(ui.text_edit.defaults.padding.to_array(), [sm, xs, sm, xs]);
    assert_eq!(ui.context_menu.panel.corners.to_array(), [float; 4]);
    assert_eq!(ui.tabs.strip_padding.to_array(), [sm, xs, sm, 0.0]);
    assert_eq!(ui.tabs.chip_padding, ui.context_menu.item.defaults.padding);
    assert_eq!(
        theme.menu_button.shut.defaults.padding,
        ui.tabs.chip_padding
    );
    assert_eq!(
        theme.menu_button.open.defaults.padding,
        ui.tabs.chip_padding
    );
    assert_eq!(ui.tabs.chip_padding.to_array(), [md, sm, md, sm]);
    assert_eq!(
        (ui.tabs.gap, ui.tabs.label_gap, ui.tabs.trailing_inset),
        (xs, sm, xs)
    );
    assert_eq!(
        ui.tabs.close.hovered.chrome.corners.to_array(),
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
    let titles = &theme.menu_button.shut.looks;
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
        (ExecStatus::None, None, theme.ink.muted),
        (ExecStatus::Cached, None, s.info),
        (ExecStatus::Executed(0.5), None, s.success),
        (ExecStatus::Skipped, None, theme.ink.muted),
        (
            ExecStatus::Running(std::time::Instant::now()),
            Some(s.busy),
            s.busy,
        ),
        (
            ExecStatus::MissingInputs(vec![0]),
            Some(s.warning),
            s.warning,
        ),
        (ExecStatus::Errored(None), Some(s.error), s.error),
    ];
    for (status, frame, ink) in cases {
        assert_eq!(theme.exec_frame(&status), frame, "{status:?} frame");
        assert_eq!(theme.exec_ink(&status), ink, "{status:?} ink");
    }
}

/// No untyped port takes a hue a run status, an alarm or a data type spends: an event is not the
/// Bool and error red, and an untyped input is not the String and success green, so a port on a
/// framed node reads as a port, and an untyped input as no type. Busy is left out: it and the
/// untyped input share the neutral grey, each as no verdict and no type, in forms that differ,
/// a spinner and a halo against a hollow ring.
#[test]
fn no_untyped_port_borrows_a_status_or_a_type_hue() {
    let t = Theme::default();
    let s = &t.status;
    let statuses = [
        s.success,
        s.info,
        s.warning,
        s.error,
        t.canvas.wire_broken,
        t.canvas.breaker_stroke,
    ];
    let hues = &t.type_colors;
    let types = [
        hues.boolean,
        hues.int,
        hues.float,
        hues.string,
        hues.path,
        hues.image,
    ];
    for (name, port) in [("event", t.port.event), ("untyped input", t.port.input)] {
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
        color::wash(theme.status.error, 0.14, theme.canvas.bg).to_srgba(),
        SrgbaU8::hex(0x352525)
    );
    assert_eq!(
        color::wash(theme.status.error, 0.0, theme.canvas.bg).to_srgba(),
        SrgbaU8::hex(0x141414)
    );
}

/// A port glyph hangs on the body's edge and takes no room in its cell, so every label starts at
/// the column inset, the border plus `space.md`, as the header's title does: the glyph's left
/// edge sits a radius out from the body's edge, and its margins, width and the cell's gap sum to
/// zero, for the plain 13 px circle, a required input's bigger one, and on either side.
#[test]
fn a_port_glyph_hangs_on_the_edge_and_leaves_its_label_at_the_inset() {
    let theme = Theme::default();
    let inset = theme.card.border_width + theme.port.col_pad_x;
    assert_eq!(inset, theme.card.border_width + theme.space.md);
    // A required input's disc: the 12 px port and a 2 px ring each side, 12 + 2 × 2 = 16.
    let enlarged = port_glyph::enlarged_diameter(theme.port.size, theme.stroke.line);
    assert_eq!(enlarged, 16.0);
    for radius in [theme.port.radius(), enlarged * 0.5] {
        let input = theme.port_margin(PortKind::Input, radius);
        let [left, _, right, _] = input.to_array();
        // From the body's outer edge: the cell starts at the inset, the glyph a margin before it.
        assert_eq!(inset + left, -radius, "the glyph's center is on the edge");
        assert_eq!(
            left + 2.0 * radius + right + theme.space.xs,
            0.0,
            "{radius}"
        );
        let output = theme.port_margin(PortKind::Output, radius);
        assert_eq!(output.to_array(), [right, 0.0, left, 0.0], "mirrored");
    }
}

/// A wire is never thinner than 2.5 px on screen: at 50 % its world width doubles to 5 so the
/// screen keeps 2.5; at 100 % it is 2.5; at 200 % it zooms with the canvas, 2.5 in the world and
/// 5 on screen.
#[test]
fn a_wire_keeps_its_width_on_screen_when_zoomed_out() {
    let stroke = Theme::default().stroke;
    assert_eq!(stroke.wire, 2.5);
    for (zoom, world, screen) in [(0.5, 5.0, 2.5), (1.0, 2.5, 2.5), (2.0, 2.5, 5.0)] {
        assert_eq!(stroke.wire_at(zoom), world, "{zoom}");
        assert_eq!(stroke.wire_at(zoom) * zoom, screen, "{zoom}");
    }
}
