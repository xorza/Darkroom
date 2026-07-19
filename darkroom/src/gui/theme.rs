use aperture::{
    Brush, ButtonTheme, Color, DragValueTheme, Shadow, Spacing, Stroke, TextEditTheme, WidgetLook,
};

use crate::core::theme_pref::ThemeChoice;

// Layout dimensions don't change between dark and light — they're factored
// out so both palettes pull the same numbers and a tweak hits both at once.

const CANVAS_DOT_SPACING: f32 = 18.0;
const CANVAS_DOT_RADIUS: f32 = 0.6;
const CONNECTION_WIDTH: f32 = 2.0;
const BREAKER_STROKE_WIDTH: f32 = 2.0;
const NODE_BORDER_WIDTH: f32 = 1.0;
const NODE_CORNER_RADIUS: f32 = 6.0;
const NODE_MIN_WIDTH: f32 = 160.0;
const NODE_MIN_HEIGHT: f32 = 10.0;
const TAB_CORNER_RADIUS: f32 = 6.0;
const PORT_SIZE: f32 = 13.0;
const PORT_COL_PAD_TOP: f32 = 6.0;
const PORT_COL_PAD_X: f32 = 8.0;
const PORT_GAP: f32 = 6.0;
const PORT_COLS_GAP: f32 = 12.0;
/// Gap between a node's edge (or a port circle) and a floating widget's near
/// edge — shared by the pin-preview card (from the port circle) and the
/// inspector panel (from the node's right edge), so every floating overlay
/// keeps the same clearance.
const FLOATING_WIDGET_GAP: f32 = 16.0;
const VALUE_EDITOR_WIDTH: f32 = 100.0;
/// Upper bound on the value column: editors fill the column up to here, then a
/// long value (a wide enum/preset dropdown, a long path) ellipsizes instead of
/// stretching the node out.
const VALUE_EDITOR_MAX_WIDTH: f32 = 240.0;
const NEW_NODE_POPUP_MAX_HEIGHT: f32 = 400.0;
const MENU_FONT_SIZE: f32 = 13.0;

// One named-const mod per built-in preset, so any builder (`Theme::dark`,
// `StaticValueEditorTheme::dark`, future per-widget theme helpers) can
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
    use aperture::Color;

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

    pub(crate) const EXEC_EXECUTED_GLOW: Color = Color::hex(0xdaff58);
    pub(crate) const EXEC_CACHED_GLOW: Color = Color::hex(0x9adbfb);
    pub(crate) const EXEC_RUNNING_GLOW: Color = Color::hex(0xd4bfff);
    pub(crate) const EXEC_MISSING_GLOW: Color = Color::hex(0xffa63d);
    pub(crate) const EXEC_ERRORED_GLOW: Color = Color::hex(0xff5e44);

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
    // sink `T` badge the subscription pin sits beside, so the trigger
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

    // aperture sub-theme palette — values aperture's widgets normally
    // read from its own `palette::*` consts. Pushed through
    // `AperturePalette` so the live `ui.theme` recolours alongside
    // darkroom chrome; reused by `StaticValueEditorTheme::dark` for
    // the per-palette path-pick chip.
    pub(crate) const PAL_TEXT: Color = Color::hex(0xe2dfd3);
    pub(crate) const PAL_TEXT_DISABLED: Color = Color::hex(0x878a8d);
    pub(crate) const PAL_ELEM_HOVER: Color = Color::hex(0x3e3e3e);
    pub(crate) const PAL_ELEM_ACTIVE: Color = Color::hex(0x4b4b4b);
    pub(crate) const PAL_BORDER_FOCUSED: Color = Color::hex(0x105577);
}

pub(crate) mod light {
    use super::{HoverColor, TypeColors};
    use aperture::Color;

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
    pub(crate) const EXEC_EXECUTED_GLOW: Color = Color::hex(0x85b304);
    pub(crate) const EXEC_CACHED_GLOW: Color = Color::hex(0x3b9ee5);
    pub(crate) const EXEC_RUNNING_GLOW: Color = Color::hex(0xa37acc);
    pub(crate) const EXEC_MISSING_GLOW: Color = Color::hex(0xfa8d3e);
    pub(crate) const EXEC_ERRORED_GLOW: Color = Color::hex(0xef7271);

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

    // aperture sub-theme palette — see `dark::PAL_*` for the contract.
    pub(crate) const PAL_TEXT: Color = Color::hex(0x5c6166);
    pub(crate) const PAL_TEXT_DISABLED: Color = Color::hex(0xa9acae);
    pub(crate) const PAL_ELEM_HOVER: Color = Color::hex(0xdfe0e1);
    pub(crate) const PAL_ELEM_ACTIVE: Color = Color::hex(0xcfd0d2);
    pub(crate) const PAL_BORDER_FOCUSED: Color = Color::hex(0xc4daf6);
}

/// Two-state colour pack for chrome that lifts under the pointer —
/// the colour-granularity peer of aperture's `StatefulLook`: the pair
/// is structural (a hover variant can't exist without its rest), and
/// state → colour goes through one `pick`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HoverColor {
    pub rest: Color,
    pub hover: Color,
}

impl HoverColor {
    #[inline]
    pub(crate) fn pick(&self, hovered: bool) -> Color {
        if hovered { self.hover } else { self.rest }
    }
}

/// Data-type → wire/port-circle hue roster (consumed by
/// `gui::node::port_color`). Serialized as the theme's `[type_colors]`
/// table so a loaded theme file can restyle type hues like any other
/// swatch. `ramp` backs the open-ended `Custom`/`Enum` families —
/// keyed by `type_id`, so distinct custom types land on stable,
/// distinct colors; `image` is the fixed hue the lens image type owns.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeColors {
    pub boolean: Color,
    pub int: Color,
    pub float: Color,
    pub string: Color,
    pub path: Color,
    pub image: Color,
    pub ramp: [Color; 4],
}

/// Declares a colour-roster struct plus its two built-in instances
/// (`DARK` / `LIGHT`, pulling `dark::CONST` / `light::CONST`) from one
/// `field: Ty => CONST` list. One roster per struct, so a colour can't
/// sit in the struct while a preset forgets it: the presets won't
/// compile until every field is filled. The serialized
/// [`PaletteColors`] chrome roster is built this way; the
/// aperture-side rosters are plain [`aperture::Palette`] consts
/// (`APERTURE_DARK` / `APERTURE_LIGHT`).
macro_rules! palette_struct {
    (
        $(#[$smeta:meta])*
        $vis:vis struct $name:ident;
        $($(#[$fmeta:meta])* $field:ident: $fty:ty => $konst:ident),+ $(,)?
    ) => {
        $(#[$smeta])*
        $vis struct $name {
            $($(#[$fmeta])* $vis $field: $fty,)+
        }

        impl $name {
            const DARK: Self = Self { $($field: dark::$konst),+ };
            const LIGHT: Self = Self { $($field: light::$konst),+ };
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

/// Visual palette + layout dimensions for darkroom's UI. Owned by
/// `MainWindow`, handed to every UI subtree through
/// [`crate::gui::app::AppContext`] so call sites read off a single source
/// instead of hard-coded constants. Layout fields live here too —
/// node ports, value editors, etc. — so a theme swap can restyle
/// geometry as well as color.
///
/// Also owns the aperture [`aperture::Theme`] this app wants on its
/// `Ui`. [`crate::gui::app::App::new`] copies `aperture_theme` into
/// `ui.theme` once before the first frame, so aperture-side widgets
/// (buttons, text edits, menus, scrollbars) read from the same source.
/// Tweak fields on `theme.aperture_theme` during construction to
/// override aperture's defaults.
///
/// Serializable so the whole bundle (aperture palette + darkroom
/// layout + colors) round-trips through serde for the Theme → Load /
/// Export menu.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Theme {
    // Scalar fields (`preset` + the layout `f32`s) come first; the tables
    // (`colors`, the per-widget sub-themes, `aperture_theme`) follow. TOML
    // serialization requires every scalar value to precede any table at the
    // same level — otherwise the serializer errors with `ValueAfterTable`.
    /// Which built-in preset assembled this theme. Round-trips
    /// through TOML so a user-loaded file restores the same toggle
    /// behaviour the original `Theme::dark` / `light` had.
    pub preset: ThemePreset,

    /// Dotted backdrop grid: world-space base spacing between dots, and
    /// on-screen dot radius (px). Spacing is wrapped by a power-of-2
    /// multiplier as the user zooms so the field never collapses into
    /// noise — see `gui::background`. (Dot colour is `colors.canvas_dot`.)
    pub canvas_dot_spacing: f32,
    pub canvas_dot_radius: f32,
    pub connection_width: f32,
    pub breaker_stroke_width: f32,
    pub node_border_width: f32,
    pub node_corner_radius: f32,
    /// Minimum content size for a node body. Caps how tightly a node
    /// with very short port labels can shrink horizontally so the
    /// header stays legible at any zoom.
    pub node_min_width: f32,
    pub node_min_height: f32,
    /// Corner radius of the tab-strip tabs (the header derives its own
    /// radius from `node_corner_radius`, so it doesn't read this).
    pub tab_corner_radius: f32,
    /// Side of the port circle quad. The circle's corner radius is
    /// derived as `port_size * 0.5` (see [`Self::port_radius`]).
    pub port_size: f32,
    /// Vertical inset at the top of each port column (gap below the
    /// header band before the first port).
    pub port_col_pad_top: f32,
    /// Horizontal inset on each side of the ports row. Port circles overhang
    /// by `-port_overhang()` (which folds in this inset + the body border) so
    /// their center sits on the node body edge regardless of this value.
    pub port_col_pad_x: f32,
    /// Vertical gap between adjacent ports in a column.
    pub port_gap: f32,
    /// Horizontal gap between the input and output port columns.
    pub port_cols_gap: f32,
    /// Gap between a node's edge (or a port circle) and a floating widget's
    /// near edge — the pin-preview card anchors from the port circle, the
    /// inspector panel from the node's right edge, so both read as the same
    /// clearance.
    pub floating_widget_gap: f32,
    /// Cap on the new-node popup's height. Inner scroll handles
    /// overflow when the function list exceeds the cap.
    pub new_node_popup_max_height: f32,

    /// Every chrome colour — the palette half of the theme, serialized as
    /// the `[colors]` sub-table.
    pub colors: PaletteColors,

    /// Data-type → wire/port hue roster (see [`TypeColors`]),
    /// serialized as the `[type_colors]` sub-table.
    pub type_colors: TypeColors,

    /// Look + dimensions for the inline static-value editor that hugs a
    /// `Binding::Const` input port (number/string field, file-pick chip).
    pub static_value_editor: StaticValueEditorTheme,

    /// The pointer-over-node variant of `static_value_editor` (chip fill
    /// pre-lit at half the hover strength). Precomputed at construction —
    /// deriving it per frame would clone the whole nested theme in the
    /// record path — and kept next to its base so the pair can't drift.
    pub static_value_editor_revealed: StaticValueEditorTheme,

    /// Look for the inline-rename widget (node title, boundary port,
    /// graph tab).
    pub inline_rename: InlineRenameTheme,

    /// Aperture-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every aperture widget (Button, TextEdit, MenuItem,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use. Last field so its TOML table
    /// follows all the scalar fields above (TOML `ValueAfterTable`).
    pub aperture_theme: aperture::Theme,
}

/// Per-widget theme bundle for the inline static-value editor on a
/// `Binding::Const` input port. Owns the `DragValue` look (scrub chip —
/// transparent at rest, hover-only background, no border — plus the inline
/// editor derived from it) which the numeric fields use directly and the
/// `Button`/`ComboBox` siblings (path pick, enum, presets) borrow via
/// `drag_value.chip`, and the fixed field width.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct StaticValueEditorTheme {
    pub drag_value: DragValueTheme,
    /// Minimum logical-px width of the value column — editors fill it down to
    /// at least this.
    pub width: f32,
    /// Maximum logical-px width of the value column, so a wide editor (enum /
    /// preset dropdown, long path) ellipsizes rather than stretching the node.
    pub max_width: f32,
}

impl StaticValueEditorTheme {
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
    fn revealed_from_palette(p: &aperture::Palette) -> Self {
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

    /// Shared shape: aperture's `menu_button` preset over `p` (transparent
    /// at rest + disabled, no border) as the chip, with the inline editor
    /// derived from that chip so both modes share one box, and
    /// caret/selection/placeholder from the same palette's text-edit
    /// recipe so it matches the app's other text fields.
    fn from_palette(p: &aperture::Palette) -> Self {
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
    pub text_edit: TextEditTheme,
}

impl InlineRenameTheme {
    /// Shared shape: start from the palette's text-edit recipe (which
    /// already carries the right caret / placeholder / selection), then
    /// strip every visual that would reshape the row (padding, margin,
    /// border, fill) so the field reads against whichever canvas hosts
    /// it.
    fn from_palette(p: &aperture::Palette) -> Self {
        let mut style = TextEditTheme {
            padding: Spacing::ZERO,
            margin: Spacing::ZERO,
            ..TextEditTheme::from_palette(p)
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
}

/// The [`aperture::Palette`] each preset hands to
/// [`aperture::Theme::from_palette`], filled from the preset's swatches
/// so swapping dark ⇄ light recolours every widget aperture paints, not
/// just darkroom-owned chrome. Notes on the mapping:
/// - `terminal_bg` wants the editor / terminal surface — the same
///   swatch as the graph canvas in both themes.
/// - `elem` and our `NODE_FILL` are the same swatch by design: nodes
///   and aperture surfaces sit on the same surface tier.
const APERTURE_DARK: aperture::Palette = aperture::Palette {
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

/// Light peer of [`APERTURE_DARK`] — same mapping over `light::*`.
const APERTURE_LIGHT: aperture::Palette = aperture::Palette {
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

/// Aperture sub-theme for darkroom: assemble every widget recipe from
/// the palette via [`aperture::Theme::from_palette`], then apply the
/// darkroom-only tweaks (smaller menu/context-menu font; menu-bar
/// triggers muted + transparent at rest so they read as menus, not
/// buttons).
fn aperture_theme_for(p: &aperture::Palette, chrome_fill: Color) -> aperture::Theme {
    let mut theme = aperture::Theme::from_palette(p);

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
    // `recolour_aperture` set.
    let base = &theme.text;
    // Font-only shrink (keeps each look's own colour) for the context-menu
    // rows; menu-bar triggers also recolour per state, so they use `restyle`.
    let shrink = |look: &mut WidgetLook| {
        let text = look.text.take().unwrap_or_else(|| base.clone());
        look.text = Some(text.with_font_size(MENU_FONT_SIZE));
    };
    let restyle = |look: &mut WidgetLook, color: Color| {
        let text = look.text.take().unwrap_or_else(|| base.clone());
        look.text = Some(text.with_color(color).with_font_size(MENU_FONT_SIZE));
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
    /// Every darkroom chrome colour — the palette half of a [`Theme`]
    /// (the other half is layout dimensions). Serialized as the theme's
    /// `[colors]` table. Deliberately not `Copy` (25 colours): moved,
    /// not silently bit-copied.
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub(crate) struct PaletteColors;
    canvas_bg: Color => CANVAS_BG,
    /// The selection accent: the rubber-band rectangle (translucent fill +
    /// near-opaque 1px border, both derived from this) *and* the selected-
    /// node border, so "in the selection" reads as one color from sweep to
    /// committed halo (palette accent).
    selection_rect: Color => SELECTION_RECT,
    /// Dotted backdrop grid dot color. Spacing + radius are layout
    /// dimensions on `Theme` (`canvas_dot_spacing` / `canvas_dot_radius`).
    canvas_dot: Color => CANVAS_DOT,
    connection_broken: Color => CONNECTION_BROKEN,
    breaker_stroke: Color => BREAKER_STROKE,
    node_fill: Color => NODE_FILL,
    node_border: Color => NODE_BORDER,
    header_fill: Color => HEADER_FILL,
    /// Muted secondary foreground (palette `text_muted`, `#aaaaa8`). The
    /// de-emphasized accent shared across chrome: inactive/disabled header
    /// chips, the pinned-inspector outline, and active-tab text — visible
    /// without competing with the bright accent (`badge_graph`) or
    /// full-strength text.
    text_muted: Color => TEXT_MUTED,
    /// Port + event label ink — de-emphasized against the full-strength
    /// value/editor text so each port row has one strong element. Its own
    /// slot (not `text_muted`) because the light palette needs a darker
    /// value for legibility on the node fill.
    port_label: Color => PORT_LABEL,
    /// Ambient elevation shadow cast by nodes and floating panels (the
    /// inspector) when no status glow claims the slot — one swatch so
    /// every elevated surface casts the same kind of shadow.
    node_ambient_shadow: Color => NODE_AMBIENT_SHADOW,
    /// Top-chrome fill behind the menu bar + tab strip. A notch darker
    /// than the node surface, sitting between the graph (`canvas_bg`)
    /// and the nodes, so the chrome recedes and the active tab (which
    /// uses `canvas_bg`) reads as continuous with the graph below it.
    chrome_fill: Color => CHROME_FILL,
    /// Inactive tab-strip chip. A notch above `chrome_fill` toward the node
    /// surface, so an unselected tab reads as a resting chip rather than a
    /// bare label; the active tab uses `canvas_bg` + a `selection_rect`
    /// accent top-line instead.
    tab_inactive: Color => TAB_INACTIVE,
    /// Graph (composite instance) chip — accent cyan.
    badge_graph: Color => BADGE_GRAPH,
    /// Sink chip — error red.
    badge_sink: Color => BADGE_SINK,
    /// RuntimeCache (persist-to-disk) chip — warning yellow.
    badge_cache: Color => BADGE_CACHE,
    /// Impure marker — `constant` purple. A read-only descriptor (the node
    /// recomputes every run and is never cached), not an interactive toggle.
    badge_impure: Color => BADGE_IMPURE,
    /// Soft glow behind a node computed this run — palette `success` (green).
    exec_executed_glow: Color => EXEC_EXECUTED_GLOW,
    /// Node reused its cached result — palette `accent` (cyan).
    exec_cached_glow: Color => EXEC_CACHED_GLOW,
    /// Node is computing this run (live) — palette `constant` (purple).
    exec_running_glow: Color => EXEC_RUNNING_GLOW,
    /// Node has unfilled required inputs — palette `syn_keyword` (orange).
    exec_missing_glow: Color => EXEC_MISSING_GLOW,
    /// Node errored — palette `error` (red).
    exec_errored_glow: Color => EXEC_ERRORED_GLOW,
    /// Positional swatch for untyped input ports (typed ports use
    /// [`TypeColors`]); hover lifts for emphasis.
    input_port: HoverColor => INPUT_PORT,
    /// Positional swatch for untyped output ports.
    output_port: HoverColor => OUTPUT_PORT,
    /// Event emitter glyphs, subscription pins, and event wires (neutral,
    /// distinct from the type-colored data ports); hover lifts it like
    /// the positional port colors.
    event_port: HoverColor => EVENT_PORT,
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
    /// aperture's `Palette::border_soft`.
    pub(crate) fn border_soft(&self) -> Color {
        self.text_muted.with_alpha(0.18)
    }
}

/// Result of [`Theme::card_border`]: the resolved outline color plus the
/// width every selectable card draws it at.
#[derive(Clone, Debug)]
pub(crate) struct CardBorder {
    pub color: Color,
    pub width: f32,
}

impl Theme {
    /// Derived radius for port circles — half the port side. Lives as
    /// a method instead of a stored field so the two can't drift if
    /// someone bumps `port_size` and forgets the radius.
    #[inline]
    pub(crate) fn port_radius(&self) -> f32 {
        self.port_size * 0.5
    }

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
        radius + self.port_col_pad_x + self.card_border_width()
    }

    /// [`Self::port_overhang_for`] at the plain port radius. Independent of
    /// `port_size` — bigger circles keep their center on the edge.
    #[inline]
    pub(crate) fn port_overhang(&self) -> f32 {
        self.port_overhang_for(self.port_radius())
    }

    /// The stroke width every selectable card draws — node bodies and
    /// pin-preview widgets alike — always the *selection* width
    /// (`node_border_width * 2`) regardless of selection state, so
    /// selecting one never resizes it (only its color changes). Named so
    /// the doubling can't drift between the call sites that need to agree
    /// on it: the stroke itself, [`Self::card_inner_radius`], and
    /// [`Self::port_overhang_for`].
    #[inline]
    pub(crate) fn card_border_width(&self) -> f32 {
        self.node_border_width * 2.0
    }

    /// Border color + width for a selectable card's 3-tier resting decision
    /// — node bodies and pin-preview widgets both resolve their outline this
    /// way: a breaker hit wins as the alarm color, else the selection halo
    /// when selected, else the neutral resting `node_border`. Width is
    /// always [`Self::card_border_width`] regardless of tier, so selecting
    /// (or breaking) a card never resizes it — only the color changes. A
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
            self.colors.node_border
        };
        CardBorder {
            color,
            width: self.card_border_width(),
        }
    }

    /// Inner corner radius for a header or footer strip that seats flush
    /// against a card's own outer stroke — node bodies and pin-preview
    /// widgets both round their header/footer band to this, not the raw
    /// `node_corner_radius`, else the strip's corner leaves a wedge of the
    /// card's plain fill showing between it and the (selection-lit) stroke.
    #[inline]
    pub(crate) fn card_inner_radius(&self) -> f32 {
        (self.node_corner_radius - self.card_border_width()).max(0.0)
    }

    /// Ambient elevation shadow shared by every floating card — node
    /// bodies, pin previews, inspector panels — so they all read as the
    /// same kind of surface. Only the blur scales with how high a surface
    /// sits; color and offset are fixed.
    #[inline]
    pub(crate) fn elevation_shadow(&self, blur: f32) -> Shadow {
        Shadow::drop(
            self.colors.node_ambient_shadow,
            glam::Vec2::new(0.0, 3.0),
            blur,
        )
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
        Self::build(
            ThemePreset::Dark,
            PaletteColors::DARK,
            dark::TYPE_COLORS,
            &APERTURE_DARK,
        )
    }

    /// Ayu Light palette — the built-in light look (Zed's "Ayu Light"
    /// variant ported into darkroom's structure).
    pub(crate) fn light() -> Self {
        Self::build(
            ThemePreset::Light,
            PaletteColors::LIGHT,
            light::TYPE_COLORS,
            &APERTURE_LIGHT,
        )
    }

    /// Shared assembly path — the darkroom peer of
    /// `aperture::Theme::from_palette`: dimensions are
    /// palette-independent; `colors` / `type_colors` (moved in, not
    /// copied) drive darkroom chrome, and every sub-recipe (the
    /// aperture widget theme, the static-value editor, inline rename)
    /// cascades from `p` here rather than being hand-assembled per
    /// preset. `preset` tags which built-in produced this theme so the
    /// toggle command doesn't have to guess.
    fn build(
        preset: ThemePreset,
        colors: PaletteColors,
        type_colors: TypeColors,
        p: &aperture::Palette,
    ) -> Self {
        let chrome_fill = colors.chrome_fill;
        Self {
            preset,
            canvas_dot_spacing: CANVAS_DOT_SPACING,
            canvas_dot_radius: CANVAS_DOT_RADIUS,
            connection_width: CONNECTION_WIDTH,
            breaker_stroke_width: BREAKER_STROKE_WIDTH,
            node_border_width: NODE_BORDER_WIDTH,
            node_corner_radius: NODE_CORNER_RADIUS,
            node_min_width: NODE_MIN_WIDTH,
            node_min_height: NODE_MIN_HEIGHT,
            tab_corner_radius: TAB_CORNER_RADIUS,
            port_size: PORT_SIZE,
            port_col_pad_top: PORT_COL_PAD_TOP,
            port_col_pad_x: PORT_COL_PAD_X,
            port_gap: PORT_GAP,
            port_cols_gap: PORT_COLS_GAP,
            floating_widget_gap: FLOATING_WIDGET_GAP,
            new_node_popup_max_height: NEW_NODE_POPUP_MAX_HEIGHT,
            colors,
            type_colors,
            static_value_editor: StaticValueEditorTheme::from_palette(p),
            static_value_editor_revealed: StaticValueEditorTheme::revealed_from_palette(p),
            inline_rename: InlineRenameTheme::from_palette(p),
            aperture_theme: aperture_theme_for(p, chrome_fill),
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
    use static_assertions::assert_not_impl_any;

    assert_not_impl_any!(Theme: Copy);
    assert_not_impl_any!(PaletteColors: Copy);
    assert_not_impl_any!(TypeColors: Copy);
    assert_not_impl_any!(HoverColor: Copy);
    assert_not_impl_any!(CardBorder: Copy);
    assert_not_impl_any!(StaticValueEditorTheme: Copy);
    assert_not_impl_any!(InlineRenameTheme: Copy);

    /// Keep the checked-in `assets/ayu-graphite.toml` in sync with the
    /// const-defined [`Theme::default`]: serialize the default and rewrite
    /// the file. The asset is a generated artifact (a reference theme
    /// users can copy / the Theme → Load-Export format), not a source of
    /// truth — running the suite regenerates it, so any change to the
    /// consts (or aperture's defaults) surfaces as an asset diff to commit.
    /// Writing is idempotent when already in sync, so it's a no-op on a
    /// clean tree.
    #[test]
    fn ayu_graphite_asset_in_sync() {
        let bytes =
            common::serialize(&Theme::default(), SerdeFormat::Toml).expect("serialize theme");
        std::fs::write("assets/ayu-graphite.toml", bytes).expect("write toml asset");
    }

    /// The whole bundle — darkroom's own fields *and* the nested
    /// aperture palette — must survive a TOML round-trip; that's the
    /// on-disk format the Theme → Load / Export menu and the preferences
    /// rely on. Exercises the formerly-fragile case too: the tooltip's
    /// infinite max-size axis (handled by `Size`'s custom serde).
    #[test]
    fn theme_roundtrips_through_toml() {
        let mut theme = Theme {
            node_min_width: 137.5,
            ..Theme::default()
        };
        theme.colors.text_muted = Color::hex(0x123456);
        theme.aperture_theme.window_clear = Color::hex(0xabcdef);

        let bytes = common::serialize(&theme, SerdeFormat::Toml).expect("serialize theme");
        let back: Theme = common::deserialize(&bytes, SerdeFormat::Toml)
            .expect("theme should deserialize from its own TOML output");

        assert_eq!(back.node_min_width, 137.5);
        assert_eq!(back.colors.text_muted, Color::hex(0x123456));
        assert_eq!(back.colors.canvas_bg, theme.colors.canvas_bg);
        // Nested aperture palette round-trips too.
        assert_eq!(back.aperture_theme.window_clear, Color::hex(0xabcdef));
        // The infinite tooltip-height axis survives `Size`'s serde.
        assert!(back.aperture_theme.tooltip.max_size.h.is_infinite());
        assert_eq!(back.aperture_theme.tooltip.max_size.w, 280.0);
    }

    /// Pin a few const-defined default values (Ayu Mirage High Contrast:
    /// canvas = terminal_bg, ports = success-green / syn-keyword-orange)
    /// plus the non-trivial aperture tweak, so an accidental const edit or
    /// a regression in `default_aperture_theme` fails loudly.
    #[test]
    fn default_palette_and_menu_tweak() {
        let theme = Theme::default();
        assert_eq!(theme.colors.canvas_bg, Color::hex(0x1a1a1a));
        assert_eq!(theme.colors.input_port.rest, Color::hex(0xdaff58));
        assert_eq!(theme.colors.output_port.rest, Color::hex(0xffa63d));
        // RuntimeCache (persist-to-disk) chip is the palette `warning` yellow.
        assert_eq!(theme.colors.badge_cache, Color::hex(0xffd44a));
        // Impure marker is the palette `constant` purple.
        assert_eq!(theme.colors.badge_impure, Color::hex(0xc56cff));
        assert_eq!(theme.node_min_width, 160.0);
        assert!(theme.aperture_theme.tooltip.max_size.h.is_infinite());
        // The menu-bar font was shrunk from aperture's default to ours.
        let menu_text = theme
            .aperture_theme
            .menu_button
            .looks
            .normal
            .text
            .expect("menu button carries an explicit text style");
        assert_eq!(menu_text.font_size_px, MENU_FONT_SIZE);
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
        assert_eq!(dark.colors.canvas_bg, Color::hex(0x1a1a1a));
        assert_eq!(light.colors.canvas_bg, Color::hex(0xfcfcfc));
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
