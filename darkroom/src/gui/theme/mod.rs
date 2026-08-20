//! [`Theme`]: darkroom's visual palette and layout dimensions, plus the
//! per-widget rosters hanging off it.
//!
//! Each roster owns a file and is declared through
//! [`palette_struct!`](palette_struct::palette_struct), which mints its `DARK`
//! and `LIGHT` presets alongside it. The palantir-side half of a preset lives
//! in [`palantir_bridge`].

pub(crate) mod canvas_theme;
pub(crate) mod card_theme;
pub(crate) mod color;
pub(crate) mod const_value_editor_theme;
pub(crate) mod hover_color;
pub(crate) mod inline_rename_theme;
pub(crate) mod palantir_bridge;
pub(crate) mod palette_colors;
pub(crate) mod palette_struct;
pub(crate) mod port_theme;
pub(crate) mod status_colors;
mod swatches;
pub(crate) mod type_colors;
pub(crate) mod type_scale;

use palantir::{ButtonTheme, FontWeight, TextStyle};

// Layout dimensions are palette-independent — dark and light pull the same
// numbers. Each one's value lives on `Theme::build` (its field carries the doc
// comment); only the few read by more than one builder earn a name here. Font
// sizes are palette-independent too, and live on `TypeScale::DEFAULT`.
use crate::core::theme_pref::ThemePreset;
use crate::gui::theme::canvas_theme::CanvasTheme;
use crate::gui::theme::card_theme::{CardBorder, CardTheme};
use crate::gui::theme::const_value_editor_theme::ConstValueEditorTheme;
use crate::gui::theme::inline_rename_theme::InlineRenameTheme;
use crate::gui::theme::palantir_bridge::{
    PALANTIR_DARK, PALANTIR_LIGHT, menu_button_for, palantir_theme_for,
};
use crate::gui::theme::palette_colors::PaletteColors;
use crate::gui::theme::port_theme::PortTheme;
use crate::gui::theme::status_colors::StatusColors;
use crate::gui::theme::swatches::{dark, light};
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
/// layout + colors) round-trips through serde for the Theme → Load /
/// Export menu.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Theme {
    // Scalar fields (`preset` + the few loose `f32`s) come first; the tables
    // (the per-widget groups, `colors`, `palantir_theme`) follow. TOML
    // serialization requires every scalar value to precede any table at the
    // same level — otherwise the serializer errors with `ValueAfterTable`.
    /// Which built-in preset assembled this theme. Round-trips
    /// through TOML so a user-loaded file restores the same toggle
    /// behaviour the original `Theme::dark` / `light` had.
    pub(crate) preset: ThemePreset,

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

    /// A node's ports: swatches, label ink, column geometry (`[ports]`).
    pub(crate) ports: PortTheme,

    /// The semantic feedback palette — success / info / busy / warning /
    /// error (`[status]`).
    pub(crate) status: StatusColors,

    /// The chrome colours belonging to no single widget (`[colors]`).
    pub(crate) colors: PaletteColors,

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
    /// style pinned to [`FontWeight::Bold`] on every state. Precomputed
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
    /// call site restyling per use. Last field so its TOML table
    /// follows all the scalar fields above (TOML `ValueAfterTable`).
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

    /// Assemble the full theme for a built-in preset. One place so
    /// startup and the Theme menu share the preset → palette mapping.
    pub(crate) fn from_preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::Dark => Self::dark(),
            ThemePreset::Light => Self::light(),
        }
    }

    /// Ayu Mirage High Contrast palette — the built-in dark look.
    pub(crate) fn dark() -> Self {
        Self::build(ThemePreset::Dark, dark::TYPE_COLORS, &PALANTIR_DARK)
    }

    /// Ayu Light palette — the built-in light look (Zed's "Ayu Light"
    /// variant ported into darkroom's structure).
    pub(crate) fn light() -> Self {
        Self::build(ThemePreset::Light, light::TYPE_COLORS, &PALANTIR_LIGHT)
    }

    /// Shared assembly path — the darkroom peer of
    /// `palantir::Theme::from_palette`: dimensions are
    /// palette-independent; `colors` / `type_colors` (moved in, not
    /// copied) drive darkroom chrome, and every sub-recipe (the
    /// palantir widget theme, the static-value editor, inline rename)
    /// cascades from `p` here rather than being hand-assembled per
    /// preset. `preset` tags which built-in produced this theme so the
    /// toggle command doesn't have to guess.
    fn build(preset: ThemePreset, type_colors: TypeColors, p: &palantir::Palette) -> Self {
        let colors = PaletteColors::for_preset(preset);
        let chrome_fill = colors.chrome_fill;
        // Built before the struct literal because the title variant
        // derives from the palantir theme's ambient text style — the
        // same style an unstyled rename would have inherited anyway,
        // so bolding it is the only difference between the two slots.
        let palantir_theme = palantir_theme_for(p, chrome_fill, &TypeScale::DEFAULT);
        let inline_rename = InlineRenameTheme::from_palette(p);
        let inline_rename_title = inline_rename.clone().with_text(TextStyle {
            weight: FontWeight::Bold,
            ..palantir_theme.text.clone()
        });
        Self {
            preset,
            // The three measurements that belong to no widget group; the
            // rest are authored beside their colours in the groups below.
            stroke_width: 2.0,
            floating_widget_gap: 16.0,
            new_node_popup_max_height: 400.0,
            text: TypeScale::DEFAULT,
            canvas: CanvasTheme::for_preset(preset),
            card: CardTheme::for_preset(preset),
            ports: PortTheme::for_preset(preset),
            status: StatusColors::for_preset(preset),
            colors,
            type_colors,
            const_value_editor: ConstValueEditorTheme::from_palette(p),
            const_value_editor_revealed: ConstValueEditorTheme::revealed_from_palette(p),
            inline_rename,
            inline_rename_title,
            menu_button: menu_button_for(p, &palantir_theme.text, &TypeScale::DEFAULT),
            palantir_theme,
        }
    }
}

impl Default for Theme {
    /// Defaults to [`Theme::dark`] — the historical look. The asset
    /// `assets/ayu-graphite.toml` is regenerated from this by
    /// `tests::ayu_graphite_asset_in_sync`.
    fn default() -> Self {
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::SerdeFormat;

    use crate::core::theme_pref::ThemeChoice;
    use crate::gui::theme::hover_color::HoverColor;
    use palantir::Color;
    use static_assertions::assert_not_impl_any;

    assert_not_impl_any!(Theme: Copy);
    assert_not_impl_any!(PaletteColors: Copy);
    assert_not_impl_any!(CanvasTheme: Copy);
    assert_not_impl_any!(CardTheme: Copy);
    assert_not_impl_any!(PortTheme: Copy);
    assert_not_impl_any!(StatusColors: Copy);
    assert_not_impl_any!(TypeColors: Copy);
    assert_not_impl_any!(HoverColor: Copy);
    assert_not_impl_any!(CardBorder: Copy);
    assert_not_impl_any!(ConstValueEditorTheme: Copy);
    assert_not_impl_any!(InlineRenameTheme: Copy);

    /// The checked-in `assets/ayu-graphite.toml` is a generated artifact — a
    /// reference theme users can copy, in the Theme → Load/Export format — so
    /// it has to track [`Theme::default`]. This *reads* it: any change to the
    /// consts (or to palantir's defaults) fails here rather than silently
    /// rewriting a tracked file mid-suite, which is what it used to do and why
    /// it could never fail.
    #[test]
    fn ayu_graphite_asset_in_sync() {
        let expected = serialized_default_theme();
        let on_disk = std::fs::read(ayu_graphite_path()).expect("the asset is checked in");
        assert!(
            on_disk == expected,
            "assets/ayu-graphite.toml no longer matches Theme::default — regenerate it with \
             `cargo test -p darkroom --all-features regenerate_ayu_graphite_asset -- --ignored`",
        );
    }

    /// Rewrite the asset from the current defaults. Ignored by default: it is
    /// the generator behind [`ayu_graphite_asset_in_sync`], not a check, and a
    /// suite that regenerates its own fixtures cannot detect a drift.
    #[test]
    #[ignore = "regenerates a tracked asset; run explicitly after changing the theme consts"]
    fn regenerate_ayu_graphite_asset() {
        std::fs::write(ayu_graphite_path(), serialized_default_theme()).expect("write toml asset");
    }

    fn serialized_default_theme() -> Vec<u8> {
        common::serialize(&Theme::default(), SerdeFormat::Toml).expect("serialize theme")
    }

    /// Anchored at the manifest rather than the working directory, so the two
    /// halves above agree regardless of where the runner was invoked.
    fn ayu_graphite_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ayu-graphite.toml")
    }

    /// The whole bundle — darkroom's own fields *and* the nested
    /// palantir palette — must survive a TOML round-trip; that's the
    /// on-disk format the Theme → Load / Export menu and the preferences
    /// rely on. Exercises the formerly-fragile case too: the tooltip's
    /// infinite max-size axis (handled by `Size`'s custom serde).
    #[test]
    fn theme_roundtrips_through_toml() {
        let mut theme = Theme::default();
        theme.card.min_width = 137.5;
        theme.colors.text_muted = Color::hex(0x123456);
        theme.palantir_theme.window_clear = Color::hex(0xabcdef);

        let bytes = common::serialize(&theme, SerdeFormat::Toml).expect("serialize theme");
        let back: Theme = common::deserialize(&bytes, SerdeFormat::Toml)
            .expect("theme should deserialize from its own TOML output");

        assert_eq!(back.card.min_width, 137.5);
        assert_eq!(back.colors.text_muted, Color::hex(0x123456));
        assert_eq!(back.canvas.bg, theme.canvas.bg);
        // Nested palantir palette round-trips too.
        assert_eq!(back.palantir_theme.window_clear, Color::hex(0xabcdef));
        // The infinite tooltip-height axis survives `Size`'s serde.
        assert!(back.palantir_theme.tooltip.max_size.h.is_infinite());
        assert_eq!(back.palantir_theme.tooltip.max_size.w, 280.0);
    }

    /// Pin which swatch reaches which field, plus the non-trivial palantir
    /// tweak, so a regression in `Theme::build`'s wiring, in
    /// `palantir_theme_for`, or in `menu_button_for` fails loudly. Against
    /// the generated consts
    /// rather than hex literals: the values are the palette's to choose, but
    /// landing `HEADER_FILL` in `canvas.bg` is still a bug.
    #[test]
    fn default_palette_and_menu_tweak() {
        let theme = Theme::default();
        assert_eq!(theme.canvas.bg, dark::CANVAS_BG);
        assert_eq!(theme.ports.input.rest, dark::INPUT_PORT.rest);
        assert_eq!(theme.ports.output.rest, dark::OUTPUT_PORT.rest);
        assert_eq!(theme.colors.badge_cache, dark::BADGE_CACHE);
        assert_eq!(theme.colors.badge_impure, dark::BADGE_IMPURE);
        // Each of those is a distinct role, so a roster that collapsed them
        // onto one swatch would pass every assertion above.
        assert_ne!(theme.canvas.bg, theme.ports.input.rest);
        assert_ne!(theme.ports.input.rest, theme.ports.output.rest);
        assert_ne!(theme.colors.badge_cache, theme.colors.badge_impure);
        assert_eq!(theme.card.min_width, 160.0);
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

    /// `from_preset` round-trips the tag both ways — the assembled theme
    /// carries the preset it was asked for and swaps the full palette,
    /// not just the tag. The builders stamp the matching preset too.
    #[test]
    fn from_preset_maps_both_presets() {
        let dark = Theme::from_preset(ThemePreset::Dark);
        let light = Theme::from_preset(ThemePreset::Light);
        assert_eq!(dark.preset, ThemePreset::Dark);
        assert_eq!(light.preset, ThemePreset::Light);
        assert_eq!(Theme::dark().preset, ThemePreset::Dark);
        assert_eq!(Theme::light().preset, ThemePreset::Light);
        // Full palette swapped, not just the tag.
        assert_eq!(dark.canvas.bg, dark::CANVAS_BG);
        assert_eq!(light.canvas.bg, light::CANVAS_BG);
        assert_ne!(dark.canvas.bg, light.canvas.bg);
    }

    /// System detection must always resolve to one of the two built-in
    /// presets (its `Unspecified`/error arms fold to `Dark`), so the
    /// startup fallback can hand the result straight to `from_preset`.
    #[test]
    fn from_system_resolves_to_built_in_preset() {
        let preset = ThemePreset::from_system();
        assert!(matches!(preset, ThemePreset::Dark | ThemePreset::Light));
    }

    /// `ThemeChoice` resolution: the explicit choices map straight to
    /// their preset, and `System` defers to OS detection — which itself
    /// always lands on a concrete preset.
    #[test]
    fn theme_choice_resolves_to_preset() {
        assert_eq!(ThemeChoice::Dark.resolve(), ThemePreset::Dark);
        assert_eq!(ThemeChoice::Light.resolve(), ThemePreset::Light);
        assert_eq!(ThemeChoice::System.resolve(), ThemePreset::from_system());
        // System is the default preference — fresh launches follow the OS.
        assert_eq!(ThemeChoice::default(), ThemeChoice::System);
    }
}
