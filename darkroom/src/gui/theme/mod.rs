//! [`Theme`]: darkroom's visual palette and layout dimensions, plus the
//! per-widget rosters hanging off it.
//!
//! Every colour comes from [`Palette`], read out of the generated
//! `assets/ayu-graphite.ron`. Each per-widget roster owns a file and fills
//! itself from that palette through its own `from_palette`. The palantir-side
//! half lives in [`palantir_bridge`].

pub(crate) mod canvas_theme;
pub(crate) mod card_theme;
pub(crate) mod chrome_colors;
pub(crate) mod color;
pub(crate) mod const_value_editor_theme;
pub(crate) mod inline_rename_theme;
pub(crate) mod palantir_bridge;
pub(crate) mod palette;
pub(crate) mod port_theme;
pub(crate) mod status_colors;
pub(crate) mod type_colors;
pub(crate) mod type_scale;

use palantir::{ButtonTheme, FontWeight, TextStyle};

use crate::gui::theme::canvas_theme::CanvasTheme;
use crate::gui::theme::card_theme::{CardBorder, CardTheme};
use crate::gui::theme::chrome_colors::ChromeColors;
use crate::gui::theme::const_value_editor_theme::ConstValueEditorTheme;
use crate::gui::theme::inline_rename_theme::InlineRenameTheme;
use crate::gui::theme::palantir_bridge::{
    BridgeRoles, menu_button_for, palantir_palette_for, palantir_theme_for,
};
use crate::gui::theme::palette::Palette;
use crate::gui::theme::port_theme::PortTheme;
use crate::gui::theme::status_colors::StatusColors;
use crate::gui::theme::type_colors::TypeColors;
use crate::gui::theme::type_scale::TypeScale;

/// Visual palette + layout dimensions for darkroom's UI. Owned by `App`,
/// handed to every UI subtree through [`crate::gui::app::ctx::AppCtx`] and the
/// contexts derived from it, so call sites read off a single source
/// instead of hard-coded constants. Layout fields live here too —
/// node ports, value editors, etc. — so a theme swap can restyle
/// geometry as well as color.
///
/// Also owns the palantir [`palantir::Theme`] this app wants on its
/// `Ui`. [`crate::gui::app::App::new`] copies `palantir_theme` into
/// `ui.theme` once before the first frame, so palantir-side widgets
/// (buttons, text edits, menus, scrollbars) read from the same source.
/// Tweak fields on `theme.palantir_theme` during construction to
/// override palantir's defaults.
///
/// Serializable so the whole bundle (palantir palette + darkroom
/// layout + colors) can be written and read back as one RON theme file. No
/// UI reaches that yet — the app assembles [`Theme::default`] every
/// launch — so the derives exist for the format, not for a caller.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Theme {
    /// Stroke width of every mark drawn on the canvas at wire scale: the
    /// wires themselves, the in-flight drag preview, the subscription pin's
    /// leader, and the breaker scribble that cuts them — one width so the
    /// blade reads at the same weight as what it severs.
    pub(crate) stroke_width: f32,
    /// Gap between a node's edge and a floating widget's near edge — the
    /// inspector panel anchors from the node's right edge, so any future
    /// floating surface reads as the same clearance.
    pub(crate) floating_widget_gap: f32,
    /// Cap on the new-node popup's height. Inner scroll handles
    /// overflow when the function list exceeds the cap.
    pub(crate) new_node_popup_max_height: f32,

    /// Font sizes by hierarchy tier, serialized as the `[text]` sub-table.
    pub(crate) text: TypeScale,

    /// The graph canvas and its dotted backdrop (`[canvas]`).
    pub(crate) canvas: CanvasTheme,

    /// Elevated rounded surfaces — node bodies, the inspector panel, dock
    /// tabs (`[card]`).
    pub(crate) card: CardTheme,

    /// A node's ports: circles, label ink, column geometry (`[ports]`).
    pub(crate) ports: PortTheme,

    /// The semantic feedback palette — success / info / busy / warning /
    /// error (`[status]`).
    pub(crate) status: StatusColors,

    /// The colours belonging to no single widget (`[colors]`).
    pub(crate) colors: ChromeColors,

    /// Data-type → wire/port hue roster (see [`TypeColors`]),
    /// serialized as the `[type_colors]` sub-table.
    pub(crate) type_colors: TypeColors,

    /// Look + dimensions for the inline static-value editor that hugs a
    /// `Binding::Const` input port (number/string field, file-pick chip).
    pub(crate) const_value_editor: ConstValueEditorTheme,

    /// The pointer-over-node variant of `const_value_editor` (chip fill
    /// pre-lit at half the hover strength). Precomputed at construction —
    /// deriving it per frame would clone the whole nested theme in the
    /// record path — and kept next to its base so the pair can't drift.
    pub(crate) const_value_editor_revealed: ConstValueEditorTheme,

    /// Look for the inline-rename widget (node title, boundary port,
    /// graph tab). Text is left unset, so a rename inherits ambient
    /// `palantir::Theme::text` like any other label.
    pub(crate) inline_rename: InlineRenameTheme,

    /// The node-title variant of `inline_rename`, with the ambient text
    /// style pinned to [`FontWeight::BOLD`] on every state. Precomputed
    /// at construction, beside its base so the two can't drift — a node
    /// header would otherwise rebuild the whole nested text-edit bundle
    /// per node per frame just to carry one weight.
    pub(crate) inline_rename_title: InlineRenameTheme,

    /// Look for a menu-bar trigger button (`[menu_button]`). Darkroom's
    /// own slot: palantir ships the recipe but no theme field, because
    /// none of its widgets resolve against a menu-bar style — the bar
    /// passes this to [`palantir::Button::style`] itself.
    pub(crate) menu_button: ButtonTheme,

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, TextEdit, MenuItem,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use.
    pub(crate) palantir_theme: palantir::Theme,
}

impl Theme {
    /// How far a port circle of the given `radius` is pulled out of its
    /// column so its **center** lands on the node body's outer edge: clear
    /// the column inset (`port_col_pad_x`) and the body border
    /// (`node_border_width * 2`, which "folds into" the body's content
    /// padding), then push out by `radius` so the dot straddles the edge
    /// evenly. Parameterized rather than always `port_radius()` so an
    /// enlarged port (e.g. a required input's bigger circle) still
    /// straddles the edge correctly — see [`Self::port_overhang`] for the
    /// common (plain-radius) case.
    #[inline]
    pub(crate) fn port_overhang_for(&self, radius: f32) -> f32 {
        radius + self.ports.col_pad_x + self.card.border_width_total()
    }

    /// [`Self::port_overhang_for`] at the plain port radius. Independent of
    /// `port_size` — bigger circles keep their center on the edge.
    #[inline]
    pub(crate) fn port_overhang(&self) -> f32 {
        self.port_overhang_for(self.ports.radius())
    }

    /// Border color + width for a selectable card's 3-tier resting decision
    /// — how a node body resolves its outline: a breaker hit wins as the
    /// alarm color, else the selection halo
    /// when selected, else the neutral resting `node_border`. Width is
    /// always [`CardTheme::border_width_total`] regardless of tier, so
    /// selecting (or breaking) a card never resizes it — only the color
    /// changes. A
    /// caller with an extra tier of its own (e.g. a node body's "missing"
    /// stub state) special-cases that tier around this call instead of
    /// forcing it in here.
    #[inline]
    pub(crate) fn card_border(&self, broken: bool, selected: bool) -> CardBorder {
        let color = if broken {
            self.colors.connection_broken
        } else if selected {
            self.colors.selection_rect
        } else {
            self.card.border
        };
        CardBorder { color }
    }

    /// Assemble the full theme from `p` — the darkroom peer of
    /// `palantir::Theme::from_palette`. Dimensions are palette-independent;
    /// every colour and every sub-recipe (the palantir widget theme, the
    /// static-value editor, inline rename) cascades from `p` rather than
    /// being hand-assembled, so a palette edit reaches the whole app.
    fn build(p: &Palette) -> Self {
        let colors = ChromeColors::from_palette(p);
        // The palantir half is derived here rather than stored on `Palette`:
        // it is a projection of the same roles, and a second copy of them
        // could drift from the one the darkroom rosters read.
        let pal = palantir_palette_for(p);
        // Built before the struct literal because the title variant
        // derives from the palantir theme's ambient text style — the
        // same style an unstyled rename would have inherited anyway,
        // so bolding it is the only difference between the two slots.
        let card = CardTheme::from_palette(p);
        let status = StatusColors::from_palette(p);
        let palantir_theme = palantir_theme_for(
            &pal,
            BridgeRoles {
                chrome_fill: colors.chrome_fill,
                tab_inactive: colors.tab_inactive,
                header_fill: card.header_fill,
                warning: status.warning,
                corner_radius: card.corner_radius,
                text: &TypeScale::DEFAULT,
            },
        );
        let inline_rename = InlineRenameTheme::from_palette(&pal);
        let inline_rename_title = inline_rename.clone().with_text(TextStyle {
            weight: FontWeight::BOLD,
            ..palantir_theme.text
        });
        Self {
            // The three measurements that belong to no widget group; the
            // rest are authored beside their colours in the groups below.
            stroke_width: 2.0,
            floating_widget_gap: 16.0,
            new_node_popup_max_height: 400.0,
            text: TypeScale::DEFAULT,
            canvas: CanvasTheme::from_palette(p),
            card,
            ports: PortTheme::from_palette(p),
            status,
            colors,
            type_colors: p.type_colors.clone(),
            const_value_editor: ConstValueEditorTheme::from_palette(&pal),
            const_value_editor_revealed: ConstValueEditorTheme::revealed_from_palette(&pal),
            inline_rename,
            inline_rename_title,
            menu_button: menu_button_for(&pal, palantir_theme.text, &TypeScale::DEFAULT),
            palantir_theme,
        }
    }
}

impl Default for Theme {
    /// Ayu Graphite — the one built-in look, read from
    /// `assets/ayu-graphite.ron`.
    fn default() -> Self {
        Self::build(&Palette::load())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::SerdeFormat;

    use palantir::Color;
    use static_assertions::assert_not_impl_any;

    assert_not_impl_any!(Theme: Copy);
    assert_not_impl_any!(ChromeColors: Copy);
    assert_not_impl_any!(CanvasTheme: Copy);
    assert_not_impl_any!(CardTheme: Copy);
    assert_not_impl_any!(PortTheme: Copy);
    assert_not_impl_any!(StatusColors: Copy);
    assert_not_impl_any!(TypeColors: Copy);
    assert_not_impl_any!(CardBorder: Copy);
    assert_not_impl_any!(ConstValueEditorTheme: Copy);
    assert_not_impl_any!(InlineRenameTheme: Copy);

    /// The whole bundle — darkroom's own fields *and* the nested
    /// palantir palette — must survive a RON round-trip. That is the
    /// on-disk theme format, and this is the only thing holding it: no
    /// UI writes or reads one. Exercises the awkward case too — the
    /// tooltip's infinite max-size axis, handled by `Size`'s custom serde.
    #[test]
    fn theme_roundtrips_through_ron() {
        let mut theme = Theme::default();
        theme.card.min_width = 137.5;
        theme.colors.text_muted = Color::hex(0x123456);
        theme.palantir_theme.window_clear = Color::hex(0xabcdef);

        let bytes = common::serialize(&theme, SerdeFormat::Ron).expect("serialize theme");
        let back: Theme = common::deserialize(&bytes, SerdeFormat::Ron)
            .expect("theme should deserialize from its own RON output");

        assert_eq!(back.card.min_width, 137.5);
        assert_eq!(back.colors.text_muted, Color::hex(0x123456));
        assert_eq!(back.canvas.bg, theme.canvas.bg);
        // Nested palantir palette round-trips too.
        assert_eq!(back.palantir_theme.window_clear, Color::hex(0xabcdef));
        // The infinite tooltip-height axis survives `Size`'s serde.
        assert!(back.palantir_theme.tooltip.max_size.h.is_infinite());
        assert_eq!(back.palantir_theme.tooltip.max_size.w, 280.0);
    }

    /// Pin which palette role reaches which field, plus the non-trivial
    /// palantir tweak, so a regression in `Theme::build`'s wiring, in
    /// `palantir_theme_for`, or in `menu_button_for` fails loudly. Against
    /// the loaded palette rather than hex literals: the values are the
    /// palette's to choose, but landing `header_fill` in `canvas.bg` is
    /// still a bug.
    #[test]
    fn default_wiring_and_menu_tweak() {
        let p = Palette::load();
        let theme = Theme::default();
        assert_eq!(theme.canvas.bg, p.canvas_bg);
        assert_eq!(theme.card.header_fill, p.header_fill);
        assert_eq!(theme.ports.input, p.input_port);
        assert_eq!(theme.ports.output, p.output_port);
        assert_eq!(theme.colors.badge_cache, p.badge_cache);
        assert_eq!(theme.colors.badge_impure, p.badge_impure);
        assert_eq!(theme.status.busy, p.status_busy);
        // Each of those is a distinct role, so a palette that collapsed them
        // onto one colour would pass every assertion above.
        assert_ne!(theme.canvas.bg, theme.card.header_fill);
        assert_ne!(theme.ports.input, theme.ports.output);
        // The palantir half is the same palette, not palantir's own default.
        assert_eq!(theme.palantir_theme.window_clear, p.canvas_bg);
        assert!(theme.palantir_theme.tooltip.max_size.h.is_infinite());
        // The menu-bar font was shrunk from palantir's default to ours.
        let menu_text = theme
            .menu_button
            .looks
            .normal
            .text
            .expect("menu button carries an explicit text style");
        assert_eq!(menu_text.font_size_px, theme.text.body);
    }

    /// Every badge is legible against every other, because a node's head
    /// can carry several at once.
    ///
    /// A badge sharing a hue with a wire type or a status is fine and
    /// common — the palette resolves both against one upstream semantic
    /// layer, and the two never share a surface. Two badges do, so this is
    /// the pair the roster cannot be allowed to collapse. Like
    /// [`chrome_surfaces_stack_darkest_first`], it is a rule a table of
    /// colours cannot state about itself.
    #[test]
    fn badges_are_told_apart_from_each_other() {
        let c = Theme::default().colors;
        let badges = [
            ("badge_graph", c.badge_graph),
            ("badge_sink", c.badge_sink),
            ("badge_cache", c.badge_cache),
            ("badge_impure", c.badge_impure),
        ];
        for (i, (name, color)) in badges.iter().enumerate() {
            for (other_name, other) in &badges[i + 1..] {
                assert_ne!(
                    color, other,
                    "{name} and {other_name} share a hue, and a head can show both",
                );
            }
        }
    }

    /// The six chrome surfaces stack in one view — a graph, the bar around
    /// it, an inactive tab, a node, a hovered control, a pressed one — so
    /// each must be lighter than the one under it. The palette's generator
    /// checks this before it writes the file; this is the half that would
    /// catch a hand-edited asset, and it is the one rule a table of colours
    /// cannot state about itself.
    #[test]
    fn chrome_surfaces_stack_darkest_first() {
        let p = Palette::load();
        let ladder = [
            ("canvas_bg", p.canvas_bg),
            ("chrome_fill", p.chrome_fill),
            ("tab_inactive", p.tab_inactive),
            ("node_fill", p.node_fill),
            ("elem_mid", p.elem_mid),
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

    /// Relative luminance, the WCAG definition. Local to the test: nothing
    /// darkroom draws needs it, and the palette it checks is generated by a
    /// tool that computes the same number.
    fn luminance(c: Color) -> f32 {
        let channel = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let srgb = c.to_srgb_u8();
        0.2126 * channel(f32::from(srgb.r) / 255.0)
            + 0.7152 * channel(f32::from(srgb.g) / 255.0)
            + 0.0722 * channel(f32::from(srgb.b) / 255.0)
    }
}
