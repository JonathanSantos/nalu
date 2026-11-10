#!/usr/bin/env python3
"""Nalu brand & design system: one source for the mark, the palette, the type and the tokens.

    python3 docs/brand/build.py

Writes:
  logo/*.svg          mark, wordmark, lockup, app icon, favicon (light, dark and one-color)
  tokens/tokens.css   CSS variables (light and dark themes)
  tokens/tokens.json  the same tokens in W3C Design Tokens format, plus the terminal palette
  index.html          the brand guide (generated in full; edit this file, not the HTML)
  README.md           the brand summary (generated in full)

Also importable: `docs/art/build.py` draws the README art from these same tokens and shapes.

The mark lives on a 64 x 64 grid and is a single filled path (no strokes), so it works in one color,
in any editor and at 16 px. Standard library only. Exits with 1 if any text/background pair used by
the system falls under WCAG 4.5:1 (3:1 for the mark).
"""
from __future__ import annotations

import html
import json
import math
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
LOGO = HERE / "logo"
TOKENS = HERE / "tokens"
ROOT = HERE.parent.parent
EXAMPLES = ROOT / "examples"


# ════════════════════════════════ color ════════════════════════════════
# A Hawaiian shoreline at noon: lagoon water over reef, deep ocean beyond it. Cool whites instead of sand,
# so code and diagrams read clean. Each lively color has exactly one job.
MATERIAL = {
    "moana":    ("#0b2b3d", "Moana, the open ocean: ink on light, the deep surfaces on dark"),
    "abyss":    ("#07161f", "Abyss: the dark background and the terminal"),
    "foam":     ("#e6f3f1", "Foam: ink on dark"),
    "salt":     ("#f3f8f8", "Salt: light surfaces"),
    "reef":     ("#0a7480", "Reef: the mark and actions on light"),
    "lagoon":   ("#36d1c4", "Lagoon: the mark and actions on dark; the signature color"),
    "lehua":    ("#c42c46", "Lehua blossom: failure and recovery on light"),
    "lehua-lt": ("#ff8094", "Lehua blossom: failure and recovery on dark"),
    "limu":     ("#1d7a4c", "Limu (seaweed): success on light"),
    "limu-lt":  ("#74d7a0", "Limu (seaweed): success on dark"),
    "olena":    ("#8f5600", "ʻŌlena (turmeric): warnings on light"),
    "olena-lt": ("#f5be5b", "ʻŌlena (turmeric): warnings on dark"),
    "sand":     ("#f1e9da", "Sand: the shore, where generated scripts land (illustrations only)"),
}
M = {k: v[0] for k, v in MATERIAL.items()}

# Roles per theme. "@name" points to a material.
ROLES = {
    "light": {
        "bg": "#ffffff", "surface": "@salt", "surface-2": "#e9f2f2", "code-bg": "#eef5f5",
        "ink": "@moana", "ink-soft": "#4a6573", "rule": "#d6e4e6",
        "action": "@reef", "fail": "@lehua", "ok": "@limu", "warn": "@olena",
        "mark": "@reef", "shore": "@sand", "shore-ink": "#5c4a2a",
    },
    "dark": {
        "bg": "@abyss", "surface": "#0c202b", "surface-2": "#11293a", "code-bg": "#0a1c26",
        "ink": "@foam", "ink-soft": "#93acb4", "rule": "#1d3746",
        "action": "@lagoon", "fail": "@lehua-lt", "ok": "@limu-lt", "warn": "@olena-lt",
        "mark": "@lagoon", "shore": "#1f2a26", "shore-ink": "#e3d3b3",
    },
}
ROLE_NOTES = {
    "action": "what you write and what you do: keywords, links, actions, {x}, $",
    "fail": "failure and recovery: catch, fail, ignore, try, errors",
    "ok": "it worked: passing tests, strings in code",
    "warn": "warnings",
    "mark": "the mark (always one color)",
    "shore": "where generated scripts land, in diagrams",
}

# The terminal is always dark (the abyss), in both page themes.
TERMINAL = {
    "bg":    {"truecolor": "#04111a", "ansi": None,   "use": "terminal block background"},
    "fg":    {"truecolor": "#d9ebe8", "ansi": "0",    "use": "regular text"},
    "dim":   {"truecolor": "#7f98a1", "ansi": "2",    "use": "the | gutter, line numbers, '+ cmd' from --verbose, '-->'"},
    "bold":  {"truecolor": "#f2faf8", "ansi": "1",    "use": "error location, generated file names"},
    "error": {"truecolor": M["lehua-lt"], "ansi": "1;31", "use": "'error[E…]:', 'error:', the ^^^ underline, 'FAILED'"},
    "warn":  {"truecolor": M["olena-lt"], "ansi": "33",   "use": "'warning[W…]:', 'warning:'"},
    "help":  {"truecolor": M["lagoon"],   "ansi": "36",   "use": "'= help:', '[dry-run]', 'info:'"},
    "ok":    {"truecolor": M["limu-lt"],  "ansi": "32",   "use": "'ok' (tests and build)"},
}


def resolve(v: str) -> str:
    return M[v[1:]] if v.startswith("@") else v


def theme(name: str) -> dict[str, str]:
    return {k: resolve(v) for k, v in ROLES[name].items()}


# ─────────────────────────── WCAG contrast ───────────────────────────
def luminance(hex_: str) -> float:
    h = hex_.lstrip("#")

    def ch(i):
        c = int(h[i:i + 2], 16) / 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(0) + 0.7152 * ch(2) + 0.0722 * ch(4)


def ratio(a: str, b: str) -> float:
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


TEXT_FG = ["ink", "ink-soft", "action", "fail", "ok", "warn"]
TEXT_BG = ["bg", "surface", "surface-2", "code-bg"]


def contrast_rows():
    """[(theme, fg, bg, ratio, minimum)] for every pairing the system uses."""
    rows = []
    for t in ("light", "dark"):
        c = theme(t)
        for bg in TEXT_BG:
            for fg in TEXT_FG:
                rows.append((t, fg, bg, ratio(c[fg], c[bg]), 4.5))
        rows.append((t, "mark", "bg", ratio(c["mark"], c["bg"]), 3.0))
        rows.append((t, "shore-ink", "shore", ratio(c["shore-ink"], c["shore"]), 4.5))
    tb = TERMINAL["bg"]["truecolor"]
    for k in ("fg", "dim", "bold", "error", "warn", "help", "ok"):
        rows.append(("terminal", k, "bg", ratio(TERMINAL[k]["truecolor"], tb), 4.5))
    return rows


# ════════════════════════════════ shapes ════════════════════════════════
def n(x: float) -> str:
    """Short SVG number: at most 2 decimals, no trailing zeros."""
    s = f"{x:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def band(cx: float, cy: float, r: float, t: float, a0: float, a1: float) -> str:
    """A filled band of thickness t around the centerline arc (center, radius r), from a0 to a1 degrees,
    clockwise on screen. Outer edge clockwise, inner edge back: every piece winds the same way, so
    overlapping pieces never punch holes (nonzero fill)."""
    ro, ri = r + t / 2, r - t / 2

    def p(rr, a):
        return cx + rr * math.cos(math.radians(a)), cy + rr * math.sin(math.radians(a))
    large = 1 if (a1 - a0) % 360 > 180 else 0
    (x0, y0), (x1, y1) = p(ro, a0), p(ro, a1)
    (x2, y2), (x3, y3) = p(ri, a1), p(ri, a0)
    return (f"M{n(x0)} {n(y0)}A{n(ro)} {n(ro)} 0 {large} 1 {n(x1)} {n(y1)}"
            f"L{n(x2)} {n(y2)}A{n(ri)} {n(ri)} 0 {large} 0 {n(x3)} {n(y3)}Z")


def disc(cx: float, cy: float, r: float) -> str:
    return f"M{n(cx - r)} {n(cy)}A{n(r)} {n(r)} 0 1 1 {n(cx + r)} {n(cy)}A{n(r)} {n(r)} 0 1 1 {n(cx - r)} {n(cy)}Z"


def rect(x: float, y: float, w: float, h: float) -> str:
    return f"M{n(x)} {n(y)}h{n(w)}v{n(h)}h{n(-w)}z"


def capsule(x: float, y0: float, y1: float, w: float, top: bool = True, bottom: bool = True) -> str:
    """A vertical stem from y0 to y1 with round ends (the ends stay inside y0..y1)."""
    r = w / 2
    a = y0 + (r if top else 0)
    b = y1 - (r if bottom else 0)
    d = rect(x, a, w, b - a)
    if top:
        d += disc(x + r, a, r)
    if bottom:
        d += disc(x + r, b, r)
    return d


# ─────────────────────────── the mark ───────────────────────────
# One stroke, three readings: the stem and arch of a lowercase n; the crest of a wave that curls into
# its own barrel (nalu); and the spiral of a shell, the thing a script runs in. Centerline arcs on a
# 64 grid, all centers on the line y = 31, each turn smaller than the last:
#   stem    x = 11.5, from the foot (y 54) up to y 31
#   crest   R 21 around (32.5, 31): up and over, left to right
#   curl    R 14 around (39.5, 31): down and back under
#   barrel  R  8 around (33.5, 31): up again, ending in a round cap at (41.5, 31)
# Stroke 9, round caps. Bounding box 7–58 x 5.5–58.5: centered on the grid.
MARK = dict(t=9.0, x0=11.5, cy=31.0, r1=21.0, r2=14.0, r3=8.0, foot=54.0)


def mark_path(t=MARK["t"], x0=MARK["x0"], cy=MARK["cy"], r1=MARK["r1"], r2=MARK["r2"], r3=MARK["r3"], foot=MARK["foot"]) -> str:
    c1 = x0 + r1
    c2 = c1 + r1 - r2
    c3 = c2 - r2 + r3
    return (
        rect(x0 - t / 2, cy, t, foot - cy) + disc(x0, foot, t / 2)      # stem with a round foot
        + band(c1, cy, r1, t, 180, 360)                                 # crest
        + band(c2, cy, r2, t, 0, 180)                                   # curl
        + band(c3, cy, r3, t, 180, 360)                                 # barrel
        + disc(c3 + r3, cy, t / 2)                                      # the lip
    )


MARK_D = mark_path()
MARK_CENTERS = [  # (center x, center y, radius) of the centerline arcs, for the construction drawing
    (MARK["x0"] + MARK["r1"], MARK["cy"], MARK["r1"]),
    (MARK["x0"] + 2 * MARK["r1"] - MARK["r2"], MARK["cy"], MARK["r2"]),
    (MARK["x0"] + 2 * MARK["r1"] - 2 * MARK["r2"] + MARK["r3"], MARK["cy"], MARK["r3"]),
]

# ─────────────────────────── the wordmark: n a l u ───────────────────────────
# Geometric lowercase drawn for the brand (not a font), all from one ring: outer radius 15, inner 8.5
# (stroke 6.5), x-height 30, ascender 44. Round terminals, like the mark. The n's arch is the crest; the
# u is the n turned over (the trough); the l is a single stem.
RO, RI = 15.0, 8.5
STEM = RO - RI
XH_TOP, BASE, ASC = 14.0, 44.0, 0.0
CY = XH_TOP + RO
GAP = 7.0
RC = (RO + RI) / 2          # centerline radius of the rings


def ring(cx: float) -> str:
    return (f"M{n(cx - RO)} {n(CY)}A{n(RO)} {n(RO)} 0 1 1 {n(cx + RO)} {n(CY)}A{n(RO)} {n(RO)} 0 1 1 {n(cx - RO)} {n(CY)}z"
            f"M{n(cx - RI)} {n(CY)}A{n(RI)} {n(RI)} 0 1 0 {n(cx + RI)} {n(CY)}A{n(RI)} {n(RI)} 0 1 0 {n(cx - RI)} {n(CY)}z")


def letters() -> tuple[str, float]:
    x = 0.0
    out = []
    # n
    cx = x + RO
    out.append(band(cx, CY, RC, STEM, 180, 360) + capsule(x, CY, BASE, STEM, top=False) + capsule(cx + RI, CY, BASE, STEM, top=False))
    x += 2 * RO + GAP
    # a
    cx = x + RO
    out.append(ring(cx) + capsule(cx + RI, CY, BASE, STEM, top=False))
    x += 2 * RO + GAP
    # l
    out.append(capsule(x, ASC, BASE, STEM))
    x += STEM + GAP
    # u
    cx = x + RO
    out.append(band(cx, CY, RC, STEM, 0, 180) + capsule(x, XH_TOP, CY, STEM, bottom=False) + capsule(cx + RI, XH_TOP, BASE, STEM))
    x += 2 * RO
    return "".join(out), x


WORD_D, WORD_W = letters()

# Lockup: the wordmark's baseline sits on the mark's foot (y 58.5); its height is 82% of the mark's,
# so the mark leads. The gap between them is the stroke width of the mark times 2.
LOCK_S = (58.5 - 5.5) / BASE * 0.82
LOCK_X = 58 + 2 * MARK["t"]
LOCK_Y = 58.5 - BASE * LOCK_S
LOCK_W = LOCK_X + WORD_W * LOCK_S


def svg(view_box: str, body: str, label: str = "Nalu", title: str | None = None) -> str:
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_box}" role="img" aria-label="{label}">\n'
            f"  <title>{title or label}</title>\n{body}</svg>\n")


def mark(color: str, ind: str = "  ") -> str:
    return f'{ind}<path fill="{color}" d="{MARK_D}"/>\n'


def word(color: str, ind: str = "  ") -> str:
    return f'{ind}<path fill="{color}" d="{WORD_D}"/>\n'


def lockup(ink: str, mark_c: str) -> str:
    return mark(mark_c) + (f'  <g transform="translate({n(LOCK_X)} {n(LOCK_Y)}) scale({LOCK_S:.4f})">\n'
                           + word(ink, "    ") + "  </g>\n")


LOCK_VB = f"3 1 {n(LOCK_W + 4 - 3)} 62"
WORD_VB = f"-3 -3 {n(WORD_W + 6)} {n(BASE + 6)}"


def favicon() -> str:
    L, D = theme("light"), theme("dark")
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n'
            f"  <style>path {{ fill: {L['mark']}; }} @media (prefers-color-scheme: dark) {{ path {{ fill: {D['mark']}; }} }}</style>\n"
            f'  <path d="{MARK_D}"/>\n</svg>\n')


def app_icon() -> str:
    # 512 tile, the mark at 6x with 64 of margin, on the abyss.
    return svg("0 0 512 512",
               f'  <rect width="512" height="512" rx="114" fill="{M["abyss"]}"/>\n'
               f'  <g transform="translate(64 64) scale(6)">\n' + mark(M["lagoon"], "    ") + "  </g>\n",
               title="Nalu (app icon)")


def build_svgs() -> dict[str, str]:
    L, D = theme("light"), theme("dark")
    return {
        "nalu-mark.svg": svg("0 0 64 64", mark(L["mark"])),
        "nalu-mark-inverse.svg": svg("0 0 64 64", mark(D["mark"]), title="Nalu (dark backgrounds)"),
        "nalu-mark-mono.svg": svg("0 0 64 64", mark("currentColor"), title="Nalu (one color)"),
        "nalu-lockup.svg": svg(LOCK_VB, lockup(L["ink"], L["mark"])),
        "nalu-lockup-inverse.svg": svg(LOCK_VB, lockup(D["ink"], D["mark"]), title="Nalu (dark backgrounds)"),
        "nalu-lockup-mono.svg": svg(LOCK_VB, lockup("currentColor", "currentColor"), title="Nalu (one color)"),
        "nalu-wordmark.svg": svg(WORD_VB, word(L["ink"])),
        "nalu-wordmark-inverse.svg": svg(WORD_VB, word(D["ink"]), title="Nalu (dark backgrounds)"),
        "nalu-app-icon.svg": app_icon(),
        "nalu-favicon.svg": favicon(),
    }


# ════════════════════════════════ type ════════════════════════════════
# Recursive (Arrow Type, OFL): one superfamily that spans proportional and monospaced, with a Casual
# axis. Code is the product, so the same letterforms carry the brand, the prose and the code; the
# Casual axis gives the display a brushed, wave-like warmth that the strict mono never has.
FONTS_URL = ("https://fonts.googleapis.com/css2?family=Recursive:slnt,wght,CASL,CRSV,MONO"
             "@-15..0,300..1000,0..1,0..1,0..1&display=swap")
FONT_STACK = ["Recursive", "ui-sans-serif", "system-ui", "-apple-system", "Segoe UI", "Helvetica Neue", "Arial", "sans-serif"]
MONO_STACK = ["Recursive", "ui-monospace", "SF Mono", "Menlo", "Consolas", "Liberation Mono", "monospace"]
FONT = {
    "display": (FONT_STACK, "'MONO' 0, 'CASL' 0.6", 800, "headlines and big numbers; never paragraphs or code"),
    "body":    (FONT_STACK, "'MONO' 0, 'CASL' 0",   400, "prose and interface"),
    "label":   (MONO_STACK, "'MONO' 1, 'CASL' 0",   600, "eyebrows, tags and table heads: uppercase, tracked"),
    "mono":    (MONO_STACK, "'MONO' 1, 'CASL' 0",   400, "code, terminal, values; no ligatures"),
}
SIZE = {"xs": 12, "sm": 14, "md": 16, "lg": 19, "xl": 23, "2xl": 28, "3xl": 36, "4xl": 48, "5xl": 68}   # ~1.2 ratio
SPACE = {"1": 4, "2": 8, "3": 12, "4": 16, "5": 24, "6": 32, "7": 48, "8": 72, "9": 112}
RADIUS = {"sm": 6, "md": 10, "lg": 16, "xl": 24, "pill": 999}

# Syntax roles: role → (theme color role, style, what it covers)
SYNTAX = {
    "keyword": ("action", "normal", "let var fn return struct enum param if else for in while match break continue defer parallel retry native test self true false"),
    "fail":    ("fail", "bold", "catch fail ignore try"),
    "string":  ("ok", "normal", "\"…\" and '…'"),
    "interp":  ("action", "normal", "the braces of {x}, in strings and commands"),
    "command": ("action", "bold", "the $ of $ cmd and $( )"),
    "program": ("ink", "bold", "the program name after $"),
    "comment": ("ink-soft", "italic", "// and ///"),
    "type":    ("ink-soft", "normal", "str int bool path duration secret and capitalized names"),
    "plain":   ("ink", "normal", "the rest: names, numbers, operators"),
}


def font_css_value(fams: list[str]) -> str:
    return ", ".join(f"'{f}'" if " " in f else f for f in fams)


def css_theme_block(t: str, ind: str) -> str:
    lines = []
    for k, v in ROLES[t].items():
        val = f"var(--nalu-{v[1:]})" if v.startswith("@") else v
        note = f"  /* {ROLE_NOTES[k]} */" if k in ROLE_NOTES else ""
        lines.append(f"{ind}--{k}: {val};{note}")
    lines.append(f"{ind}color-scheme: {t};")
    return "\n".join(lines)


def tokens_css() -> str:
    mats = "\n".join(f"  --nalu-{k}: {v};{' ' * (9 - len(k))}/* {d} */" for k, (v, d) in MATERIAL.items())
    syn = "\n".join(f"  --syn-{k}: var(--{role});{' ' * (17 - len(k) - len(role))}/* {style}: {what} */"
                    for k, (role, style, what) in SYNTAX.items())
    term = "\n".join(f"  --term-{k}: {v['truecolor']};{' ' * (6 - len(k))}/* {('ANSI ' + v['ansi'] + ': ') if v['ansi'] else ''}{v['use']} */"
                     for k, v in TERMINAL.items())
    fonts = "\n".join(f"  --font-{k}: {font_css_value(f)};\n  --font-{k}-axes: {axes};\n  --font-{k}-weight: {w};  /* {note} */"
                      for k, (f, axes, w, note) in FONT.items())
    sizes = "\n".join(f"  --text-{k}: {v}px;" for k, v in SIZE.items())
    space = "\n".join(f"  --space-{k}: {v}px;" for k, v in SPACE.items())
    radius = "\n".join(f"  --radius-{k}: {v}px;" for k, v in RADIUS.items())
    return f"""/* Nalu design tokens. GENERATED by build.py: edit it there and run
 *   python3 docs/brand/build.py
 * One source for the brand guide, the README art, the site and the tools.
 *
 * Theme follows the system; force it with <html data-theme="light|dark">.
 * Fonts (Google Fonts, OFL):
 *   {FONTS_URL}
 * Use: font-family: var(--font-body); font-variation-settings: var(--font-body-axes);
 */

:root {{
  /* ---- materials (fixed; they don't change with the theme) ---- */
{mats}

  /* ---- roles: light theme ---- */
{css_theme_block("light", "  ")}

  /* ---- code (syntax highlighting) ---- */
{syn}

  /* ---- terminal (always dark, in both themes) ---- */
{term}

  /* ---- type ---- */
{fonts}
{sizes}
  --leading-body: 1.6;
  --leading-title: 1.05;
  --tracking-title: -.025em;
  --tracking-label: .12em;

  /* ---- shape and space ---- */
{radius}
{space}
}}

@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
{css_theme_block("dark", "    ")}
  }}
}}
:root[data-theme="dark"] {{
{css_theme_block("dark", "  ")}
}}
"""


def tokens_json() -> str:
    def ref(v):
        return "{material." + v[1:] + "}" if v.startswith("@") else v
    data = {
        "$description": "Nalu brand tokens (W3C Design Tokens). Mirrors tokens.css; generated by build.py.",
        "material": {k: {"$type": "color", "$value": v, "$description": d} for k, (v, d) in MATERIAL.items()},
        "color": {
            t: {k: ({"$type": "color", "$value": ref(v)} | ({"$description": ROLE_NOTES[k]} if k in ROLE_NOTES else {}))
                for k, v in ROLES[t].items()}
            for t in ("light", "dark")
        },
        "syntax": {
            k: {"$type": "color", "$value": "{color.light." + role + "}", "$description": f"{what} ({style})",
                "$extensions": {"nalu.dark": "{color.dark." + role + "}", "nalu.fontStyle": style}}
            for k, (role, style, what) in SYNTAX.items()
        },
        "font": {k: {"$type": "fontFamily", "$value": f, "$description": note,
                     "$extensions": {"nalu.axes": axes, "nalu.weight": w}} for k, (f, axes, w, note) in FONT.items()},
        "size": {k: {"$type": "dimension", "$value": f"{v}px"} for k, v in SIZE.items()},
        "space": {k: {"$type": "dimension", "$value": f"{v}px"} for k, v in SPACE.items()},
        "radius": {k: {"$type": "dimension", "$value": f"{v}px"} for k, v in RADIUS.items()},
        "terminal": {
            "$description": "Compiler and generated-script output: 16-color ANSI code and the truecolor equivalent. "
                            "Respect NO_COLOR and turn color off outside a terminal; color never carries meaning alone.",
            **{k: {"$type": "color", "$value": v["truecolor"], "$extensions": {"nalu.ansi": v["ansi"]}, "$description": v["use"]}
               for k, v in TERMINAL.items()},
        },
    }
    return json.dumps(data, ensure_ascii=False, indent=2) + "\n"


# ═══════════════════════ syntax highlighting (Nalu → spans) ═══════════════════════
KEYWORDS = set(SYNTAX["keyword"][2].split())
FAILS = set(SYNTAX["fail"][2].split())
TYPES = {"str", "int", "bool", "path", "duration", "secret"}
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
NUMBER = re.compile(r"\d[\d_]*(?:ms|s|m|h|d)?\b")
PROGRAM = re.compile(r"[^\s{}\"'|<>()]+")


def tokenize(src: str) -> list[tuple[str, str]]:
    """Nalu source → [(role, text)] with roles from SYNTAX ('plain' for the rest). Used for HTML and SVG."""
    out: list[tuple[str, str]] = []
    i, L = 0, len(src)
    cmd = False          # inside `$ command` (until end of line, `catch` or a trailing `{`)
    capture = 0          # paren depth inside $( )
    first = False        # the next command word is the program

    def end_string(j: int) -> int:
        j += 1
        while j < L and src[j] not in '"\n':
            if src[j] == "\\":
                j += 2
                continue
            if src[j] == "{":
                if src.startswith("{{", j):
                    j += 2
                    continue
                j = match_brace(j) + 1
                continue
            j += 1
        return j + 1

    def match_brace(j: int) -> int:
        depth = 0
        while j < L:
            if src[j] == '"':
                j = end_string(j) - 1
            elif src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    return j
            j += 1
        return L - 1

    def dq(text: str):
        buf, j = "", 1
        out.append(("string", '"'))
        while j < len(text) - 1:
            if text[j] == "\\":
                buf += text[j:j + 2]; j += 2; continue
            if text.startswith("{{", j) or text.startswith("}}", j):
                buf += text[j:j + 2]; j += 2; continue
            if text[j] == "{":
                depth, k = 0, j
                while k < len(text):
                    if text[k] == "{": depth += 1
                    elif text[k] == "}":
                        depth -= 1
                        if depth == 0: break
                    k += 1
                if buf: out.append(("string", buf)); buf = ""
                out.append(("interp", "{")); out.extend(tokenize(text[j + 1:k])); out.append(("interp", "}"))
                j = k + 1
                continue
            buf += text[j]; j += 1
        if buf: out.append(("string", buf))
        if text.endswith('"') and len(text) > 1: out.append(("string", '"'))

    while i < L:
        ch = src[i]
        if ch == "\n":
            cmd = first = False
            out.append(("plain", ch)); i += 1; continue
        if src.startswith("//", i) and not cmd:
            j = src.find("\n", i); j = L if j < 0 else j
            out.append(("comment", src[i:j])); i = j; continue
        if ch == '"':
            j = end_string(i)
            dq(src[i:j]); i = j; first = False; continue
        if ch == "'":
            j = src.find("'", i + 1); j = L if j < 0 else j + 1
            out.append(("string", src[i:j])); i = j; first = False; continue
        if src.startswith("$(", i):
            out.append(("command", "$(")); i += 2; cmd = first = True; capture = 1; continue
        if ch == "$" and i + 1 < L and src[i + 1] == " ":
            out.append(("command", "$")); i += 1; cmd = first = True; capture = 0; continue
        if cmd:
            if ch == "{":
                nl = src.find("\n", i)
                rest = src[i + 1:nl if nl >= 0 else L]
                if rest.strip() == "":
                    cmd = False; out.append(("plain", "{")); i += 1; continue
                k = match_brace(i)
                out.append(("interp", "{")); out.extend(tokenize(src[i + 1:k])); out.append(("interp", "}"))
                i = k + 1; first = False; continue
            if capture and ch in "()":
                capture += 1 if ch == "(" else -1
                if capture == 0:
                    out.append(("command", ")")); cmd = False; i += 1; continue
            m = WORD.match(src, i)
            if m and m.group() == "catch" and src[i - 1] == " " and (i + 5 >= L or src[i + 5] in " \n"):
                out.append(("fail", "catch")); i += 5; cmd = first = False; continue
            if first and not ch.isspace():
                m = PROGRAM.match(src, i)
                if m:
                    out.append(("program", m.group())); i = m.end(); first = False; continue
            m = PROGRAM.match(src, i)
            if m:
                out.append(("plain", m.group())); i = m.end(); continue
            out.append(("plain", ch)); i += 1; continue
        m = WORD.match(src, i)
        if m and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            w = m.group()
            role = "fail" if w in FAILS else "keyword" if w in KEYWORDS else "type" if (w in TYPES or w[0].isupper()) else "plain"
            out.append((role, w)); i = m.end(); continue
        out.append(("plain", ch)); i += 1
    # junta vizinhos do mesmo papel
    merged: list[tuple[str, str]] = []
    for r, t in out:
        if merged and merged[-1][0] == r:
            merged[-1] = (r, merged[-1][1] + t)
        else:
            merged.append((r, t))
    return merged


CLASS = {"keyword": "k", "fail": "f", "string": "s", "interp": "i", "command": "x", "program": "p", "comment": "c", "type": "t"}


def highlight_html(src: str) -> str:
    return "".join(html.escape(t, quote=False) if r == "plain" else f'<span class="{CLASS[r]}">{html.escape(t, quote=False)}</span>'
                   for r, t in tokenize(src))


def excerpt(text: str, start: str, end: str) -> str:
    lines = text.splitlines()
    try:
        a = next(i for i, l in enumerate(lines) if l.startswith(start))
        b = next(i for i in range(a + 1, len(lines)) if lines[i] == end)
        return "\n".join(lines[a:b + 1])
    except StopIteration:
        return text.rstrip("\n")


RECOVERY = """\
// Fail loud by default; `catch` decides.
let user = env.get("USER") catch "unknown"
let tmp = fs.temp_dir() catch fail "no temp dir"
$ open {report} catch ignore
$ git pull catch {
    log.warn("offline: {err}")
}"""

HEALTH = """\
fn wait_healthy(url: str) {
    retry(10, every: 5s) {
        let code = http.status(url, timeout: 5s)
        if code != 200 {
            fail "{url} answered {code}"
        }
    }
    log.info("{url} is up")
}"""


# ════════════════════════════════ guide ════════════════════════════════
def esc(t: str) -> str:
    return html.escape(t, quote=True)


def construction_svg() -> str:
    """The mark on its grid: centerline, arc centers, radii, stroke."""
    grid = "".join(f'<path d="M{k} 0V64M0 {k}H64" class="g{" g8" if k % 8 == 0 else ""}"/>' for k in range(0, 65, 4))
    t = MARK["t"]
    centerline = (f'<path class="cl" d="M{n(MARK["x0"])} {n(MARK["foot"])}V{n(MARK["cy"])}'
                  + "".join(f'A{n(r)} {n(r)} 0 0 1 {n(cx + (r if i % 2 == 0 else -r))} {n(cy)}'
                            for i, (cx, cy, r) in enumerate(MARK_CENTERS)) + '"/>')
    centers = "".join(f'<circle class="c" cx="{n(cx)}" cy="{n(cy)}" r="0.9"/>' for cx, cy, _ in MARK_CENTERS)
    labels = (f'<text x="{n(MARK_CENTERS[0][0])}" y="3.6" class="lb">R21</text>'
              f'<text x="{n(MARK_CENTERS[1][0] + 6)}" y="51.6" class="lb">R14</text>'
              f'<text x="{n(MARK_CENTERS[2][0] - 1)}" y="20.6" class="lb">R8</text>'
              f'<text x="{n(MARK["x0"] - 8.5)}" y="44" class="lb">t{n(t)}</text>')
    return (f'<svg viewBox="-4 -4 72 72" class="construct" role="img" aria-label="Construction of the mark">'
            f'{grid}<path class="m" d="{MARK_D}"/>{centerline}{centers}{labels}</svg>')


def guide_html() -> str:
    L, D = theme("light"), theme("dark")
    rows = contrast_rows()
    css = tokens_css()
    tokens_block = css[css.index(":root {"):]
    sw = []
    for k, (v, d) in MATERIAL.items():
        name, _, rest = d.partition(": ")
        sw.append(f'<figure class="sw"><div class="chip" style="background:{v}"></div>'
                  f'<figcaption><b>{esc(k)}</b><code>{v}</code><span>{esc(rest or d)}</span></figcaption></figure>')
    role_rows = "".join(
        f'<tr><th>{k}</th><td><i style="background:{L[k]}"></i><code>{L[k]}</code></td>'
        f'<td><i style="background:{D[k]}"></i><code>{D[k]}</code></td><td>{esc(ROLE_NOTES.get(k, ""))}</td></tr>'
        for k in ROLES["light"])
    contrast = "".join(f'<tr class="{"bad" if r < mn else ""}"><td>{t}</td><td>{fg} / {bg}</td><td>{r:.2f}:1</td><td>{"✓" if r >= mn else "✗"} ≥ {mn:g}</td></tr>'
                       for t, fg, bg, r, mn in rows)
    term = "".join(f'<span style="color:{v["truecolor"]}">{k}</span> ' for k, v in TERMINAL.items() if k not in ("bg",))
    syn_rows = "".join(f'<tr><th><span class="{CLASS.get(k, "")}">{k}</span></th><td>{esc(role)} · {style}</td><td>{esc(what)}</td></tr>'
                       for k, (role, style, what) in SYNTAX.items())
    sample = HEALTH
    files = [
        ("nalu-lockup.svg", "Lockup, light backgrounds: the default for headers and READMEs"),
        ("nalu-lockup-inverse.svg", "Lockup, dark backgrounds"),
        ("nalu-lockup-mono.svg", "Lockup, one color (currentColor)"),
        ("nalu-mark.svg", "Mark, light backgrounds"),
        ("nalu-mark-inverse.svg", "Mark, dark backgrounds"),
        ("nalu-mark-mono.svg", "Mark, one color: stamps, editor icons, colored backgrounds"),
        ("nalu-wordmark.svg", "Wordmark alone, when the mark is already nearby"),
        ("nalu-app-icon.svg", "App icon, 512 × 512: installers, Homebrew, winget, avatars"),
        ("nalu-favicon.svg", "Favicon: follows the system theme"),
    ]
    file_rows = "".join(f'<tr><td><a href="logo/{f}"><code>logo/{f}</code></a></td><td>{esc(u)}</td></tr>' for f, u in files)
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Nalu brand</title>
<link rel="icon" href="logo/nalu-favicon.svg" type="image/svg+xml">
<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="{FONTS_URL}" rel="stylesheet">
<style>
{tokens_block}
* {{ box-sizing: border-box; }}
html {{ background: var(--bg); }}
body {{ margin: 0; color: var(--ink); background: var(--bg); font-family: var(--font-body); font-variation-settings: var(--font-body-axes);
  font-size: var(--text-md); line-height: var(--leading-body); -webkit-font-smoothing: antialiased; }}
a {{ color: var(--action); text-underline-offset: 3px; }}
a:focus-visible, button:focus-visible {{ outline: 3px solid var(--action); outline-offset: 3px; border-radius: 4px; }}
code, pre, .mono {{ font-family: var(--font-mono); font-variation-settings: var(--font-mono-axes); font-feature-settings: "liga" 0, "calt" 0; }}
.wrap {{ max-width: 1120px; margin: 0 auto; padding: 0 var(--space-5); }}
header.top {{ display: flex; align-items: center; justify-content: space-between; padding: var(--space-5) 0; }}
header.top svg {{ height: 34px; width: auto; }}
header.top nav a {{ margin-left: var(--space-5); color: var(--ink-soft); text-decoration: none; font-size: var(--text-sm); }}
header.top nav a:hover {{ color: var(--action); }}
.eyebrow {{ font-family: var(--font-label); font-variation-settings: var(--font-label-axes); font-weight: 600; font-size: var(--text-xs);
  letter-spacing: var(--tracking-label); text-transform: uppercase; color: var(--action); }}
h1, h2, h3 {{ font-family: var(--font-display); font-variation-settings: var(--font-display-axes); font-weight: var(--font-display-weight);
  letter-spacing: var(--tracking-title); line-height: var(--leading-title); margin: 0; }}
h1 {{ font-size: clamp(44px, 8vw, var(--text-5xl)); }}
h2 {{ font-size: var(--text-3xl); margin: var(--space-2) 0 var(--space-4); }}
h3 {{ font-size: var(--text-xl); margin-bottom: var(--space-2); }}
p.lede {{ font-size: var(--text-lg); color: var(--ink-soft); max-width: 62ch; }}
.hero {{ display: grid; grid-template-columns: 1.2fr 1fr; gap: var(--space-7); align-items: center; padding: var(--space-8) 0 var(--space-7); }}
.hero .art {{ background: var(--surface); border-radius: var(--radius-xl); aspect-ratio: 1; display: grid; place-items: center; position: relative; overflow: hidden; }}
.hero .art svg.big {{ width: 62%; height: auto; position: relative; }}
.swell {{ position: absolute; inset: 0; width: 100%; height: 100%; }}
.swell path {{ fill: none; stroke: var(--rule); stroke-width: 1.2; }}
section {{ padding: var(--space-8) 0; border-top: 1px solid var(--rule); }}
.readings {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: var(--space-5); margin-top: var(--space-6); }}
.card {{ background: var(--surface); border-radius: var(--radius-lg); padding: var(--space-5); }}
.card svg {{ width: 96px; height: 96px; display: block; margin-bottom: var(--space-4); }}
.card p {{ margin: 0; color: var(--ink-soft); }}
.mk {{ fill: var(--mark); }}
.hl {{ fill: var(--mark); }} .lo {{ fill: var(--rule); }}
.two {{ display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-6); align-items: start; }}
.construct {{ width: 100%; max-width: 460px; background: var(--surface); border-radius: var(--radius-lg); }}
.construct .g {{ stroke: var(--rule); stroke-width: .12; fill: none; }} .construct .g8 {{ stroke-width: .25; }}
.construct .m {{ fill: var(--mark); opacity: .18; }}
.construct .cl {{ fill: none; stroke: var(--action); stroke-width: .35; stroke-dasharray: 1.2 .8; }}
.construct .c {{ fill: var(--fail); }}
.construct .lb {{ font: 600 2.6px var(--font-mono); fill: var(--ink-soft); text-anchor: middle; }}
ul.rules {{ padding-left: 1.1em; }} ul.rules li {{ margin-bottom: var(--space-2); }}
.sizes {{ display: flex; align-items: end; gap: var(--space-5); margin-top: var(--space-4); }}
.sizes figure {{ margin: 0; text-align: center; font-size: var(--text-xs); color: var(--ink-soft); }}
.space {{ display: inline-grid; place-items: center; padding: 18px; outline: 1px dashed var(--fail); background: var(--surface); border-radius: 6px; }}
.palette {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: var(--space-4); }}
.sw {{ margin: 0; background: var(--surface); border-radius: var(--radius-lg); overflow: hidden; }}
.sw .chip {{ height: 88px; box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--ink) 14%, transparent); border-radius: var(--radius-lg) var(--radius-lg) 0 0; }}
details.contrast {{ margin-top: var(--space-6); background: var(--surface); border-radius: var(--radius-lg); padding: var(--space-4) var(--space-5); }}
details.contrast summary {{ cursor: pointer; color: var(--ink-soft); }} details.contrast summary b {{ color: var(--ok); }}
details.contrast table {{ margin-top: var(--space-4); }}
.sw figcaption {{ padding: var(--space-3) var(--space-4); display: grid; gap: 2px; font-size: var(--text-sm); }}
.sw code {{ color: var(--ink-soft); font-size: var(--text-xs); }} .sw span {{ color: var(--ink-soft); font-size: var(--text-xs); line-height: 1.4; }}
table {{ border-collapse: collapse; width: 100%; font-size: var(--text-sm); }}
th, td {{ text-align: left; padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--rule); vertical-align: top; }}
thead th {{ font-family: var(--font-label); font-variation-settings: var(--font-label-axes); font-size: var(--text-xs); letter-spacing: var(--tracking-label);
  text-transform: uppercase; color: var(--ink-soft); }}
td i {{ display: inline-block; width: 14px; height: 14px; border-radius: 4px; vertical-align: -2px; margin-right: 6px; border: 1px solid var(--rule); }}
tr.bad td {{ color: var(--fail); }}
.type .row {{ display: grid; grid-template-columns: 160px 1fr; gap: var(--space-5); padding: var(--space-5) 0; border-bottom: 1px solid var(--rule); align-items: baseline; }}
.type .row .meta {{ color: var(--ink-soft); font-size: var(--text-sm); }}
.disp {{ font-family: var(--font-display); font-variation-settings: var(--font-display-axes); font-weight: 800; font-size: var(--text-4xl); letter-spacing: var(--tracking-title); line-height: 1.05; }}
.body {{ font-size: var(--text-lg); max-width: 60ch; }}
.monos {{ font-size: var(--text-md); }}
.lab {{ font-family: var(--font-label); font-variation-settings: var(--font-label-axes); font-weight: 600; letter-spacing: var(--tracking-label); text-transform: uppercase; font-size: var(--text-sm); }}
pre.code {{ background: var(--code-bg); border-radius: var(--radius-lg); padding: var(--space-5); overflow-x: auto; font-size: var(--text-sm); line-height: 1.65; margin: 0; }}
.k, .i, .x {{ color: var(--action); }} .x {{ font-weight: 700; }} .f {{ color: var(--fail); font-weight: 700; }} .s {{ color: var(--ok); }}
.p {{ font-weight: 700; }} .c {{ color: var(--ink-soft); font-style: italic; }} .t {{ color: var(--ink-soft); }}
.term {{ white-space: pre; background: var(--term-bg); color: var(--term-fg); border-radius: var(--radius-lg); padding: var(--space-5); font-family: var(--font-mono);
  font-variation-settings: var(--font-mono-axes); font-size: var(--text-sm); line-height: 1.7; overflow-x: auto; }}
.term .e {{ color: var(--term-error); font-weight: 700; }} .term .h {{ color: var(--term-help); }} .term .d {{ color: var(--term-dim); }}
.term .b {{ color: var(--term-bold); font-weight: 700; }} .term .o {{ color: var(--term-ok); }} .term .w {{ color: var(--term-warn); }}
.dont {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: var(--space-4); }}
.dont figure {{ margin: 0; background: var(--surface); border-radius: var(--radius-lg); padding: var(--space-4); }}
.dont svg {{ width: 100%; height: 96px; }}
.dont figcaption {{ font-size: var(--text-sm); color: var(--ink-soft); margin-top: var(--space-2); }}
.dont figcaption b {{ color: var(--fail); }}
.diagram img {{ width: 100%; height: auto; border-radius: var(--radius-lg); }}
footer {{ padding: var(--space-7) 0; color: var(--ink-soft); font-size: var(--text-sm); border-top: 1px solid var(--rule); }}
@media (max-width: 820px) {{
  .hero, .two {{ grid-template-columns: 1fr; }} .readings {{ grid-template-columns: 1fr; }}
  .type .row {{ grid-template-columns: 1fr; gap: var(--space-2); }} header.top nav {{ display: none; }}
}}
@media (prefers-reduced-motion: no-preference) {{
  .swell path {{ animation: drift 14s ease-in-out infinite alternate; }}
  @keyframes drift {{ from {{ transform: translateX(-6px); }} to {{ transform: translateX(6px); }} }}
}}
</style>
</head>
<body>
<div class="wrap">
<header class="top">
  <svg viewBox="{LOCK_VB}" role="img" aria-label="Nalu"><path class="mk" d="{MARK_D}"/><g transform="translate({n(LOCK_X)} {n(LOCK_Y)}) scale({LOCK_S:.4f})"><path fill="currentColor" d="{WORD_D}"/></g></svg>
  <nav><a href="#mark">Mark</a><a href="#color">Color</a><a href="#type">Type</a><a href="#code">Code</a><a href="#art">Diagrams</a><a href="#files">Files</a></nav>
</header>

<div class="hero">
  <div>
    <div class="eyebrow">Brand &amp; design system</div>
    <h1>One stroke.<br>Every shell.</h1>
    <p class="lede"><i>Nalu</i> is Hawaiian for the wave that reaches the shore. The language takes one source to every shell; the mark is one stroke that reaches three meanings. Everything on this page is generated by <code>docs/brand/build.py</code> from one set of shapes and tokens.</p>
  </div>
  <div class="art">
    <svg class="swell" viewBox="0 0 400 400" preserveAspectRatio="none" aria-hidden="true">{"".join(f'<path d="M-20 {y} C 80 {y-14}, 120 {y+14}, 210 {y} S 340 {y-14}, 420 {y}"/>' for y in range(40, 400, 26))}</svg>
    <svg class="big" viewBox="0 0 64 64" aria-hidden="true"><path class="mk" d="{MARK_D}"/></svg>
  </div>
</div>

<section id="mark">
  <div class="eyebrow">The mark</div>
  <h2>Three readings, one path</h2>
  <p class="lede">A single filled path on a 64-unit grid. Read it as a letter, as water or as a shell, and it is still the same mark.</p>
  <div class="readings">
    <div class="card"><svg viewBox="0 0 64 64"><path class="lo" d="{MARK_D}"/><path class="hl" d="{rect(MARK['x0'] - 4.5, 31, 9, 23) + disc(MARK['x0'], 54, 4.5) + band(32.5, 31, 21, 9, 180, 360)}"/></svg>
      <h3>The n</h3><p>Stem and arch of a lowercase n: the first letter of the name and the shape that opens the wordmark.</p></div>
    <div class="card"><svg viewBox="0 0 64 64"><path class="lo" d="{MARK_D}"/><path class="hl" d="{band(32.5, 31, 21, 9, 270, 360) + band(39.5, 31, 14, 9, 0, 180) + band(33.5, 31, 8, 9, 180, 360) + disc(41.5, 31, 4.5)}"/></svg>
      <h3>The wave</h3><p>The crest pitches forward and curls into its own barrel: <i>nalu</i>, the wave that arrives.</p></div>
    <div class="card"><svg viewBox="0 0 64 64"><path class="hl" d="{MARK_D}"/></svg>
      <h3>The shell</h3><p>Turn after shrinking turn, a spiral shell: the thing every script runs in.</p></div>
  </div>
  <div class="two" style="margin-top:var(--space-7)">
    <div>{construction_svg()}</div>
    <div>
      <h3>Construction</h3>
      <ul class="rules">
        <li>Centerline arcs share the axis y = 31; radii 21 → 14 → 8, each turn smaller than the last.</li>
        <li>Stroke 9 with round caps; the wordmark uses the same round terminals.</li>
        <li>Bounding box 7–58 × 5.5–58.5: optically centered on the grid.</li>
        <li>Filled shapes only, no strokes: it renders the same in any editor and in one color.</li>
      </ul>
      <h3 style="margin-top:var(--space-5)">Space and size</h3>
      <ul class="rules"><li>Clear space: one stroke width (9 units) on every side, never less.</li>
        <li>Minimum: 16 px for the mark, 96 px wide for the lockup. The mark needs no simplified favicon.</li></ul>
      <div class="sizes">
        <figure><span class="space"><svg width="48" height="48" viewBox="0 0 64 64"><path class="mk" d="{MARK_D}"/></svg></span><figcaption>48 + clear space</figcaption></figure>
        <figure><svg width="24" height="24" viewBox="0 0 64 64"><path class="mk" d="{MARK_D}"/></svg><figcaption>24</figcaption></figure>
        <figure><svg width="16" height="16" viewBox="0 0 64 64"><path class="mk" d="{MARK_D}"/></svg><figcaption>16</figcaption></figure>
      </div>
    </div>
  </div>
</section>

<section>
  <div class="eyebrow">Wordmark &amp; lockup</div>
  <h2>n a l u, drawn from one ring</h2>
  <div class="two">
    <p class="lede">Geometric lowercase drawn for the brand, not set in a font: every letter comes from one ring (radius 15, stroke 6.5), x-height 30, ascender 44. The u is the n turned over: crest and trough. In the lockup the wordmark sits on the mark's foot at 82% of its height, two strokes away. Write <b>Nalu</b> in running text; <code>nalu</code> only for the command and code.</p>
    <div class="card" style="display:grid;gap:var(--space-5)">
      <svg viewBox="{WORD_VB}" style="width:70%;height:auto;margin:0" aria-label="nalu wordmark"><path fill="currentColor" d="{WORD_D}"/></svg>
    </div>
  </div>
</section>

<section id="color">
  <div class="eyebrow">Color</div>
  <h2>Lagoon over reef</h2>
  <p class="lede">A Hawaiian shoreline at noon: cool whites instead of sand, deep ocean ink, and one signature color, the lagoon. Every lively color has exactly one job: reef and lagoon are you acting, lehua is something that failed (or that you chose to recover), limu is what worked, ʻōlena warns.</p>
  <div class="palette">{"".join(sw)}</div>
  <h3 style="margin-top:var(--space-7)">Roles</h3>
  <table><thead><tr><th>Role</th><th>Light</th><th>Dark</th><th>Job</th></tr></thead><tbody>{role_rows}</tbody></table>
  <details class="contrast"><summary><b>Contrast:</b> all {len(rows)} text and mark pairs pass WCAG 2.x, or the build fails. Show the pairs</summary>
  <table><thead><tr><th>Theme</th><th>Pair</th><th>Ratio</th><th>WCAG</th></tr></thead><tbody>{contrast}</tbody></table></details>
</section>

<section id="type" class="type">
  <div class="eyebrow">Type</div>
  <h2>Recursive, one family for prose and code</h2>
  <p class="lede">Nalu is a language, so code is the product. Recursive spans proportional and monospaced in one superfamily: the brand, the prose and the code share letterforms. Its Casual axis gives the display a brushed warmth; the mono stays strictly linear.</p>
  <div class="row"><div class="meta">Display<br><code>CASL 0.6 · 800</code></div><div class="disp">The wave that reaches every shore</div></div>
  <div class="row"><div class="meta">Body<br><code>CASL 0 · 400</code></div><div class="body">Write a script once. Nalu checks it, then writes the Bash for Linux and macOS and the PowerShell for Windows, readable and with nothing to install.</div></div>
  <div class="row"><div class="meta">Label<br><code>MONO 1 · 600 · +12%</code></div><div class="lab">Source · Compiler · Targets</div></div>
  <div class="row"><div class="meta">Mono<br><code>MONO 1 · 400</code></div><div class="monos"><code>let tag = $(git describe --tags) catch "v0.0.0"</code></div></div>
  <p style="color:var(--ink-soft);font-size:var(--text-sm);margin-top:var(--space-4)">Scale (px): {" · ".join(f"{k} {v}" for k, v in SIZE.items())}. Fallbacks: system UI sans and mono, which is also what the README diagrams use (GitHub shows SVGs without web fonts).</p>
</section>

<section id="code">
  <div class="eyebrow">Code &amp; terminal</div>
  <h2>Color follows meaning</h2>
  <div class="two">
    <pre class="code">{highlight_html(sample)}</pre>
    <pre class="code">{highlight_html(RECOVERY)}</pre>
  </div>
  <table style="margin-top:var(--space-6)"><thead><tr><th>Role</th><th>Color · style</th><th>Covers</th></tr></thead><tbody>{syn_rows}</tbody></table>
  <div class="term" style="margin-top:var(--space-6)"><span class="e">error[E006]</span><span class="b">: `or` does not exist in Nalu</span>
  <span class="d">--&gt;</span> deploy.nalu:12:17
<span class="d">   |</span>
<span class="d">12 |</span> let ok = a == 1 or a == 2
<span class="d">   |</span>                 <span class="e">^^</span>
<span class="d">   =</span> <span class="h">help: the logical or is `||`; to handle a failure use `catch`: `x catch default`</span>
<span class="o">ok</span>: 44 cases · <span class="w">warning[W001]</span> · <span class="h">[dry-run] fs.copy</span></div>
  <p style="color:var(--ink-soft);font-size:var(--text-sm)">Terminal palette (always dark): {term}— respects <code>NO_COLOR</code>; color never carries meaning alone.</p>
</section>

<section id="art" class="diagram">
  <div class="eyebrow">Diagrams</div>
  <h2>Swell lines, not arrows</h2>
  <div class="two">
    <div><p class="lede">Diagrams are calm cards on the page background. Flow is drawn as a swell: a soft wave from source to destination, ending in a foam dot. Destinations that are generated scripts sit on the shore (sand). Labels are uppercase mono eyebrows; names are body text; code is mono.</p>
    <ul class="rules"><li>One lively color per diagram for flow (reef / lagoon); lehua only for failure paths.</li>
      <li>Every diagram exists in light and dark and in English and Portuguese, generated by <code>docs/art/build.py</code>.</li>
      <li>System fonts only (GitHub renders SVGs without web fonts): leave room for wider fallbacks.</li></ul></div>
    <picture><source media="(prefers-color-scheme: dark)" srcset="../art/en/flow-dark.svg"><img src="../art/en/flow-light.svg" alt="One source compiled to three native scripts"></picture>
  </div>
</section>

<section>
  <div class="eyebrow">Don't</div>
  <h2>Keep the stroke whole</h2>
  <div class="dont">
    <figure><svg viewBox="0 0 64 64"><g transform="rotate(-14 32 32)"><path class="mk" d="{MARK_D}"/></g></svg><figcaption><b>Don't</b> rotate or tilt it.</figcaption></figure>
    <figure><svg viewBox="0 0 64 64" preserveAspectRatio="none"><g transform="translate(-10 0) scale(1.35 1)"><path class="mk" d="{MARK_D}"/></g></svg><figcaption><b>Don't</b> stretch it.</figcaption></figure>
    <figure><svg viewBox="0 0 64 64"><path fill="var(--fail)" d="{MARK_D}"/></svg><figcaption><b>Don't</b> paint it lehua: red means failure.</figcaption></figure>
    <figure><svg viewBox="0 0 64 64"><path fill="none" stroke="var(--mark)" stroke-width="1.5" d="{MARK_D}"/></svg><figcaption><b>Don't</b> outline it.</figcaption></figure>
    <figure><svg viewBox="0 0 64 64"><defs><linearGradient id="gr" x1="0" x2="1"><stop offset="0" stop-color="{M['lagoon']}"/><stop offset="1" stop-color="{M['lehua-lt']}"/></linearGradient></defs><path fill="url(#gr)" d="{MARK_D}"/></svg><figcaption><b>Don't</b> add gradients, shadows or glows.</figcaption></figure>
    <figure><svg viewBox="0 0 64 64"><path class="mk" d="{mark_path(r2=10, r3=3.5)}"/></svg><figcaption><b>Don't</b> redraw the curl: the turns are fixed.</figcaption></figure>
  </div>
</section>

<section id="files">
  <div class="eyebrow">Files</div>
  <h2>Assets and tokens</h2>
  <table><thead><tr><th>File</th><th>Use</th></tr></thead><tbody>{file_rows}
  <tr><td><a href="tokens/tokens.css"><code>tokens/tokens.css</code></a></td><td>CSS variables: light and dark roles, syntax, terminal, type, space, radius</td></tr>
  <tr><td><a href="tokens/tokens.json"><code>tokens/tokens.json</code></a></td><td>The same tokens in W3C Design Tokens format, plus the ANSI map of the terminal</td></tr>
  <tr><td><a href="../art/"><code>docs/art/</code></a></td><td>README banner, social card, badges and diagrams (light/dark, en/pt)</td></tr></tbody></table>
</section>
</div>
<footer><div class="wrap">Nalu brand · generated by <code>docs/brand/build.py</code> · mark, wordmark and tokens are part of the project (MIT)</div></footer>
</body>
</html>
"""


def readme_md() -> str:
    rows = contrast_rows()
    bad = [r for r in rows if r[3] < r[4]]
    pal = "\n".join(f"| `{k}` | `{v}` | {d} |" for k, (v, d) in MATERIAL.items())
    return f"""# Nalu brand

> *Nalu*: Hawaiian for the wave that reaches the shore. **One stroke, every shell.**

<picture><source media="(prefers-color-scheme: dark)" srcset="logo/nalu-lockup-inverse.svg"><img src="logo/nalu-lockup.svg" alt="Nalu" width="260"></picture>

The full guide is [index.html](index.html) (open it in a browser; no server needed). Everything here, the guide included, is generated by [build.py](build.py) from one set of shapes and tokens:

```bash
python3 docs/brand/build.py      # logos, tokens, guide; fails if any contrast pair drops under WCAG
python3 docs/art/build.py        # README banner, social card, badges and diagrams
```

## The mark

One filled path on a 64 grid that reads three ways: the **n** (stem and arch), the **wave** (a crest that curls into its own barrel) and the **shell** (a spiral that shrinks every turn). Centerline arcs on the axis y = 31 with radii 21 → 14 → 8, stroke 9, round caps. Clear space: one stroke width. Minimum size: 16 px (no simplified favicon needed).

Don't rotate, stretch, outline or recolor it in lehua (red means failure), don't add gradients or shadows, and don't redraw the curl.

## Wordmark

`n a l u`, geometric lowercase drawn from one ring (radius 15, stroke 6.5), x-height 30, ascender 44, round terminals. In running text write **Nalu**; `nalu` only for the command and in code.

## Color

Lagoon over reef: cool whites, deep-ocean ink, one signature color. Each lively color has one job.

| Material | Hex | Role |
|---|---|---|
{pal}

All {len(rows)} text/background pairs the system uses pass WCAG 2.x ({'none below the minimum' if not bad else f'{len(bad)} below the minimum'}); the build fails otherwise.

## Type

[Recursive](https://www.recursive.design/) (OFL) for everything: display (Casual 0.6, 800), body (Linear, 400), labels and code (Mono). One superfamily for prose and code, because code is the product. Diagrams on GitHub fall back to system fonts.

## Files

| Path | What |
|---|---|
| `logo/nalu-lockup*.svg` | lockup: light, inverse (dark), mono |
| `logo/nalu-mark*.svg` | mark: light, inverse, mono |
| `logo/nalu-wordmark*.svg` | wordmark alone |
| `logo/nalu-app-icon.svg`, `logo/nalu-favicon.svg` | app icon (512) and theme-aware favicon |
| `tokens/tokens.css`, `tokens/tokens.json` | design tokens (CSS variables and W3C Design Tokens) |
| `../art/` | README art: banner, social card, badges, diagrams |
"""


# ════════════════════════════════ main ════════════════════════════════
def main() -> int:
    LOGO.mkdir(exist_ok=True)
    TOKENS.mkdir(exist_ok=True)
    for old in LOGO.glob("*.svg"):
        old.unlink()
    for name, content in build_svgs().items():
        (LOGO / name).write_text(content)
        print(f"logo/{name}")
    icons = ROOT / "editors" / "vscode" / "icons"
    if icons.parent.exists():
        icons.mkdir(exist_ok=True)
        for t in ("light", "dark"):
            # file icon for .nalu in the explorer: the mark, a little larger than the 64 grid's margins
            (icons / f"nalu-{t}.svg").write_text(svg("4 3 56 58", mark(theme(t)["mark"]), title="Nalu file"))
        print("editors/vscode/icons/nalu-{light,dark}.svg")
    (TOKENS / "tokens.css").write_text(tokens_css())
    (TOKENS / "tokens.json").write_text(tokens_json())
    (HERE / "index.html").write_text(guide_html())
    (HERE / "README.md").write_text(readme_md())
    print("tokens/tokens.css\ntokens/tokens.json\nindex.html\nREADME.md")

    rows = contrast_rows()
    bad = [r for r in rows if r[3] < r[4]]
    width = max(len(f"{t} {fg}/{bg}") for t, fg, bg, _, _ in rows)
    print("\ncontrast (WCAG 2.x):")
    for t, fg, bg, r, mn in rows:
        print(f"  {f'{t} {fg}/{bg}':<{width}}  {r:5.2f}:1  {'ok' if r >= mn else 'FAIL'} (min {mn:g})")
    if bad:
        print(f"\n{len(bad)} pair(s) under the minimum", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
