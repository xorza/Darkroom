#!/usr/bin/env python3
"""Build the derived semantic palettes from the Ayu Graphite base palette.

`assets/ayu-graphite-base.toml` is the single source of truth: a two-tier file
where `[primitives]` holds hex values named by hue + brightness step and
`[semantic]` maps a role to a primitive name. This script resolves those refs
and emits two flat, fully-resolved palettes with an identical key set:

  assets/ayu-graphite-palette.toml — dark; a direct projection of the base.
  assets/ayu-light-palette.toml    — light; the same roles put through a
                                     role-class transform in OKLCH.

The light half cannot be a hand-picked theme — it is derived — so the transform
is stated as rules rather than swatches. Every rule was fitted against Zed's
hand-made "Ayu Light" so the output lands in the same neighbourhood; see the
constants below for the observed targets each one reproduces.

Usage:
    python3 darkroom/tools/build_palettes.py           # write both files
    python3 darkroom/tools/build_palettes.py --check   # fail if either is stale
"""

import math
import sys
import tomllib
from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"
BASE = ASSETS / "ayu-graphite-base.toml"
DARK_OUT = ASSETS / "ayu-graphite-palette.toml"
LIGHT_OUT = ASSETS / "ayu-light-palette.toml"


def to_linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def from_linear(c):
    return 12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def parse_hex(text):
    """`#rrggbb` -> linear-free sRGB floats. Alpha is not used by any role."""
    h = text.lstrip("#")
    assert len(h) == 6, f"expected #rrggbb, got {text!r}"
    return tuple(int(h[i : i + 2], 16) / 255 for i in (0, 2, 4))


def format_hex(rgb):
    return "#" + "".join(f"{round(max(0.0, min(1.0, c)) * 255):02x}" for c in rgb)


def srgb_to_oklch(rgb):
    r, g, b = (to_linear(c) for c in rgb)
    l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b
    m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b
    s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b
    l_, m_, s_ = l ** (1 / 3), m ** (1 / 3), s ** (1 / 3)
    lab_l = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_
    lab_a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_
    lab_b = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_
    return lab_l, math.hypot(lab_a, lab_b), math.degrees(math.atan2(lab_b, lab_a)) % 360


def oklch_to_srgb(lch):
    lab_l, chroma, hue = lch
    rad = math.radians(hue)
    lab_a, lab_b = chroma * math.cos(rad), chroma * math.sin(rad)
    l_ = lab_l + 0.3963377774 * lab_a + 0.2158037573 * lab_b
    m_ = lab_l - 0.1055613458 * lab_a - 0.0638541728 * lab_b
    s_ = lab_l - 0.0894841775 * lab_a - 1.2914855480 * lab_b
    l, m, s = l_**3, m_**3, s_**3
    return (
        from_linear(+4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        from_linear(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        from_linear(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
    )


def gamut_clamp(lch):
    """Nearest in-gamut sRGB at this lightness and hue, by bisecting chroma.

    Lightness carries the role's meaning (a surface that lands one step off is a
    surface that stops being distinguishable), so chroma is what gives way.
    """
    if all(-0.001 <= c <= 1.001 for c in oklch_to_srgb(lch)):
        return oklch_to_srgb(lch)
    lab_l, chroma, hue = lch
    lo, hi = 0.0, chroma
    for _ in range(24):
        mid = (lo + hi) / 2
        if all(-0.001 <= c <= 1.001 for c in oklch_to_srgb((lab_l, mid, hue))):
            lo = mid
        else:
            hi = mid
    return oklch_to_srgb((lab_l, lo, hue))


def luminance(rgb):
    r, g, b = (to_linear(c) for c in rgb)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(fg, bg):
    a, b = luminance(fg), luminance(bg)
    return (max(a, b) + 0.05) / (min(a, b) + 0.05)


# Roles whose light counterpart is not a foreground. Everything absent from
# these lists is a foreground, split by chroma between INK and ACCENT: a role
# under CHROMA_SPLIT is de-emphasized ink (comments, docs, line numbers), one
# over it is a hue that has to stay a hue (syntax, status, ports).
SURFACES = [
    "terminal_bg",
    "bg",
    "panel",
    "surface",
    "elem",
    "elem_hover",
    "elem_active",
    "elem_disabled",
    "title_bar",
    "title_bar_inactive",
    "chat_msg_bg",
    "border",
]
TINT_BG = [
    "info_bg",
    "hint_bg",
    "success_bg",
    "warning_bg",
    "error_bg",
    "diagnostic_muted_bg",
    "selection_bg",
    "diff_created_bg",
    "diff_deleted_bg",
]
TINT_BORDER = [
    "info_border",
    "hint_border",
    "success_border",
    "warning_border",
    "error_border",
    "border_focused",
]
# Roles that carry across unchanged. A scrim is an alpha wash over whatever is
# under it, so it is black in both modes. `on_accent` is ink for a bright fill
# and both palettes keep their accents at the bright end — it is defined against
# the fill, not against the background, so mirroring it would invert the one
# relationship it exists to hold.
VERBATIM = ["overlay_black", "on_accent", "on_accent_muted"]

CHROMA_SPLIT = 0.075

# Interaction states are the one place ordering must flip rather than carry. A
# dark theme lifts a control toward the light to emphasize it; a light theme
# pushes it toward the dark. Each of these keeps its dark-mode distance from its
# anchor and reverses the sign, so hover still means "more" in both modes.
MIRRORED_STATES = {"accent_hover": "accent", "accent_active": "accent"}

# Light-mode surface ramp. The dark ramp runs deepest (terminal/editor) to
# lightest (elem_active); light reverses it, so the editor is the near-white
# floor and each interactive step sits *below* the resting fill. The span is
# compressed because a light theme separates layers on less lightness than a
# dark one — Ayu Light's chrome spans 0.86..0.99 against Mirage's 0.22..0.41.
SURFACE_TOP = 0.99
SURFACE_SCALE = 0.62
# The border rides the same ramp but is pushed deeper than the mirror gives it.
# Contrast compresses hard at the top of sRGB: the lightness step that separates
# border from elem down at L 0.35 is worth 1.27:1, and the same step up at L 0.90
# is worth 1.08:1. Ayu Light opens its border to dL 0.084 for exactly this.
BORDER_DEEPEN = 0.055

# Foreground inks keep their distance from the background, scaled. Fitted on
# Ayu Light: text 0.490 (predicts 0.490), syn_comment 0.755 (predicts 0.748),
# syn_punctuation 0.567 (predicts 0.544).
INK_SCALE = 0.72
INK_CHROMA = 0.80

# Hues compress toward a mid band instead of mirroring — a light theme's accents
# sit near L 0.71 whatever their dark-mode lightness was, because a dark theme
# pushes them bright to clear a dark ground and a light one cannot. Chroma comes
# *down* a little on the way, which is the opposite of the folk rule that light
# surfaces need more saturation: at L 0.7 the warm hues are already against the
# sRGB boundary, so asking for more chroma only trades lightness away and turns
# the yellows olive. Swept against Ayu Light over MID 0.64..0.80 and chroma
# 0.85..1.30; 0.71 / 0.90 sits at the minimum (mean dE 0.061 over 12 hues).
ACCENT_MID = 0.71
ACCENT_PIVOT = 0.82
ACCENT_SCALE = 0.45
ACCENT_RANGE = (0.56, 0.80)
ACCENT_CHROMA = 0.90

# Status tints: near-white washes with most of the chroma taken out, the border
# a step deeper so a chip reads as a chip. Ayu Light lands its tint backgrounds
# at 0.94 +/- 0.02 and its tint borders at 0.89 +/- 0.03.
TINT_BG_L = 0.94
TINT_BG_CHROMA = 0.42
TINT_BORDER_L = 0.89
TINT_BORDER_CHROMA = 0.62

# ANSI is the one family that does not mirror: a terminal's black stays the dark
# end and its white the light end in both modes, or every program that hardcodes
# a colour comes out wrong. The neutral rungs compress into a band that is
# legible on a white ground while keeping dim < normal < bright.
ANSI_NEUTRAL_FLOOR = 0.15
ANSI_NEUTRAL_SCALE = 0.63


def resolve(base):
    """Semantic role -> hex, with the refs into `[primitives]` followed."""
    primitives, semantic = base["primitives"], base["semantic"]
    out = {}
    for role, ref in semantic.items():
        assert ref in primitives, f"[semantic] {role} references unknown primitive {ref!r}"
        out[role] = primitives[ref]
    return out


def add_derived(roles):
    """Roles the base does not name because they restate one that it does."""
    bg, panel = srgb_to_oklch(parse_hex(roles["bg"])), srgb_to_oklch(parse_hex(roles["panel"]))
    # One more rung down the neutral ramp, at the step the ramp already uses.
    step = panel[0] - bg[0]
    roles["terminal_bg"] = format_hex(gamut_clamp((bg[0] - step, bg[1], bg[2])))
    roles["diff_created"] = roles["success"]
    roles["diff_created_bg"] = roles["success_bg"]
    roles["diff_deleted"] = roles["error"]
    roles["diff_deleted_bg"] = roles["error_bg"]
    return roles


def to_light(roles):
    """Dark role map -> light role map, one rule per role class."""
    lch = {role: srgb_to_oklch(parse_hex(hexstr)) for role, hexstr in roles.items()}
    surface_floor = min(lch[role][0] for role in SURFACES)
    bg_l = lch["bg"][0]
    out = {}

    def accent_lightness(dark_l):
        shifted = ACCENT_MID + (dark_l - ACCENT_PIVOT) * ACCENT_SCALE
        return min(max(shifted, ACCENT_RANGE[0]), ACCENT_RANGE[1])

    for role, (lightness, chroma, hue) in lch.items():
        if role in VERBATIM:
            out[role] = roles[role]
            continue
        if role in MIRRORED_STATES:
            anchor = MIRRORED_STATES[role]
            delta = lightness - lch[anchor][0]
            target = (accent_lightness(lch[anchor][0]) - delta, chroma * ACCENT_CHROMA, hue)
        elif role in SURFACES:
            mirrored = SURFACE_TOP - (lightness - surface_floor) * SURFACE_SCALE
            target = (mirrored - (BORDER_DEEPEN if role == "border" else 0.0), chroma, hue)
        elif role in TINT_BG:
            target = (TINT_BG_L, chroma * TINT_BG_CHROMA, hue)
        elif role in TINT_BORDER:
            target = (TINT_BORDER_L, chroma * TINT_BORDER_CHROMA, hue)
        elif role.startswith("ansi_") and chroma < CHROMA_SPLIT:
            target = (ANSI_NEUTRAL_FLOOR + lightness * ANSI_NEUTRAL_SCALE, chroma, hue)
        elif chroma < CHROMA_SPLIT:
            target = (SURFACE_TOP - (lightness - bg_l) * INK_SCALE, chroma * INK_CHROMA, hue)
        else:
            target = (accent_lightness(lightness), chroma * ACCENT_CHROMA, hue)
        out[role] = format_hex(gamut_clamp(target))
    return out


SECTIONS = [
    (
        "backgrounds",
        "Chrome layers. Each step must stay distinguishable from the one under\n"
        "it — a control that shares its fill with the panel it sits on\n"
        "disappears. `terminal_bg` is one rung below `bg`: the terminal pane in\n"
        "an editor, the graph canvas in darkroom.",
        [
            "terminal_bg",
            "bg",
            "panel",
            "surface",
            "elem",
            "elem_hover",
            "elem_active",
            "elem_disabled",
            "title_bar",
            "title_bar_inactive",
            "chat_msg_bg",
            "overlay_black",
        ],
    ),
    ("borders", None, ["border", "border_focused"]),
    (
        "text",
        "`on_accent` is the ink for anything drawn on a bright fill — accent\n"
        "buttons, unread badges, status chips. `text` never clears 2:1 on one.",
        ["text", "text_muted", "text_disabled", "on_accent", "on_accent_muted"],
    ),
    (
        "accent_status",
        None,
        ["accent", "accent_hover", "accent_active", "success", "warning", "error", "hint"],
    ),
    ("selection", None, ["selection_bg", "selection_fg"]),
    (
        "status_bg",
        None,
        [
            "info_bg",
            "info_border",
            "hint_bg",
            "hint_border",
            "success_bg",
            "success_border",
            "warning_bg",
            "warning_border",
            "error_bg",
            "error_border",
            "diagnostic_muted_bg",
        ],
    ),
    (
        "diff",
        "`created` / `deleted` restate success / error; the `term_` pair is the\n"
        "louder terminal-pane variant.",
        [
            "diff_created",
            "diff_created_bg",
            "diff_deleted",
            "diff_deleted_bg",
            "diff_term_plus",
            "diff_term_minus",
        ],
    ),
    (
        "editor",
        None,
        [
            "line_number",
            "line_number_active",
            "line_number_hover",
            "scrollbar_thumb",
            "drop_target",
            "search_highlight",
            "search_match_active",
        ],
    ),
    (
        "ansi",
        "Bright is lighter than normal, dim is darker, per hue — in both modes.\n"
        "This family does not mirror: black stays the dark end and white the\n"
        "light end, or every program that hardcodes a colour comes out wrong.",
        [
            "ansi_black",
            "ansi_red",
            "ansi_green",
            "ansi_yellow",
            "ansi_blue",
            "ansi_magenta",
            "ansi_cyan",
            "ansi_white",
            "ansi_bright_black",
            "ansi_bright_red",
            "ansi_bright_green",
            "ansi_bright_yellow",
            "ansi_bright_blue",
            "ansi_bright_magenta",
            "ansi_bright_cyan",
            "ansi_bright_white",
            "ansi_dim_black",
            "ansi_dim_red",
            "ansi_dim_green",
            "ansi_dim_yellow",
            "ansi_dim_blue",
            "ansi_dim_magenta",
            "ansi_dim_cyan",
            "ansi_dim_white",
        ],
    ),
    ("misc", None, ["player_bg", "chat_check"]),
    (
        "syntax",
        None,
        [
            "syn_keyword",
            "syn_function",
            "syn_string",
            "syn_string_regex",
            "syn_comment",
            "syn_number",
            "syn_type",
            "syn_operator",
            "syn_attribute",
            "syn_punctuation",
            "syn_doc",
            "syn_string_special",
            "syn_predictive",
        ],
    ),
]

DARK_HEADER = """\
# Ayu Graphite — dark semantic palette.
#
# GENERATED by tools/build_palettes.py from assets/ayu-graphite-base.toml, which is
# the single source of truth. Edit that file, not this one.
#
# A direct projection: every value is the primitive its role names in the base,
# with the ref followed.
"""

LIGHT_HEADER = """\
# Ayu Light — light semantic palette.
#
# GENERATED by tools/build_palettes.py from assets/ayu-graphite-base.toml, which is
# the single source of truth. Edit that file, not this one.
#
# Derived, not hand-picked: each role goes through an OKLCH transform chosen by
# its class — surfaces mirror into a near-white ramp, inks keep their distance
# from the background, hues compress toward a mid band and gain the chroma a
# light surface needs, status tints wash out to near-white, and ANSI keeps its
# absolute black-to-white ordering. The rules and their fitted constants live at
# the top of the generator.
"""


def render(header, roles):
    lines = [header]
    for name, note, keys in SECTIONS:
        lines.append(f"[{name}]")
        if note:
            lines.extend(f"# {line}" for line in note.split("\n"))
        for key in keys:
            lines.append(f'{key} = "{roles[key]}"')
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n"


def audit(roles, label, light):
    """Report the invariants the base palette's header commits to."""
    problems = []
    stack = ["bg", "panel", "elem", "elem_hover", "elem_active"]
    lightness = {role: srgb_to_oklch(parse_hex(roles[role]))[0] for role in roles}
    for lower, upper in zip(stack, stack[1:]):
        delta = lightness[upper] - lightness[lower]
        # Light mode reverses the ramp: elevation and interaction read darker.
        if (delta < 0.015) if not light else (delta > -0.015):
            problems.append(f"{lower} -> {upper} do not separate (dL {delta:+.3f})")
    for under in ("panel", "elem"):
        ratio = contrast(parse_hex(roles["border"]), parse_hex(roles[under]))
        if ratio < 1.1:
            problems.append(f"border is invisible on {under} ({ratio:.2f}:1)")
    for role, floor in (("text", 4.5), ("text_muted", 3.0), ("text_disabled", 2.0)):
        ratio = contrast(parse_hex(roles[role]), parse_hex(roles["bg"]))
        if ratio < floor:
            problems.append(f"{role} on bg is {ratio:.2f}:1, under {floor}")
    for fill in ("accent", "success", "warning", "error"):
        ratio = contrast(parse_hex(roles["on_accent"]), parse_hex(roles[fill]))
        if ratio < 4.5:
            problems.append(f"on_accent on {fill} is {ratio:.2f}:1, under 4.5")
    for problem in problems:
        print(f"{label}: {problem}", file=sys.stderr)
    return not problems


def main():
    check = "--check" in sys.argv[1:]
    with BASE.open("rb") as handle:
        roles = add_derived(resolve(tomllib.load(handle)))

    placed = [key for _, _, keys in SECTIONS for key in keys]
    assert len(placed) == len(set(placed)), "a role is emitted into two sections"
    missing, extra = set(roles) - set(placed), set(placed) - set(roles)
    assert not missing, f"roles with no section: {sorted(missing)}"
    assert not extra, f"sections name roles the base does not define: {sorted(extra)}"

    light = to_light(roles)
    clean = audit(roles, "dark", light=False) & audit(light, "light", light=True)

    stale = False
    for path, header, table in ((DARK_OUT, DARK_HEADER, roles), (LIGHT_OUT, LIGHT_HEADER, light)):
        text = render(header, table)
        if check:
            if not path.exists() or path.read_text() != text:
                print(f"stale: {path.name}", file=sys.stderr)
                stale = True
        else:
            path.write_text(text)
            print(f"wrote {path.relative_to(ASSETS.parent.parent)}")
    return 1 if (stale or not clean) else 0


if __name__ == "__main__":
    sys.exit(main())
