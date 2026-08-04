pub(crate) mod color;

use palantir::{
    Brush, ButtonTheme, Color, DragValueTheme, FontWeight, Shadow, Spacing, Stroke, TextEditTheme,
    TextStyle, WidgetLook,
};

use crate::core::theme_pref::ThemeChoice;

// Layout dimensions are palette-independent — dark and light pull the same
// numbers. Each one's value lives on `Theme::build` (its field carries the doc
// comment); only the few read by more than one builder earn a name here. Font
// sizes are palette-independent too, and live on `TypeScale::DEFAULT`.

const VALUE_EDITOR_WIDTH: f32 = 100.0;
/// Upper bound on the value column: editors fill the column up to here, then a
/// long value (a wide enum/preset dropdown, a long path) ellipsizes instead of
/// stretching the node out. Read by both `Theme::build` and
/// [`ConstValueEditorTheme::from_palette`], which sizes the editor itself.
const VALUE_EDITOR_MAX_WIDTH: f32 = 240.0;

// One named-const mod per built-in preset, so any builder (`Theme::dark`,
// `ConstValueEditorTheme::dark`, future per-widget theme helpers) can
// reach a swatch by name instead of inlining a hex literal. The two
// mods line up 1:1 — every name in `dark::*` has a `light::*` peer with
// the matching role.
//
// Sourced from the semantic palette TOMLs in `assets/`:
//   - `dark`  — `ayu-graphite-palette.toml` (Ayu Mirage High Contrast)
//   - `light` — `ayu-light-palette.toml`    (Zed's "Ayu Light")
// The toml files are the hand-edited reference; the consts here are the
// compile-time copy. Keep in sync when the palette changes.

pub(crate) mod dark {
    use super::{HoverColor, TypeColors};
    use palantir::Color;

    pub(crate) const CANVAS_BG: Color = Color::hex(0x1a1a1a);
    pub(crate) const SELECTION_RECT: Color = Color::hex(0x9adbfb);
    pub(crate) const CANVAS_DOT: Color = Color::hex(0x363636);

    pub(crate) const CONNECTION_BROKEN: Color = Color::hex(0xff5e44);
    pub(crate) const BREAKER_STROKE: Color = Color::hex(0xff5e44);

    pub(crate) const NODE_FILL: Color = Color::hex(0x343434);
    // Transparent at rest: the ambient node shadow carries the edge, and the
    // stroke slot is reserved for the selection / breaker / missing colors
    // (its width still folds into layout, so selecting never resizes).
    pub(crate) const NODE_BORDER: Color = Color::TRANSPARENT;
    // Palette `elem_active` — a step brighter than the old `title_bar`
    // swatch so the header band actually reads against the body fill.
    pub(crate) const HEADER_FILL: Color = Color::hex(0x4b4b4b);
    pub(crate) const TEXT_MUTED: Color = Color::hex(0xaaaaa8);
    // Port/event labels: de-emphasized so the value column carries each row.
    pub(crate) const PORT_LABEL: Color = Color::hex(0xaaaaa8);
    // Ambient elevation shadow under nodes and floating panels. Heavy black:
    // a near-black canvas needs a lot of alpha before a shadow registers.
    pub(crate) const NODE_AMBIENT_SHADOW: Color = Color::linear_rgba(0.0, 0.0, 0.0, 0.5);
    pub(crate) const CHROME_FILL: Color = Color::hex(0x252525);
    // Inactive tab chip — a notch above `CHROME_FILL` toward the node
    // surface, so an unselected tab reads as a resting chip, not a bare
    // label on the band.
    pub(crate) const TAB_INACTIVE: Color = Color::hex(0x2e2e2e);

    pub(crate) const BADGE_GRAPH: Color = Color::hex(0x9adbfb);
    pub(crate) const BADGE_SINK: Color = Color::hex(0xff5e44);
    // cache (persist-to-disk) chip — palette `warning` yellow.
    pub(crate) const BADGE_CACHE: Color = Color::hex(0xffd44a);
    // impure marker — a saturated violet (the "volatile / recomputes every run"
    // hue). Deliberately punchier than the pale running-glow purple so the `~`
    // marker reads at a glance.
    pub(crate) const BADGE_IMPURE: Color = Color::hex(0xc56cff);

    pub(crate) const STATUS_SUCCESS: Color = Color::hex(0xdaff58);
    pub(crate) const STATUS_INFO: Color = Color::hex(0x9adbfb);
    pub(crate) const STATUS_BUSY: Color = Color::hex(0xd4bfff);
    pub(crate) const STATUS_WARNING: Color = Color::hex(0xffa63d);
    pub(crate) const STATUS_ERROR: Color = Color::hex(0xff5e44);

    // ports — hover variants brighten for emphasis on a dark canvas.
    pub(crate) const INPUT_PORT: HoverColor = HoverColor {
        rest: Color::hex(0xdaff58),
        hover: Color::hex(0xe9ff8e),
    };
    pub(crate) const OUTPUT_PORT: HoverColor = HoverColor {
        rest: Color::hex(0xffa63d),
        hover: Color::hex(0xffc878),
    };
    // Events wear the palette's `error` red — the same swatch as the
    // sink `■` marker the subscription pin sits beside, so the trigger
    // machinery reads as one family. Shape (triangle vs. circle) keeps
    // events apart from data ports; hover lifts toward white like the
    // typed port hovers.
    pub(crate) const EVENT_PORT: HoverColor = HoverColor {
        rest: Color::hex(0xff5e44),
        hover: Color::hex(0xff8b78),
    };

    // data-type hues (wires + typed port circles) — hand-tuned to
    // harmonize with the palette. The ramp deliberately carries no rose
    // (Image owns it) and no purple (the running/impure status family),
    // so a hash pick can't impersonate either.
    pub(crate) const TYPE_COLORS: TypeColors = TypeColors {
        boolean: Color::hex(0xf28779),
        int: Color::hex(0x95e6cb),
        float: Color::hex(0x73d0ff),
        string: Color::hex(0xffd173),
        path: Color::hex(0xd4bfff),
        // Safelight rose — the photographic-darkroom hue for the image
        // payload.
        image: Color::hex(0xff9eb5),
        ramp: [
            Color::hex(0xffa759),
            Color::hex(0x7bd88f),
            Color::hex(0x5ccfe6),
            Color::hex(0xe6cd8a),
        ],
    };

    // palantir sub-theme palette — values palantir's widgets normally
    // read from its own `palette::*` consts. Pushed through
    // `PalantirPalette` so the live `ui.theme` recolours alongside
    // darkroom chrome; reused by `ConstValueEditorTheme::dark` for
    // the per-palette path-pick chip.
    pub(crate) const PAL_TEXT: Color = Color::hex(0xe2dfd3);
    pub(crate) const PAL_TEXT_DISABLED: Color = Color::hex(0x878a8d);
    pub(crate) const PAL_ELEM_HOVER: Color = Color::hex(0x3e3e3e);
    pub(crate) const PAL_ELEM_ACTIVE: Color = Color::hex(0x4b4b4b);
    pub(crate) const PAL_BORDER_FOCUSED: Color = Color::hex(0x105577);
}

pub(crate) mod light {
    use super::{HoverColor, TypeColors};
    use palantir::Color;

    pub(crate) const CANVAS_BG: Color = Color::hex(0xfcfcfc);
    pub(crate) const SELECTION_RECT: Color = Color::hex(0x3b9ee5);
    pub(crate) const CANVAS_DOT: Color = Color::hex(0xcfd1d2);

    pub(crate) const CONNECTION_BROKEN: Color = Color::hex(0xef7271);
    pub(crate) const BREAKER_STROKE: Color = Color::hex(0xef7271);

    // node chrome — light surfaces keep the hairline border even with the
    // ambient shadow; a shadow alone reads mushy on near-white.
    pub(crate) const NODE_FILL: Color = Color::hex(0xececed);
    pub(crate) const NODE_BORDER: Color = Color::hex(0xcfd1d2);
    pub(crate) const HEADER_FILL: Color = Color::hex(0xdcddde);
    pub(crate) const TEXT_MUTED: Color = Color::hex(0x8b8e92);
    // Darker than `text_muted`: labels are primary content and Ayu Light's
    // muted gray drops under 3:1 on the node fill.
    pub(crate) const PORT_LABEL: Color = Color::hex(0x6e7378);
    // Light surfaces need far less shadow than the dark canvas.
    pub(crate) const NODE_AMBIENT_SHADOW: Color = Color::linear_rgba(0.0, 0.0, 0.0, 0.2);
    pub(crate) const CHROME_FILL: Color = Color::hex(0xdcddde);
    // Inactive tab chip — a notch above `CHROME_FILL` toward the node
    // surface, so an unselected tab reads as a chip on the light band.
    pub(crate) const TAB_INACTIVE: Color = Color::hex(0xe6e7e8);

    // header badges — accent / error / a deeper amber than the palette's
    // warning yellow (#f1ad49 was barely visible on a light surface).
    pub(crate) const BADGE_GRAPH: Color = Color::hex(0x3b9ee5);
    pub(crate) const BADGE_SINK: Color = Color::hex(0xef7271);
    // cache (persist-to-disk) chip — palette `warning` yellow.
    pub(crate) const BADGE_CACHE: Color = Color::hex(0xf1ad49);
    // impure marker — a saturated violet, punchier than the running-glow purple
    // so the `~` marker reads at a glance on the light ground.
    pub(crate) const BADGE_IMPURE: Color = Color::hex(0x9333d6);

    // execution-status glow — success / accent / syn_keyword / error.
    pub(crate) const STATUS_SUCCESS: Color = Color::hex(0x85b304);
    pub(crate) const STATUS_INFO: Color = Color::hex(0x3b9ee5);
    pub(crate) const STATUS_BUSY: Color = Color::hex(0xa37acc);
    pub(crate) const STATUS_WARNING: Color = Color::hex(0xfa8d3e);
    pub(crate) const STATUS_ERROR: Color = Color::hex(0xef7271);

    // ports — input = success, output = syn_keyword. Hover variants on
    // the light canvas *darken* for emphasis (opposite to the dark theme).
    pub(crate) const INPUT_PORT: HoverColor = HoverColor {
        rest: Color::hex(0x85b304),
        hover: Color::hex(0x6f9603),
    };
    pub(crate) const OUTPUT_PORT: HoverColor = HoverColor {
        rest: Color::hex(0xfa8d3e),
        hover: Color::hex(0xd97527),
    };
    // Events wear the light palette's `error` red (see the dark peer's
    // rationale); hover darkens for emphasis like the light port hovers.
    pub(crate) const EVENT_PORT: HoverColor = HoverColor {
        rest: Color::hex(0xef7271),
        hover: Color::hex(0xb35555),
    };

    // data-type hues — the light peers of `dark::TYPE_COLORS` (deeper
    // values: light surfaces need saturation, not brightness).
    pub(crate) const TYPE_COLORS: TypeColors = TypeColors {
        boolean: Color::hex(0xe05252),
        int: Color::hex(0x2e9e5b),
        float: Color::hex(0x2b8fd6),
        string: Color::hex(0xb8860b),
        path: Color::hex(0x7a4fd0),
        image: Color::hex(0xc23b73),
        ramp: [
            Color::hex(0xd9722a),
            Color::hex(0x1f8fb3),
            Color::hex(0x2f9e6a),
            Color::hex(0xa67c1a),
        ],
    };

    // palantir sub-theme palette — see `dark::PAL_*` for the contract.
    pub(crate) const PAL_TEXT: Color = Color::hex(0x5c6166);
    pub(crate) const PAL_TEXT_DISABLED: Color = Color::hex(0xa9acae);
    pub(crate) const PAL_ELEM_HOVER: Color = Color::hex(0xdfe0e1);
    pub(crate) const PAL_ELEM_ACTIVE: Color = Color::hex(0xcfd0d2);
    pub(crate) const PAL_BORDER_FOCUSED: Color = Color::hex(0xc4daf6);
}

/// Two-state colour pack for chrome that lifts under the pointer —
/// the colour-granularity peer of palantir's `StatefulLook`: the pair
/// is structural (a hover variant can't exist without its rest), and
/// state → colour goes through one `pick`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HoverColor {
    pub(crate) rest: Color,
    pub(crate) hover: Color,
}

impl HoverColor {
    #[inline]
    pub(crate) fn pick(&self, hovered: bool) -> Color {
        if hovered { self.hover } else { self.rest }
    }
}

/// Data-type → wire/port-circle hue roster (consumed by
/// `gui::pane::graph::node::port_color`). Serialized as the theme's `[type_colors]`
/// table so a loaded theme file can restyle type hues like any other
/// swatch. `ramp` backs the open-ended `Custom`/`Enum` families —
/// keyed by `type_id`, so distinct custom types land on stable,
/// distinct colors; `image` is the fixed hue the lens image type owns.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeColors {
    pub(crate) boolean: Color,
    pub(crate) int: Color,
    pub(crate) float: Color,
    pub(crate) string: Color,
    pub(crate) path: Color,
    pub(crate) image: Color,
    pub(crate) ramp: [Color; 4],
}

/// Declares a colour-roster struct plus its two built-in instances
/// (`DARK` / `LIGHT`, pulling `dark::CONST` / `light::CONST`) from one
/// `field: Ty => CONST` list. One roster per struct, so a colour can't
/// sit in the struct while a preset forgets it: the presets won't
/// compile until every field is filled. The serialized
/// [`PaletteColors`] chrome roster is built this way; the
/// palantir-side rosters are plain [`palantir::Palette`] consts
/// (`PALANTIR_DARK` / `PALANTIR_LIGHT`).
///
/// Fields listed after a `;` are palette-independent — layout measurements,
/// mostly — given as `field: Ty = value` and copied verbatim into both
/// presets. That is what lets a per-widget group ([`CardTheme`],
/// [`PortTheme`], [`CanvasTheme`]) hold its geometry beside its colours
/// without the numbers being authored twice.
macro_rules! palette_struct {
    (
        $(#[$smeta:meta])*
        $vis:vis struct $name:ident;
        $($(#[$fmeta:meta])* $field:ident: $fty:ty => $konst:ident),+ $(,)?
        $(; $($(#[$dmeta:meta])* $dfield:ident: $dty:ty = $dval:expr),+ $(,)?)?
    ) => {
        $(#[$smeta])*
        $vis struct $name {
            $($(#[$fmeta])* $vis $field: $fty,)+
            $($($(#[$dmeta])* $vis $dfield: $dty,)+)?
        }

        impl $name {
            const DARK: Self = Self {
                $($field: dark::$konst,)+
                $($($dfield: $dval,)+)?
            };
            const LIGHT: Self = Self {
                $($field: light::$konst,)+
                $($($dfield: $dval,)+)?
            };

            /// This roster for `preset` — the per-group half of
            /// [`Theme::from_preset`].
            fn for_preset(preset: ThemePreset) -> Self {
                match preset {
                    ThemePreset::Dark => Self::DARK,
                    ThemePreset::Light => Self::LIGHT,
                }
            }
        }
    };
}

/// Which built-in palette built this [`Theme`] — the concrete palette
/// a [`ThemeChoice`] resolves to. Carried on the theme itself and
/// round-tripped through TOML so a loaded theme file restores its
/// origin palette. `Default = Dark` so a hand-rolled `Theme` (e.g. the
/// deserialised round-trip used by tests) has a deterministic tag
/// without callers having to spell it out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemePreset {
    #[default]
    Dark,
    Light,
}

impl ThemePreset {
    /// The OS's current light/dark preference, falling back to
    /// [`Dark`](Self::Dark) when the platform reports no preference or
    /// detection fails. Backs [`ThemeChoice::System`].
    pub(crate) fn from_system() -> Self {
        match dark_light::detect() {
            Ok(dark_light::Mode::Light) => Self::Light,
            Ok(dark_light::Mode::Dark | dark_light::Mode::Unspecified) | Err(_) => Self::Dark,
        }
    }
}

impl ThemeChoice {
    /// Resolve to the concrete built-in preset to load. `System` queries
    /// the OS (falling back to dark); `Dark` / `Light` map straight
    /// through.
    pub(crate) fn resolve(self) -> ThemePreset {
        match self {
            Self::System => ThemePreset::from_system(),
            Self::Dark => ThemePreset::Dark,
            Self::Light => ThemePreset::Light,
        }
    }
}

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

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, TextEdit, MenuItem,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use. Last field so its TOML table
    /// follows all the scalar fields above (TOML `ValueAfterTable`).
    pub(crate) palantir_theme: palantir::Theme,
}

/// Font sizes by tier in the visual hierarchy — the typographic half of a
/// [`Theme`], beside the [`PaletteColors`] palette half and the layout
/// dimensions.
///
/// Named by *prominence*, never by the surface that happens to use a tier, so
/// a new surface picks one by asking how loud its text should be rather than
/// copying whichever number a neighbouring panel reached for. Every size the
/// app draws is here: a literal at a call site is a missing tier, not a local
/// decision.
///
/// Palette-independent like the dimensions — dark and light pull the same
/// numbers, so unlike [`PaletteColors`] there is no preset pair to keep in
/// step and no macro generating one; a single [`Self::DEFAULT`] is the whole
/// story.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeScale {
    /// The loudest tier: a floating panel's own heading (the inspector's node
    /// title) and the "+" chip's glyph, which wants the same presence.
    pub(crate) title: f32,
    /// Default UI text — dock tab labels, menu rows, settings rows, the drag
    /// ghost, the status bar, a settings row's help/error/link line, an
    /// inspector row's value. The tier to reach for when none of the others
    /// has a reason to win.
    pub(crate) body: f32,
    /// Labels on dense surfaces, where body would crowd: inspector port rows,
    /// a node's preview row, the viewer's swatch caption, and the tabular
    /// figures beside them (byte counts, dimensions) — the mono family comes
    /// from [`crate::gui::widgets::support::mono_text`], not from a tier of
    /// its own.
    pub(crate) label: f32,
    /// The caption above a readout — the smallest type that ships.
    pub(crate) caption: f32,
}

impl TypeScale {
    /// The authored scale. Four tiers, each a step the eye can actually
    /// resolve: the 15/14 and 13/12 and 11/10.5 pairs this replaced sat a
    /// half-step or a point apart and read as one size, so the smaller of
    /// each was doing no work its neighbour wasn't.
    ///
    /// Badge glyphs stay out: a `■` sized to an 18px chip box is geometry
    /// that happens to be drawn with a font, tracking the box rather than the
    /// hierarchy, so it stays named beside the box (`BADGE_FONT`).
    const DEFAULT: Self = Self {
        title: 15.0,
        body: 13.0,
        label: 11.0,
        caption: 8.5,
    };
}

/// Per-widget theme bundle for the inline static-value editor on a
/// `Binding::Const` input port. Owns the `DragValue` look (scrub chip —
/// transparent at rest, hover-only background, no border — plus the inline
/// editor derived from it) which the numeric fields use directly and the
/// `Button`/`ComboBox` siblings (path pick, enum, presets) borrow via
/// `drag_value.chip`, and the fixed field width.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConstValueEditorTheme {
    pub(crate) drag_value: DragValueTheme,
    /// Minimum logical-px width of the value column — editors fill it down to
    /// at least this.
    pub(crate) width: f32,
    /// Maximum logical-px width of the value column, so a wide editor (enum /
    /// preset dropdown, long path) ellipsizes rather than stretching the node.
    pub(crate) max_width: f32,
}

impl ConstValueEditorTheme {
    /// The pointer-over-node variant of [`Self::from_palette`]: the
    /// chip's hover fill (`elem_hover`), at reduced alpha, becomes the
    /// *resting* background — const editors surface as soon as the
    /// pointer is anywhere over the node, without waiting for a direct
    /// hover. Fill only, so geometry is identical to the resting look.
    /// Built from the same palette recipe rather than patching a
    /// finished theme, so it can't drift from what the recipe painted.
    ///
    /// Both the resting `chip` (numeric editors, which show it at rest) and
    /// the inline `editor`'s normal state (string/`Any` editors, which are
    /// always a `TextEdit` and so show `editor.normal` at rest) get the same
    /// fill, so every field's edit affordance surfaces together.
    fn revealed_from_palette(p: &palantir::Palette) -> Self {
        const REVEAL_ALPHA: f32 = 0.5;
        let mut out = Self::from_palette(p);
        let reveal = Brush::Solid(p.elem_hover.with_alpha(REVEAL_ALPHA));
        for look in [
            out.drag_value.chip.looks.normal.background.as_mut(),
            out.drag_value.editor.looks.normal.background.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            look.fill = reveal.clone();
        }
        out
    }

    /// Shared shape: palantir's `menu_button` preset over `p` (transparent
    /// at rest + disabled, no border) as the chip, with the inline editor
    /// derived from that chip so both modes share one box, and
    /// caret/selection/placeholder from the same palette's text-edit
    /// recipe so it matches the app's other text fields.
    fn from_palette(p: &palantir::Palette) -> Self {
        Self {
            drag_value: DragValueTheme::from_chip(
                ButtonTheme::menu_button(p),
                &TextEditTheme::from_palette(p),
            ),
            width: VALUE_EDITOR_WIDTH,
            max_width: VALUE_EDITOR_MAX_WIDTH,
        }
    }
}

/// Per-widget theme bundle for the inline-rename label⇄field widget
/// (node title, boundary-port name, graph tab). The `text_edit`
/// look is stripped to the bare editor surface (no padding/margin, no
/// border, transparent fill) so the field's `Hug` height equals its
/// plain `Text` twin and the row doesn't reshape on a swap.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct InlineRenameTheme {
    pub(crate) text_edit: TextEditTheme,
}

impl InlineRenameTheme {
    /// Shared shape: start from the palette's text-edit recipe (which
    /// already carries the right caret / placeholder / selection), then
    /// strip every visual that would reshape the row (padding, margin,
    /// border, fill) so the field reads against whichever canvas hosts
    /// it.
    fn from_palette(p: &palantir::Palette) -> Self {
        Self::flattened(&TextEditTheme::from_palette(p))
    }

    /// The flattening half of [`Self::from_palette`], over an existing
    /// text-edit bundle rather than a palette — how an
    /// [`InlineRename`](crate::gui::widgets::inline_rename::InlineRename)
    /// with no `.style(…)` derives its look from ambient
    /// [`palantir::Theme::text_edit`].
    ///
    /// Split out so the ambient path and this theme's own slot are
    /// stripped by one function: a visual added here can't be flattened
    /// in the configured bundle and left standing in the fallback.
    pub(crate) fn flattened(text_edit: &TextEditTheme) -> Self {
        let mut style = TextEditTheme {
            padding: Spacing::ZERO,
            margin: Spacing::ZERO,
            ..text_edit.clone()
        };
        for look in [
            &mut style.looks.normal,
            &mut style.looks.hovered,
            &mut style.looks.active,
            &mut style.looks.disabled,
        ] {
            if let Some(bg) = look.background.as_mut() {
                bg.stroke = Stroke::ZERO;
                bg.fill = Brush::TRANSPARENT;
            }
        }
        Self { text_edit: style }
    }

    /// The same bundle with `text` pinned on every state.
    ///
    /// All four looks, not just `normal`: the idle label reads `normal`
    /// while the open editor resolves per state, so leaving the others
    /// to inherit would change the font when the field is hovered or
    /// focused — mid-rename, on the widget whose whole contract is that
    /// the glyphs don't move when it opens.
    pub(crate) fn with_text(mut self, text: TextStyle) -> Self {
        for look in [
            &mut self.text_edit.looks.normal,
            &mut self.text_edit.looks.hovered,
            &mut self.text_edit.looks.active,
            &mut self.text_edit.looks.disabled,
        ] {
            look.text = Some(text.clone());
        }
        self
    }
}

/// The [`palantir::Palette`] each preset hands to
/// [`palantir::Theme::from_palette`], filled from the preset's swatches
/// so swapping dark ⇄ light recolours every widget palantir paints, not
/// just darkroom-owned chrome. Notes on the mapping:
/// - `terminal_bg` wants the editor / terminal surface — the same
///   swatch as the graph canvas in both themes.
/// - `elem` and our `NODE_FILL` are the same swatch by design: nodes
///   and palantir surfaces sit on the same surface tier.
const PALANTIR_DARK: palantir::Palette = palantir::Palette {
    text: dark::PAL_TEXT,
    text_muted: dark::TEXT_MUTED,
    text_disabled: dark::PAL_TEXT_DISABLED,
    terminal_bg: dark::CANVAS_BG,
    elem: dark::NODE_FILL,
    elem_hover: dark::PAL_ELEM_HOVER,
    elem_active: dark::PAL_ELEM_ACTIVE,
    border_focused: dark::PAL_BORDER_FOCUSED,
    accent: dark::SELECTION_RECT,
};

/// Light peer of [`PALANTIR_DARK`] — same mapping over `light::*`.
const PALANTIR_LIGHT: palantir::Palette = palantir::Palette {
    text: light::PAL_TEXT,
    text_muted: light::TEXT_MUTED,
    text_disabled: light::PAL_TEXT_DISABLED,
    terminal_bg: light::CANVAS_BG,
    elem: light::NODE_FILL,
    elem_hover: light::PAL_ELEM_HOVER,
    elem_active: light::PAL_ELEM_ACTIVE,
    border_focused: light::PAL_BORDER_FOCUSED,
    accent: light::SELECTION_RECT,
};

/// Palantir sub-theme for darkroom: assemble every widget recipe from
/// the palette via [`palantir::Theme::from_palette`], then apply the
/// darkroom-only tweaks (smaller menu/context-menu font; menu-bar
/// triggers muted + transparent at rest so they read as menus, not
/// buttons).
///
/// Takes `text` rather than reaching for a private menu-font const: menu rows
/// are ordinary UI text, so they read [`TypeScale::body`] like every other
/// surface at that tier.
fn palantir_theme_for(
    p: &palantir::Palette,
    chrome_fill: Color,
    text: &TypeScale,
) -> palantir::Theme {
    let mut theme = palantir::Theme::from_palette(p);

    // Dock splitter: the resting seam paints the chrome band that frames
    // the panes, so the gap reads as part of that surround rather than a
    // dark line (hover/drag fill still marks the grab target); a wider
    // seam does the visual separation.
    theme.splitter.rule = chrome_fill;
    theme.splitter.rule_thickness = 4.0;

    // Menu-bar triggers read as menus, not buttons: transparent at rest
    // (the `menu_button` preset already is — no chip overlay), the label
    // muted until hovered, and the whole thing at the smaller menu scale.
    // hover/pressed keep the `elem_hover`/`elem_active` fills that
    // `recolour_palantir` set.
    let base = &theme.text;
    // Font-only shrink (keeps each look's own colour) for the context-menu
    // rows; menu-bar triggers also recolour per state, so they use `restyle`.
    let shrink = |look: &mut WidgetLook| {
        let style = look.text.take().unwrap_or_else(|| base.clone());
        look.text = Some(style.with_font_size(text.body));
    };
    let restyle = |look: &mut WidgetLook, color: Color| {
        let style = look.text.take().unwrap_or_else(|| base.clone());
        look.text = Some(style.with_color(color).with_font_size(text.body));
    };
    let mb = &mut theme.menu_button;
    restyle(&mut mb.looks.normal, p.text_muted);
    restyle(&mut mb.looks.hovered, p.text);
    restyle(&mut mb.looks.active, p.text);
    restyle(&mut mb.looks.disabled, p.text_disabled);

    let item = &mut theme.context_menu.item;
    shrink(&mut item.looks.normal);
    shrink(&mut item.looks.hovered);
    shrink(&mut item.looks.active);
    shrink(&mut item.looks.disabled);
    theme
}

palette_struct! {
    /// The graph canvas itself — the ground everything else sits on, and the
    /// dotted grid ruled across it.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct CanvasTheme;
    /// Ground fill behind the whole graph.
    bg: Color => CANVAS_BG,
    /// Backdrop grid dot colour.
    dot: Color => CANVAS_DOT,
    ;
    /// World-space base spacing between dots. Wrapped by a power-of-2
    /// multiplier as the user zooms so the field never collapses into noise —
    /// see `gui::pane::graph::background`.
    dot_spacing: f32 = 18.0,
    /// On-screen radius (px) of a backdrop dot.
    dot_radius: f32 = 0.6,
}

palette_struct! {
    /// An elevated rounded surface: node bodies, the inspector panel, the
    /// dock's tabs. Named for the shape rather than the node, because all
    /// three read from it — a header band derives its own tighter radius from
    /// [`Self::inner_radius`] rather than carrying fields of its own.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct CardTheme;
    /// Body fill.
    fill: Color => NODE_FILL,
    /// Resting outline. Transparent in the dark preset, where the ambient
    /// shadow carries the edge and the stroke slot is reserved for the
    /// selection / breaker / missing colours.
    border: Color => NODE_BORDER,
    /// Header band fill, a step brighter than `fill` so the band reads
    /// against the body. Doubles as the chrome lift behind a hovered strip
    /// glyph.
    header_fill: Color => HEADER_FILL,
    /// Ambient elevation shadow cast when no status glow claims the slot —
    /// one swatch so every elevated surface casts the same kind of shadow.
    ambient_shadow: Color => NODE_AMBIENT_SHADOW,
    ;
    /// Resting outline width. The drawn stroke is always
    /// [`Self::border_width_total`] — twice this — so selecting never resizes
    /// a card.
    border_width: f32 = 1.0,
    /// How round a card is. A header derives its own from
    /// [`Self::inner_radius`].
    corner_radius: f32 = 6.0,
    /// Minimum content size for a node body. Caps how tightly a node with
    /// very short port labels can shrink horizontally so the header stays
    /// legible at any zoom.
    min_width: f32 = 160.0,
    min_height: f32 = 10.0,
}

palette_struct! {
    /// A node's ports: the circles straddling the body edge, their labels,
    /// and the column geometry that lays them out.
    ///
    /// Positional swatches only — a *typed* port takes its hue from
    /// [`TypeColors`] instead, resolved by
    /// `gui::pane::graph::node::port_color`, which needs this roster, the
    /// type roster and the preset together and so stays a function over the
    /// whole [`Theme`].
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct PortTheme;
    /// Positional swatch for untyped input ports; hover lifts for emphasis.
    input: HoverColor => INPUT_PORT,
    /// Positional swatch for untyped output ports.
    output: HoverColor => OUTPUT_PORT,
    /// Event emitter glyphs, subscription pins, and event wires — neutral,
    /// distinct from the type-coloured data ports; hover lifts it like the
    /// positional colours.
    event: HoverColor => EVENT_PORT,
    /// Port + event label ink — de-emphasized against the full-strength
    /// value/editor text so each port row has one strong element. Its own
    /// slot (not `text_muted`) because the light palette needs a darker value
    /// for legibility on the card fill.
    label: Color => PORT_LABEL,
    ;
    /// Side of the port circle quad; the circle's radius is derived as half
    /// this (see [`Self::radius`]).
    size: f32 = 13.0,
    /// Horizontal inset on each side of the ports row. Circles overhang by
    /// `-Theme::port_overhang()` (which folds in this inset and the card
    /// border) so their centre sits on the body edge regardless of it.
    col_pad_x: f32 = 8.0,
    /// The column's vertical rhythm, spent twice over: the inset below the
    /// header band before the first port, and the gap between adjacent ports.
    /// One field because equal spacing is the point — a distinct top inset
    /// would read as a misaligned first row.
    gap: f32 = 6.0,
    /// Horizontal gap between the input and output columns.
    cols_gap: f32 = 12.0,
}

palette_struct! {
    /// The app's semantic feedback palette: what an outcome *means*, not
    /// which surface reports it.
    ///
    /// Node execution is the largest consumer — `gui::pane::graph::node`
    /// maps an `ExecStatus` onto these — but it is not the only one, which is
    /// why these are not named for it: `error` is also the invalid-path
    /// outline in preferences, the status-bar message and `LogLevel::Error`;
    /// `warning` is `LogLevel::Warn` and an unconnected required port;
    /// `success` is the toolbar's run glyph.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct StatusColors;
    /// It worked / it ran — palette `success` (green).
    success: Color => STATUS_SUCCESS,
    /// It was reused from cache — palette `accent` (cyan).
    info: Color => STATUS_INFO,
    /// It is happening right now — palette `constant` (purple).
    busy: Color => STATUS_BUSY,
    /// It is incomplete but not broken — palette `syn_keyword` (orange).
    warning: Color => STATUS_WARNING,
    /// It failed — palette `error` (red).
    error: Color => STATUS_ERROR,
}

palette_struct! {
    /// Chrome colours that belong to no single widget — the surround, the
    /// shared inks, and the badge roster. Serialized as the theme's
    /// `[colors]` table.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct PaletteColors;
    /// The selection accent: the rubber-band rectangle (translucent fill +
    /// near-opaque 1px border, both derived from this) *and* the selected-
    /// node border, so "in the selection" reads as one color from sweep to
    /// committed halo (palette accent).
    selection_rect: Color => SELECTION_RECT,
    connection_broken: Color => CONNECTION_BROKEN,
    breaker_stroke: Color => BREAKER_STROKE,
    /// Muted secondary foreground (palette `text_muted`, `#aaaaa8`). The
    /// de-emphasized accent shared across chrome: inactive/disabled header
    /// chips, the pinned-inspector outline, and active-tab text — visible
    /// without competing with the bright accent (`badge_graph`) or
    /// full-strength text.
    text_muted: Color => TEXT_MUTED,
    /// Top-chrome fill behind the menu bar + tab strip. A notch darker
    /// than the card surface, sitting between the graph (`canvas.bg`)
    /// and the nodes, so the chrome recedes and the active tab (which
    /// uses `canvas.bg`) reads as continuous with the graph below it.
    chrome_fill: Color => CHROME_FILL,
    /// Inactive tab-strip chip. A notch above `chrome_fill` toward the card
    /// surface, so an unselected tab reads as a resting chip rather than a
    /// bare label; the active tab uses `canvas.bg` + a `selection_rect`
    /// accent top-line instead.
    tab_inactive: Color => TAB_INACTIVE,
    /// Accent cyan: the inspect chip, the pinned-inspector outline, and the
    /// VRAM half of a memory readout.
    badge_graph: Color => BADGE_GRAPH,
    /// Sink chip — error red.
    badge_sink: Color => BADGE_SINK,
    /// RuntimeCache (persist-to-disk) chip — warning yellow.
    badge_cache: Color => BADGE_CACHE,
    /// Impure marker — `constant` purple. A read-only descriptor (the node
    /// recomputes every run and is never cached), not an interactive toggle.
    badge_impure: Color => BADGE_IMPURE,
}

impl PaletteColors {
    /// Rubber-band interior wash — `selection_rect` at 12%, pairing
    /// with [`Self::selection_border`] (the derivation the
    /// `selection_rect` doc promises lives in one place).
    pub(crate) fn selection_fill(&self) -> Color {
        self.selection_rect.with_alpha(0.12)
    }

    /// Rubber-band outline — `selection_rect` near-opaque.
    pub(crate) fn selection_border(&self) -> Color {
        self.selection_rect.with_alpha(0.85)
    }

    /// Soft hairline rule — `text_muted` at 18%, the peer of
    /// palantir's `Palette::border_soft`.
    pub(crate) fn border_soft(&self) -> Color {
        self.text_muted.with_alpha(0.18)
    }
}

/// Result of [`Theme::card_border`]: the resolved outline color. The width is
/// [`CardTheme::border_width_total`] — constant, so selecting never resizes a
/// card.
#[derive(Clone, Debug)]
pub(crate) struct CardBorder {
    pub(crate) color: Color,
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
            palantir_theme,
        }
    }
}

impl CardTheme {
    /// The stroke width a card actually draws — always the *selection* width
    /// (`border_width * 2`) regardless of selection state, so selecting one
    /// never resizes it (only its colour changes). Named so the doubling
    /// can't drift between the call sites that must agree on it: the stroke
    /// itself, [`Self::inner_radius`], and [`Theme::port_overhang_for`].
    #[inline]
    pub(crate) fn border_width_total(&self) -> f32 {
        self.border_width * 2.0
    }

    /// Inner corner radius for a header or footer strip seating flush against
    /// the card's own outer stroke — a node body rounds its header/footer band
    /// to this, not the raw `corner_radius`, else the strip's corner leaves a
    /// wedge of plain fill showing between it and the (selection-lit) stroke.
    #[inline]
    pub(crate) fn inner_radius(&self) -> f32 {
        (self.corner_radius - self.border_width_total()).max(0.0)
    }

    /// Ambient elevation shadow shared by every card — node bodies, inspector
    /// panels — so they all read as the same kind of surface. Only the blur
    /// scales with how high a surface sits; colour and offset are fixed.
    #[inline]
    pub(crate) fn elevation_shadow(&self, blur: f32) -> Shadow {
        Shadow::drop(self.ambient_shadow, glam::Vec2::new(0.0, 3.0), blur)
    }
}

impl PortTheme {
    /// Derived radius for port circles — half the port side. A method rather
    /// than a stored field so the two can't drift if someone bumps `size`.
    #[inline]
    pub(crate) fn radius(&self) -> f32 {
        self.size * 0.5
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

    /// Pin a few const-defined default values (Ayu Mirage High Contrast:
    /// canvas = terminal_bg, ports = success-green / syn-keyword-orange)
    /// plus the non-trivial palantir tweak, so an accidental const edit or
    /// a regression in `default_palantir_theme` fails loudly.
    #[test]
    fn default_palette_and_menu_tweak() {
        let theme = Theme::default();
        assert_eq!(theme.canvas.bg, Color::hex(0x1a1a1a));
        assert_eq!(theme.ports.input.rest, Color::hex(0xdaff58));
        assert_eq!(theme.ports.output.rest, Color::hex(0xffa63d));
        // RuntimeCache (persist-to-disk) chip is the palette `warning` yellow.
        assert_eq!(theme.colors.badge_cache, Color::hex(0xffd44a));
        // Impure marker is the palette `constant` purple.
        assert_eq!(theme.colors.badge_impure, Color::hex(0xc56cff));
        assert_eq!(theme.card.min_width, 160.0);
        assert!(theme.palantir_theme.tooltip.max_size.h.is_infinite());
        // The menu-bar font was shrunk from palantir's default to ours.
        let menu_text = theme
            .palantir_theme
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
        assert_eq!(dark.canvas.bg, Color::hex(0x1a1a1a));
        assert_eq!(light.canvas.bg, Color::hex(0xfcfcfc));
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
