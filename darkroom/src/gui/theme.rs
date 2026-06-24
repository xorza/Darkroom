use palantir::{
    Background, Brush, ButtonTheme, Color, Corners, Shadow, Spacing, Stroke, TextEditTheme,
    WidgetLook,
};

// ── shared dimensions ────────────────────────────────────────────────
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
const VALUE_EDITOR_WIDTH: f32 = 100.0;
/// Upper bound on the value column: editors fill the column up to here, then
/// a long value (e.g. a long file path) ellipsizes instead of stretching the
/// node out.
const VALUE_EDITOR_MAX_WIDTH: f32 = 240.0;
const NEW_NODE_POPUP_MAX_HEIGHT: f32 = 400.0;
// palantir sub-theme tweaks (see `palantir_theme_for`)
const MENU_FONT_SIZE: f32 = 13.0;
const MENU_CHIP_ALPHA: f32 = 0.85;

// ── colour palettes ──────────────────────────────────────────────────
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
//
// TODO: see `theme-review.md` §2/§4 — most of `recolour_palantir` should
// move into palantir as a `Theme::from_palette` ctor; once that lands,
// the `PAL_*` block in each mod (and `PalantirPalette`) collapses to
// "pick `palantir::Palette::dark()` or `::light()`".

pub(crate) mod dark {
    use palantir::Color;

    // canvas
    pub(crate) const CANVAS_BG: Color = Color::hex(0x1a1a1a);
    pub(crate) const SELECTION_RECT: Color = Color::hex(0x9adbfb);
    pub(crate) const CANVAS_DOT: Color = Color::hex(0x363636);

    // connections + breaker
    pub(crate) const CONNECTION_BROKEN: Color = Color::hex(0xff5e44);
    pub(crate) const BREAKER_STROKE: Color = Color::hex(0xff5e44);

    // node chrome
    pub(crate) const NODE_FILL: Color = Color::hex(0x343434);
    pub(crate) const NODE_BORDER: Color = Color::hex(0x363636);
    pub(crate) const HEADER_FILL: Color = Color::hex(0x414141);
    pub(crate) const TEXT_MUTED: Color = Color::hex(0xaaaaa8);
    pub(crate) const CHROME_FILL: Color = Color::hex(0x252525);

    // header badges
    pub(crate) const BADGE_SUBGRAPH: Color = Color::hex(0x9adbfb);
    pub(crate) const BADGE_TERMINAL: Color = Color::hex(0xff5e44);
    // cache (persist-to-disk) chip — palette `warning` yellow.
    pub(crate) const BADGE_CACHE: Color = Color::hex(0xffd44a);

    // execution-status glow
    pub(crate) const EXEC_EXECUTED_GLOW: Color = Color::hex(0xdaff58);
    pub(crate) const EXEC_CACHED_GLOW: Color = Color::hex(0x9adbfb);
    pub(crate) const EXEC_RUNNING_GLOW: Color = Color::hex(0xd4bfff);
    pub(crate) const EXEC_MISSING_GLOW: Color = Color::hex(0xffa63d);
    pub(crate) const EXEC_ERRORED_GLOW: Color = Color::hex(0xff5e44);

    // ports — hover variants brighten for emphasis on a dark canvas.
    pub(crate) const INPUT_PORT: Color = Color::hex(0xdaff58);
    pub(crate) const OUTPUT_PORT: Color = Color::hex(0xffa63d);
    pub(crate) const INPUT_PORT_HOVER: Color = Color::hex(0xe9ff8e);
    pub(crate) const OUTPUT_PORT_HOVER: Color = Color::hex(0xffc878);

    // palantir sub-theme palette — values palantir's widgets normally
    // read from its own `palette::*` consts. Pushed through
    // `PalantirPalette` so the live `ui.theme` recolours alongside
    // darkroom chrome; reused by `StaticValueEditorTheme::dark` for
    // the per-palette path-pick chip.
    pub(crate) const PAL_TEXT: Color = Color::hex(0xe2dfd3);
    pub(crate) const PAL_TEXT_DISABLED: Color = Color::hex(0x878a8d);
    pub(crate) const PAL_ELEM_HOVER: Color = Color::hex(0x3e3e3e);
    pub(crate) const PAL_ELEM_ACTIVE: Color = Color::hex(0x4b4b4b);
    pub(crate) const PAL_BORDER_FOCUSED: Color = Color::hex(0x105577);
}

pub(crate) mod light {
    use palantir::Color;

    // canvas
    pub(crate) const CANVAS_BG: Color = Color::hex(0xfcfcfc);
    pub(crate) const SELECTION_RECT: Color = Color::hex(0x3b9ee5);
    pub(crate) const CANVAS_DOT: Color = Color::hex(0xcfd1d2);

    // connections + breaker
    pub(crate) const CONNECTION_BROKEN: Color = Color::hex(0xef7271);
    pub(crate) const BREAKER_STROKE: Color = Color::hex(0xef7271);

    // node chrome
    pub(crate) const NODE_FILL: Color = Color::hex(0xececed);
    pub(crate) const NODE_BORDER: Color = Color::hex(0xcfd1d2);
    pub(crate) const HEADER_FILL: Color = Color::hex(0xdcddde);
    pub(crate) const TEXT_MUTED: Color = Color::hex(0x8b8e92);
    pub(crate) const CHROME_FILL: Color = Color::hex(0xdcddde);

    // header badges — accent / error / a deeper amber than the palette's
    // warning yellow (#f1ad49 was barely visible on a light surface).
    pub(crate) const BADGE_SUBGRAPH: Color = Color::hex(0x3b9ee5);
    pub(crate) const BADGE_TERMINAL: Color = Color::hex(0xef7271);
    // cache (persist-to-disk) chip — palette `warning` yellow.
    pub(crate) const BADGE_CACHE: Color = Color::hex(0xf1ad49);

    // execution-status glow — success / accent / syn_keyword / error.
    pub(crate) const EXEC_EXECUTED_GLOW: Color = Color::hex(0x85b304);
    pub(crate) const EXEC_CACHED_GLOW: Color = Color::hex(0x3b9ee5);
    pub(crate) const EXEC_RUNNING_GLOW: Color = Color::hex(0xa37acc);
    pub(crate) const EXEC_MISSING_GLOW: Color = Color::hex(0xfa8d3e);
    pub(crate) const EXEC_ERRORED_GLOW: Color = Color::hex(0xef7271);

    // ports — input = success, output = syn_keyword. Hover variants on
    // the light canvas *darken* for emphasis (opposite to the dark theme).
    pub(crate) const INPUT_PORT: Color = Color::hex(0x85b304);
    pub(crate) const OUTPUT_PORT: Color = Color::hex(0xfa8d3e);
    pub(crate) const INPUT_PORT_HOVER: Color = Color::hex(0x6f9603);
    pub(crate) const OUTPUT_PORT_HOVER: Color = Color::hex(0xd97527);

    // palantir sub-theme palette — see `dark::PAL_*` for the contract.
    pub(crate) const PAL_TEXT: Color = Color::hex(0x5c6166);
    pub(crate) const PAL_TEXT_DISABLED: Color = Color::hex(0xa9acae);
    pub(crate) const PAL_ELEM_HOVER: Color = Color::hex(0xdfe0e1);
    pub(crate) const PAL_ELEM_ACTIVE: Color = Color::hex(0xcfd0d2);
    pub(crate) const PAL_BORDER_FOCUSED: Color = Color::hex(0xc4daf6);
}

/// Which built-in palette built this [`Theme`] — the concrete palette
/// a [`ThemeChoice`] resolves to. Carried on the theme itself and
/// round-tripped through TOML so a loaded theme file restores its
/// origin palette. `Default = Dark` so a hand-rolled `Theme` (e.g. the
/// deserialised round-trip used by tests) has a deterministic tag
/// without callers having to spell it out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreset {
    #[default]
    Dark,
    Light,
}

impl ThemePreset {
    /// The OS's current light/dark preference, falling back to
    /// [`Dark`](Self::Dark) when the platform reports no preference or
    /// detection fails. Backs [`ThemeChoice::System`].
    pub fn from_system() -> Self {
        match dark_light::detect() {
            Ok(dark_light::Mode::Light) => Self::Light,
            Ok(dark_light::Mode::Dark | dark_light::Mode::Unspecified) | Err(_) => Self::Dark,
        }
    }
}

use crate::core::theme_pref::ThemeChoice;

impl ThemeChoice {
    /// Resolve to the concrete built-in preset to load. `System` queries
    /// the OS (falling back to dark); `Dark` / `Light` map straight
    /// through.
    pub fn resolve(self) -> ThemePreset {
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
/// Also owns the palantir [`palantir::Theme`] this app wants on its
/// `Ui`. [`crate::gui::app::App::new`] copies `palantir_theme` into
/// `ui.theme` once before the first frame, so palantir-side widgets
/// (buttons, text edits, menus, scrollbars) read from the same source.
/// Tweak fields on `theme.palantir_theme` during construction to
/// override palantir's defaults.
///
/// Serializable so the whole bundle (palantir palette + darkroom
/// layout + colors) round-trips through Rhai for the Theme → Load /
/// Export menu.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Theme {
    // Scalar (`Color` / `f32`) fields come first; `palantir_theme`
    // (a nested table) is last. TOML serialization requires every
    // scalar value to precede any table at the same level — otherwise
    // the serializer errors with `ValueAfterTable`.
    /// Which built-in preset assembled this theme. Round-trips
    /// through TOML so a user-loaded file restores the same toggle
    /// behaviour the original `Theme::dark` / `light` had.
    pub preset: ThemePreset,

    // ── canvas ────────────────────────────────────────────────────
    pub canvas_bg: Color,
    /// Tint of the rubber-band multi-selection rectangle. Drawn as a
    /// translucent fill plus a near-opaque 1px border, both derived
    /// from this single color (palette accent).
    pub selection_rect: Color,
    /// Dotted backdrop grid: dot color, world-space base spacing
    /// between dots, and on-screen dot radius (px). Spacing is wrapped
    /// by a power-of-2 multiplier as the user zooms so the field never
    /// collapses into noise — see `gui::background`.
    pub canvas_dot: Color,
    pub canvas_dot_spacing: f32,
    pub canvas_dot_radius: f32,

    // ── connections ──────────────────────────────────────────────
    pub connection_broken: Color,
    pub connection_width: f32,
    pub breaker_stroke: Color,
    pub breaker_stroke_width: f32,

    // ── node chrome ──────────────────────────────────────────────
    pub node_fill: Color,
    pub node_border: Color,
    pub node_border_width: f32,
    pub node_corner_radius: f32,
    /// Minimum content size for a node body. Caps how tightly a node
    /// with very short port labels can shrink horizontally so the
    /// header stays legible at any zoom.
    pub node_min_width: f32,
    pub node_min_height: f32,

    pub header_fill: Color,
    /// Corner radius of the tab-strip tabs (the header derives its own
    /// radius from `node_corner_radius`, so it doesn't read this).
    pub tab_corner_radius: f32,

    /// Muted secondary foreground (palette `text_muted`, `#aaaaa8`). The
    /// de-emphasized accent shared across chrome: the selected-node halo,
    /// inactive/disabled header chips, the pinned-inspector outline, and
    /// active-tab text — visible without competing with the bright accent
    /// (`badge_subgraph`) or full-strength text.
    pub text_muted: Color,

    /// Top-chrome fill behind the menu bar + tab strip. A notch darker
    /// than the node surface, sitting between the graph (`canvas_bg`)
    /// and the nodes, so the chrome recedes and the active tab (which
    /// uses `canvas_bg`) reads as continuous with the graph below it.
    pub chrome_fill: Color,

    // ── header badges ────────────────────────────────────────────
    /// Subgraph (composite instance) chip — accent cyan.
    pub badge_subgraph: Color,
    /// Terminal (sink) chip — error red.
    pub badge_terminal: Color,
    /// Cache (persist-to-disk) chip — warning yellow.
    pub badge_cache: Color,

    // ── execution-status glow ────────────────────────────────────
    // Color of the soft glow shadow behind a node, by the last run's
    // outcome.
    // Palette swatches: `success`/`accent`/`syn_keyword`/`error`.
    /// Node computed this run — palette `success` (green).
    pub exec_executed_glow: Color,
    /// Node reused its cached result — palette `accent` (cyan).
    pub exec_cached_glow: Color,
    /// Node is computing this run (live) — palette `constant` (purple).
    pub exec_running_glow: Color,
    /// Node has unfilled required inputs — palette `syn_keyword` (orange).
    pub exec_missing_glow: Color,
    /// Node errored — palette `error` (red).
    pub exec_errored_glow: Color,

    // ── ports ────────────────────────────────────────────────────
    pub input_port: Color,
    pub output_port: Color,
    pub input_port_hover: Color,
    pub output_port_hover: Color,
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

    // ── inline editors / popups ─────────────────────────────────
    /// Cap on the new-node popup's height. Inner scroll handles
    /// overflow when the function list exceeds the cap.
    pub new_node_popup_max_height: f32,

    /// Look + dimensions for the inline static-value editor that hugs a
    /// `Binding::Const` input port (number/string field, file-pick chip).
    pub static_value_editor: StaticValueEditorTheme,

    /// Look for the inline-rename widget (node title, boundary port,
    /// subgraph tab).
    pub inline_rename: InlineRenameTheme,

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, TextEdit, MenuItem,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use. Last field so its TOML table
    /// follows all the scalar fields above (TOML `ValueAfterTable`).
    pub palantir_theme: palantir::Theme,
}

/// Per-widget theme bundle for the inline static-value editor on a
/// `Binding::Const` input port. Owns the look of the path-pick chip
/// (transparent at rest, hover-only background, no border) and the
/// fixed field width so a single struct covers every visual axis the
/// editor needs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StaticValueEditorTheme {
    pub button: ButtonTheme,
    /// Minimum logical-px width of the embedded `TextEdit` / pick button /
    /// dropdown — editors fill the value column down to at least this.
    pub width: f32,
    /// Maximum logical-px width of the value column, so a long value
    /// ellipsizes rather than stretching the node.
    pub max_width: f32,
}

impl StaticValueEditorTheme {
    /// Path-pick chip in the dark theme — transparent at rest, dark
    /// `elem_hover`/`elem_active` fills on hover and press.
    fn dark() -> Self {
        Self::with_button_fills(dark::PAL_ELEM_HOVER, dark::PAL_ELEM_ACTIVE)
    }

    /// Path-pick chip in the light theme — same structure as `dark`,
    /// with the light palette's `elem_hover`/`elem_active` fills.
    fn light() -> Self {
        Self::with_button_fills(light::PAL_ELEM_HOVER, light::PAL_ELEM_ACTIVE)
    }

    /// Shared shape: palantir's `menu_button` preset (transparent at
    /// rest + disabled, no border) with the two hover/press fills
    /// substituted to match the live palette. `ButtonTheme` isn't
    /// `const`-constructible, so this lives behind a `fn` instead of
    /// `const DARK / LIGHT` peers next to the palette mods.
    fn with_button_fills(hover: Color, pressed: Color) -> Self {
        let mut button = ButtonTheme::menu_button();
        if let Some(bg) = button.hovered.background.as_mut() {
            bg.fill = hover.into();
        }
        if let Some(bg) = button.pressed.background.as_mut() {
            bg.fill = pressed.into();
        }
        Self {
            button,
            width: VALUE_EDITOR_WIDTH,
            max_width: VALUE_EDITOR_MAX_WIDTH,
        }
    }
}

/// Per-widget theme bundle for the inline-rename label⇄field widget
/// (node title, boundary-port name, subgraph tab). The `text_edit`
/// look is stripped to the bare editor surface (no padding/margin, no
/// border, transparent fill) so the field's `Hug` height equals its
/// plain `Text` twin and the row doesn't reshape on a swap.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InlineRenameTheme {
    pub text_edit: TextEditTheme,
}

impl InlineRenameTheme {
    /// Inline-rename editor in the dark theme — flat surface (no
    /// padding/margin/border, transparent fill) plus the dark palette's
    /// foreground/accent for the caret + selection.
    pub(crate) fn dark() -> Self {
        Self::with_palette_colors(dark::PAL_TEXT, dark::TEXT_MUTED, dark::SELECTION_RECT)
    }

    /// Inline-rename editor in the light theme — same flat surface,
    /// light-palette caret + selection so it stays visible on the
    /// light canvas.
    fn light() -> Self {
        Self::with_palette_colors(light::PAL_TEXT, light::TEXT_MUTED, light::SELECTION_RECT)
    }

    /// Shared shape: start from palantir's `TextEditTheme::default`,
    /// strip every visual that would reshape the row (padding, margin,
    /// border, fill), then recolour the live-state foreground
    /// (`caret`, `placeholder`, `selection`) from the supplied palette
    /// so the field reads against whichever canvas hosts it.
    fn with_palette_colors(text: Color, muted: Color, accent: Color) -> Self {
        let mut style = TextEditTheme {
            padding: Spacing::ZERO,
            margin: Spacing::ZERO,
            ..TextEditTheme::default()
        };
        for look in [&mut style.normal, &mut style.focused, &mut style.disabled] {
            if let Some(bg) = look.background.as_mut() {
                bg.stroke = Stroke::ZERO;
                bg.fill = Brush::TRANSPARENT;
            }
        }
        style.caret = text;
        style.placeholder = muted;
        style.selection = accent.with_alpha(0.25);
        Self { text_edit: style }
    }
}

/// Palette palantir's widgets need to render correctly under a darkroom
/// theme. Mirrors palantir's own (private) `palette::*` consts; we hand
/// it in so swapping dark ⇄ light recolours every widget palantir paints,
/// not just darkroom-owned chrome. Built from the matching palette mod's
/// `PAL_*` block.
struct PalantirPalette {
    text: Color,
    text_muted: Color,
    text_disabled: Color,
    terminal_bg: Color,
    elem: Color,
    elem_hover: Color,
    elem_active: Color,
    border_focused: Color,
    accent: Color,
}

impl PalantirPalette {
    const DARK: Self = Self {
        text: dark::PAL_TEXT,
        text_muted: dark::TEXT_MUTED,
        text_disabled: dark::PAL_TEXT_DISABLED,
        // Palantir's `window_clear` slot wants the editor / terminal
        // surface — same swatch as the graph canvas in both themes.
        terminal_bg: dark::CANVAS_BG,
        // Palantir's `palette::ELEM` and our `NODE_FILL` are the same
        // swatch by design: nodes and palantir surfaces sit on the
        // same surface tier.
        elem: dark::NODE_FILL,
        elem_hover: dark::PAL_ELEM_HOVER,
        elem_active: dark::PAL_ELEM_ACTIVE,
        border_focused: dark::PAL_BORDER_FOCUSED,
        accent: dark::SELECTION_RECT,
    };

    const LIGHT: Self = Self {
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
}

/// Palantir sub-theme for darkroom: start from palantir's defaults,
/// recolour every widget using `p`, then apply darkroom-only tweaks
/// (smaller menu/context-menu font, semi-transparent node-surface chip
/// on the floating menu-bar triggers).
fn palantir_theme_for(p: &PalantirPalette) -> palantir::Theme {
    let mut theme = palantir::Theme::default();
    recolour_palantir(&mut theme, p);

    let base = theme.text;
    let shrink = |look: &mut WidgetLook| {
        look.text = Some(look.text.unwrap_or(base).with_font_size(MENU_FONT_SIZE));
    };
    let chip = Brush::Solid(p.elem.with_alpha(MENU_CHIP_ALPHA));
    let mb = &mut theme.menu_button;
    shrink(&mut mb.normal);
    shrink(&mut mb.hovered);
    shrink(&mut mb.pressed);
    shrink(&mut mb.disabled);
    if let Some(bg) = mb.normal.background.as_mut() {
        bg.fill = chip.clone();
    }
    if let Some(bg) = mb.disabled.background.as_mut() {
        bg.fill = chip;
    }
    let item = &mut theme.context_menu.item;
    shrink(&mut item.normal);
    shrink(&mut item.hovered);
    shrink(&mut item.disabled);
    theme
}

/// Walk `theme` and replace every colour palantir's defaults pulled from
/// its (private) `palette::*` consts with the matching entry in `p`. One
/// place so a palette swap propagates through every widget palantir
/// paints — text, buttons, text-edits, toggles, scrollbars, menus,
/// tooltips — not just the darkroom-owned chrome.
fn recolour_palantir(t: &mut palantir::Theme, p: &PalantirPalette) {
    use palantir::TextStyle;

    let muted_edge = p.text_muted.with_alpha(0.18);
    let panel_edge = p.text_muted.with_alpha(0.22);

    // Helpers cribbed from palantir's own theme defaults so the
    // structural recipe (which fill / which stroke per state) stays in
    // one shape — only the palette values diverge.
    let solid_bg = |fill: Color, stroke: Color, stroke_w: f32| Background {
        fill: fill.into(),
        stroke: Stroke::solid(stroke, stroke_w),
        corners: Corners::all(4.0),
        shadow: Shadow::NONE,
    };
    let disabled_text = Some(TextStyle::default().with_color(p.text_disabled));

    // Top-level surfaces + text.
    t.text.color = p.text;
    t.window_clear = p.terminal_bg;

    // Button — same recipe as `ButtonTheme::default`, recoloured.
    let pressed_stroke = Stroke::solid(p.border_focused, 1.0);
    t.button.normal.background = Some(solid_bg(p.elem_hover, muted_edge, 1.0));
    t.button.hovered.background = Some(solid_bg(p.elem_active, muted_edge, 1.0));
    t.button.pressed.background = Some(Background {
        fill: p.elem_active.into(),
        stroke: pressed_stroke,
        corners: Corners::all(4.0),
        shadow: Shadow::NONE,
    });
    t.button.disabled.background = Some(solid_bg(p.elem, muted_edge, 1.0));
    t.button.disabled.text = disabled_text;

    // Menu-button — `ButtonTheme::menu_button` recipe: transparent at
    // rest + disabled, hover = elem_hover, pressed = elem_active. The
    // darkroom-side `palantir_theme_for` then overlays a semi-transparent
    // node-fill chip on the normal/disabled looks for legibility over
    // busy nodes; we only need to recolour the hover/pressed fills here.
    let flat_round = |fill: Color| Background {
        fill: fill.into(),
        stroke: Stroke::ZERO,
        corners: Corners::all(4.0),
        shadow: Shadow::NONE,
    };
    t.menu_button.hovered.background = Some(flat_round(p.elem_hover));
    t.menu_button.pressed.background = Some(flat_round(p.elem_active));

    // TextEdit — stroke-width-constant recipe from `TextEditTheme::default`.
    let te_stroke_w = 1.5;
    t.text_edit.normal.background = Some(solid_bg(p.elem_hover, muted_edge, te_stroke_w));
    t.text_edit.focused.background = Some(solid_bg(p.elem_hover, p.border_focused, te_stroke_w));
    t.text_edit.disabled.background = Some(solid_bg(p.elem, muted_edge, te_stroke_w));
    t.text_edit.disabled.text = disabled_text;
    t.text_edit.placeholder = p.text_muted;
    t.text_edit.caret = p.text;
    t.text_edit.selection = p.accent.with_alpha(0.25);

    // Toggle (checkbox + radio) — same recipe as palantir's
    // `ToggleTheme::with_radius`, applied separately per toggle so the
    // corner radius (square checkbox vs pill radio) stays correct.
    let recolour_toggle = |toggle: &mut palantir::ToggleTheme, corner: f32| {
        let radius = Corners::all(corner);
        let edge = p.text_muted.with_alpha(0.35);
        let make = |fill: Color, stroke: Stroke| -> Option<Background> {
            Some(Background {
                fill: fill.into(),
                stroke,
                corners: radius,
                shadow: Shadow::NONE,
            })
        };
        toggle.unchecked.normal.background = make(p.elem_hover, Stroke::solid(edge, 1.0));
        toggle.unchecked.hovered.background = make(p.elem_active, Stroke::solid(edge, 1.0));
        toggle.unchecked.pressed.background =
            make(p.elem_active, Stroke::solid(p.border_focused, 1.0));
        toggle.unchecked.disabled.background =
            make(p.elem, Stroke::solid(edge.with_alpha(0.18), 1.0));
        toggle.unchecked.disabled.text = disabled_text;
        toggle.checked.normal.background = make(p.accent, Stroke::ZERO);
        toggle.checked.hovered.background = make(p.accent, Stroke::ZERO);
        toggle.checked.pressed.background = make(p.accent, Stroke::solid(p.border_focused, 1.0));
        toggle.checked.disabled.background = make(p.accent.with_alpha(0.45), Stroke::ZERO);
        toggle.checked.disabled.text = disabled_text;
        toggle.indicator = p.terminal_bg;
    };
    recolour_toggle(&mut t.checkbox, 3.0);
    let radio_radius = t.radio.box_size * 0.5;
    recolour_toggle(&mut t.radio, radio_radius);

    // Scrollbar — transparent track, thumb tiers via text_muted alpha
    // (matches `ScrollbarTheme::default`).
    t.scrollbar.track = Color::TRANSPARENT;
    t.scrollbar.thumb = p.text_muted.with_alpha(0.45);
    t.scrollbar.thumb_hover = p.text_muted.with_alpha(0.65);
    t.scrollbar.thumb_active = p.text_muted.with_alpha(0.85);

    // Context menu — panel + item rows + shortcut + separator.
    // (`ContextMenuTheme::default` recipe.)
    t.context_menu.panel = Background {
        fill: p.elem.into(),
        stroke: Stroke::solid(panel_edge, 1.0),
        corners: Corners::all(6.0),
        shadow: Shadow::NONE,
    };
    t.context_menu.item.normal.background = None;
    t.context_menu.item.hovered.background = Some(Background {
        fill: p.elem_hover.into(),
        stroke: Stroke::ZERO,
        corners: Corners::all(4.0),
        shadow: Shadow::NONE,
    });
    t.context_menu.item.disabled.background = None;
    t.context_menu.item.disabled.text = disabled_text;
    t.context_menu.item.shortcut = p.text_muted;
    t.context_menu.separator = p.text_muted.with_alpha(0.18);

    // Tooltip — `TooltipTheme::default` recipe; the panel keeps its
    // drop shadow so the bubble lifts off whatever it overlaps.
    t.tooltip.panel = Background {
        fill: p.elem.into(),
        stroke: Stroke::solid(panel_edge, 1.0),
        corners: Corners::all(4.0),
        shadow: Shadow {
            color: Color::linear_rgba(0.0, 0.0, 0.0, 0.6),
            offset: glam::Vec2::new(2.0, 2.0),
            blur: 5.0,
            spread: 0.0,
            inset: false,
        },
    };
    t.tooltip.text.color = p.text;
}

/// Colour swatches a [`Theme`] needs from a palette mod. Borrowed by
/// [`Theme::build`] so the two presets share one assembly path — only
/// the colour values diverge, every dimension and sub-theme comes from
/// the shared block.
struct PaletteColors {
    canvas_bg: Color,
    selection_rect: Color,
    canvas_dot: Color,
    connection_broken: Color,
    breaker_stroke: Color,
    node_fill: Color,
    node_border: Color,
    header_fill: Color,
    text_muted: Color,
    chrome_fill: Color,
    badge_subgraph: Color,
    badge_terminal: Color,
    badge_cache: Color,
    exec_executed_glow: Color,
    exec_cached_glow: Color,
    exec_running_glow: Color,
    exec_missing_glow: Color,
    exec_errored_glow: Color,
    input_port: Color,
    output_port: Color,
    input_port_hover: Color,
    output_port_hover: Color,
}

impl PaletteColors {
    const DARK: Self = Self {
        canvas_bg: dark::CANVAS_BG,
        selection_rect: dark::SELECTION_RECT,
        canvas_dot: dark::CANVAS_DOT,
        connection_broken: dark::CONNECTION_BROKEN,
        breaker_stroke: dark::BREAKER_STROKE,
        node_fill: dark::NODE_FILL,
        node_border: dark::NODE_BORDER,
        header_fill: dark::HEADER_FILL,
        text_muted: dark::TEXT_MUTED,
        chrome_fill: dark::CHROME_FILL,
        badge_subgraph: dark::BADGE_SUBGRAPH,
        badge_terminal: dark::BADGE_TERMINAL,
        badge_cache: dark::BADGE_CACHE,
        exec_executed_glow: dark::EXEC_EXECUTED_GLOW,
        exec_cached_glow: dark::EXEC_CACHED_GLOW,
        exec_running_glow: dark::EXEC_RUNNING_GLOW,
        exec_missing_glow: dark::EXEC_MISSING_GLOW,
        exec_errored_glow: dark::EXEC_ERRORED_GLOW,
        input_port: dark::INPUT_PORT,
        output_port: dark::OUTPUT_PORT,
        input_port_hover: dark::INPUT_PORT_HOVER,
        output_port_hover: dark::OUTPUT_PORT_HOVER,
    };

    const LIGHT: Self = Self {
        canvas_bg: light::CANVAS_BG,
        selection_rect: light::SELECTION_RECT,
        canvas_dot: light::CANVAS_DOT,
        connection_broken: light::CONNECTION_BROKEN,
        breaker_stroke: light::BREAKER_STROKE,
        node_fill: light::NODE_FILL,
        node_border: light::NODE_BORDER,
        header_fill: light::HEADER_FILL,
        text_muted: light::TEXT_MUTED,
        chrome_fill: light::CHROME_FILL,
        badge_subgraph: light::BADGE_SUBGRAPH,
        badge_terminal: light::BADGE_TERMINAL,
        badge_cache: light::BADGE_CACHE,
        exec_executed_glow: light::EXEC_EXECUTED_GLOW,
        exec_cached_glow: light::EXEC_CACHED_GLOW,
        exec_running_glow: light::EXEC_RUNNING_GLOW,
        exec_missing_glow: light::EXEC_MISSING_GLOW,
        exec_errored_glow: light::EXEC_ERRORED_GLOW,
        input_port: light::INPUT_PORT,
        output_port: light::OUTPUT_PORT,
        input_port_hover: light::INPUT_PORT_HOVER,
        output_port_hover: light::OUTPUT_PORT_HOVER,
    };
}

impl Theme {
    /// Derived radius for port circles — half the port side. Lives as
    /// a method instead of a stored field so the two can't drift if
    /// someone bumps `port_size` and forgets the radius.
    #[inline]
    pub fn port_radius(&self) -> f32 {
        self.port_size * 0.5
    }

    /// How far a port circle is pulled out of its column so its **center**
    /// lands on the node body's outer edge: clear the column inset
    /// (`port_col_pad_x`) and the body border (`node_border_width * 2`, which
    /// "folds into" the body's content padding), then push out by `port_radius`
    /// so the dot straddles the edge evenly. Independent of `port_size` — bigger
    /// circles keep their center on the edge.
    #[inline]
    pub fn port_overhang(&self) -> f32 {
        self.port_radius() + self.port_col_pad_x + self.node_border_width * 2.0
    }

    /// Assemble the full theme for a built-in preset. One place so
    /// startup and the Theme menu share the preset → palette mapping.
    pub fn from_preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::Dark => Self::dark(),
            ThemePreset::Light => Self::light(),
        }
    }

    /// Ayu Mirage High Contrast palette — the built-in dark look.
    pub fn dark() -> Self {
        Self::build(
            ThemePreset::Dark,
            &PaletteColors::DARK,
            &PalantirPalette::DARK,
            StaticValueEditorTheme::dark(),
            InlineRenameTheme::dark(),
        )
    }

    /// Ayu Light palette — the built-in light look (Zed's "Ayu Light"
    /// variant ported into darkroom's structure).
    pub fn light() -> Self {
        Self::build(
            ThemePreset::Light,
            &PaletteColors::LIGHT,
            &PalantirPalette::LIGHT,
            StaticValueEditorTheme::light(),
            InlineRenameTheme::light(),
        )
    }

    /// Shared assembly path: dimensions are palette-independent; `c`
    /// drives darkroom chrome, `p` drives the palantir widget
    /// recolouring, and `sve` / `inline_rename` are the per-palette
    /// per-widget bundles (handed in rather than rebuilt here so their
    /// hex values stay alongside the rest of the palette). `preset`
    /// tags which built-in produced this theme so the toggle command
    /// doesn't have to guess.
    fn build(
        preset: ThemePreset,
        c: &PaletteColors,
        p: &PalantirPalette,
        sve: StaticValueEditorTheme,
        inline_rename: InlineRenameTheme,
    ) -> Self {
        Self {
            preset,
            canvas_bg: c.canvas_bg,
            selection_rect: c.selection_rect,
            canvas_dot: c.canvas_dot,
            canvas_dot_spacing: CANVAS_DOT_SPACING,
            canvas_dot_radius: CANVAS_DOT_RADIUS,
            connection_broken: c.connection_broken,
            connection_width: CONNECTION_WIDTH,
            breaker_stroke: c.breaker_stroke,
            breaker_stroke_width: BREAKER_STROKE_WIDTH,
            node_fill: c.node_fill,
            node_border: c.node_border,
            node_border_width: NODE_BORDER_WIDTH,
            node_corner_radius: NODE_CORNER_RADIUS,
            node_min_width: NODE_MIN_WIDTH,
            node_min_height: NODE_MIN_HEIGHT,
            header_fill: c.header_fill,
            tab_corner_radius: TAB_CORNER_RADIUS,
            text_muted: c.text_muted,
            chrome_fill: c.chrome_fill,
            badge_subgraph: c.badge_subgraph,
            badge_terminal: c.badge_terminal,
            badge_cache: c.badge_cache,
            exec_executed_glow: c.exec_executed_glow,
            exec_cached_glow: c.exec_cached_glow,
            exec_running_glow: c.exec_running_glow,
            exec_missing_glow: c.exec_missing_glow,
            exec_errored_glow: c.exec_errored_glow,
            input_port: c.input_port,
            output_port: c.output_port,
            input_port_hover: c.input_port_hover,
            output_port_hover: c.output_port_hover,
            port_size: PORT_SIZE,
            port_col_pad_top: PORT_COL_PAD_TOP,
            port_col_pad_x: PORT_COL_PAD_X,
            port_gap: PORT_GAP,
            port_cols_gap: PORT_COLS_GAP,
            new_node_popup_max_height: NEW_NODE_POPUP_MAX_HEIGHT,
            static_value_editor: sve,
            inline_rename,
            palantir_theme: palantir_theme_for(p),
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

    /// Keep the checked-in `assets/ayu-graphite.toml` in sync with the
    /// const-defined [`Theme::default`]: serialize the default and rewrite
    /// the file. The asset is a generated artifact (a reference theme
    /// users can copy / the Theme → Load-Export format), not a source of
    /// truth — running the suite regenerates it, so any change to the
    /// consts (or palantir's defaults) surfaces as an asset diff to commit.
    /// Writing is idempotent when already in sync, so it's a no-op on a
    /// clean tree.
    #[test]
    fn ayu_graphite_asset_in_sync() {
        let bytes =
            common::serialize(&Theme::default(), SerdeFormat::Toml).expect("serialize theme");
        std::fs::write("assets/ayu-graphite.toml", bytes).expect("write toml asset");
    }

    /// The whole bundle — darkroom's own fields *and* the nested
    /// palantir palette — must survive a TOML round-trip; that's the
    /// on-disk format the Theme → Load / Export menu and the config
    /// rely on. Exercises the formerly-fragile case too: the tooltip's
    /// infinite max-size axis (handled by `Size`'s custom serde).
    #[test]
    fn theme_roundtrips_through_toml() {
        let mut theme = Theme {
            node_min_width: 137.5,
            text_muted: Color::hex(0x123456),
            ..Theme::default()
        };
        theme.palantir_theme.window_clear = Color::hex(0xabcdef);

        let bytes = common::serialize(&theme, SerdeFormat::Toml).expect("serialize theme");
        let back: Theme = common::deserialize(&bytes, SerdeFormat::Toml)
            .expect("theme should deserialize from its own TOML output");

        assert_eq!(back.node_min_width, 137.5);
        assert_eq!(back.text_muted, Color::hex(0x123456));
        assert_eq!(back.canvas_bg, theme.canvas_bg);
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
        assert_eq!(theme.canvas_bg, Color::hex(0x1a1a1a));
        assert_eq!(theme.input_port, Color::hex(0xdaff58));
        assert_eq!(theme.output_port, Color::hex(0xffa63d));
        // Cache (persist-to-disk) chip is the palette `warning` yellow.
        assert_eq!(theme.badge_cache, Color::hex(0xffd44a));
        assert_eq!(theme.node_min_width, 160.0);
        assert!(theme.palantir_theme.tooltip.max_size.h.is_infinite());
        // The menu-bar font was shrunk from palantir's default to ours.
        let menu_text = theme
            .palantir_theme
            .menu_button
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
        assert_eq!(dark.canvas_bg, Color::hex(0x1a1a1a));
        assert_eq!(light.canvas_bg, Color::hex(0xfcfcfc));
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
