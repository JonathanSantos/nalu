#!/usr/bin/env python3
"""README art for Nalu: banner, social card, badges and diagrams, from the brand's shapes and tokens.

    python3 docs/art/build.py

Writes docs/art/{en,pt}/<name>-{light,dark}.svg, docs/art/badges/*.svg and docs/art/social.svg.
GitHub shows these SVGs as images: no web fonts, no scripts. Text uses system font stacks, so every
layout leaves room for wider fallbacks (DejaVu is ~10% wider than SF or Segoe). Standard library only.

Diagram language (see docs/brand/index.html#art): calm cards; flow drawn as a swell (a soft wave with two
faint echoes) ending in a foam dot; generated scripts land on the shore (sand); uppercase mono eyebrows
for structure, body text for names, mono for code. One lively color for flow; lehua only for failure.
"""
from __future__ import annotations

import math
import re
import sys
from html import escape
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "brand"))
import build as brand  # noqa: E402  (the brand's shapes and tokens)

SANS = "Recursive, ui-sans-serif, -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Helvetica Neue', Helvetica, Arial, sans-serif"
MONO = "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace"
n = brand.n


# ════════════════════════════ color ════════════════════════════
def mix(a: str, b: str, t: float) -> str:
    """a blended toward b by t (0 = a, 1 = b)."""
    pa = [int(a[i:i + 2], 16) for i in (1, 3, 5)]
    pb = [int(b[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(x + (y - x) * t):02x}" for x, y in zip(pa, pb))


def palette(theme: str) -> dict[str, str]:
    c = brand.theme(theme)
    dark = theme == "dark"
    c.update({
        "canvas": c["surface"],                                   # the diagram card
        "node": c["surface-2"] if dark else "#ffffff",
        "node-line": c["rule"],
        "hot": mix(c["action"], c["surface"], 0.86 if dark else 0.9),   # emphasized node fill
        "hot-line": mix(c["action"], c["surface"], 0.45),
        "fail-bg": mix(c["fail"], c["surface"], 0.86 if dark else 0.9),
        "swell": c["action"],
        "echo": mix(c["action"], c["surface"], 0.55),
        "code": c["code-bg"] if not dark else "#081923",
        "texture": mix(c["rule"], c["surface"], 0.15),
    })
    return c


# ════════════════════════════ text ════════════════════════════
NARROW = set("il.,:;|!'`ıjI")
SEMI = set("frt()[]{}-/\\ ")
WIDE = set("mwMW@%")


def tw(s: str, size: float, mono: bool = False, weight: int = 400) -> float:
    """Estimated rendered width, a little on the wide side (fallback fonts vary)."""
    if mono:
        return len(s) * 0.61 * size
    w = 0.0
    for ch in s:
        if ch in NARROW: w += 0.29
        elif ch in SEMI: w += 0.38
        elif ch in WIDE: w += 0.9
        elif ch.isupper() or ch.isdigit(): w += 0.66
        else: w += 0.56
    return w * size * (1.07 if weight >= 600 else 1.0)


def wrap(s: str, size: float, width: float, mono: bool = False, weight: int = 400) -> list[str]:
    words, lines, cur = s.split(" "), [], ""
    for w in words:
        test = (cur + " " + w).strip()
        if tw(test, size, mono, weight) <= width or not cur:
            cur = test
        else:
            lines.append(cur)
            cur = w
    if cur:
        lines.append(cur)
    return lines


def rich_wrap(text: str, size: float, width: float, weight: int = 400) -> list[list[tuple[bool, str]]]:
    """Wraps text with `code` spans into lines of (is_code, text) segments. Keeps punctuation glued to code
    (`catch`? stays "catch?") and only breaks at real spaces."""
    words: list[tuple[bool, str, bool]] = []          # (code, text, space before)
    pending = False
    for k, seg in enumerate(text.split("`")):
        code = k % 2 == 1
        for j, piece in enumerate(seg.split(" ")):
            if piece == "":
                pending = True
                continue
            words.append((code, piece, pending or j > 0))
            pending = False
    lines: list[list[tuple[bool, str]]] = [[]]
    cur = 0.0
    sp = tw(" ", size)
    for code, w, space in words:
        ww = tw(w, size * (0.94 if code else 1), mono=code, weight=weight)
        gap = sp if (space and lines[-1]) else 0.0
        if lines[-1] and space and cur + gap + ww > width:
            lines.append([])
            cur, gap = 0.0, 0.0
        txt = (" " if gap else "") + w
        if lines[-1] and lines[-1][-1][0] == code:
            lines[-1][-1] = (code, lines[-1][-1][1] + txt)
        elif gap and code:                 # the space before a code span stays outside it
            lines[-1].append((False, " "))
            lines[-1].append((code, w))
        else:
            lines[-1].append((code, txt))
        cur += gap + ww
    return lines


def rich_height(text: str, size: float, width: float, lh: float = 1.4) -> float:
    return len(rich_wrap(text, size, width)) * size * lh


class Svg:
    def __init__(self, w: int, h: int, title: str, desc: str = ""):
        self.w, self.h, self.title, self.desc = w, h, title, desc
        self.parts: list[str] = []

    def add(self, s: str):
        self.parts.append(s)

    def text(self, x, y, s, size=16, color="#000", weight=400, anchor="start", mono=False, ls=None, upper=False, opacity=None):
        s = s.upper() if upper else s
        attrs = [f'x="{n(x)}"', f'y="{n(y)}"', f'font-size="{n(size)}"', f'fill="{color}"', f'font-family="{MONO if mono else SANS}"']
        if weight != 400: attrs.append(f'font-weight="{weight}"')
        if anchor != "start": attrs.append(f'text-anchor="{anchor}"')
        if ls: attrs.append(f'letter-spacing="{ls}"')
        if opacity is not None: attrs.append(f'opacity="{opacity}"')
        self.add(f'<text {" ".join(attrs)}>{escape(s, quote=False)}</text>')

    def lines(self, x, y, lines, size, color, lh=1.4, **kw):
        for i, l in enumerate(lines):
            self.text(x, y + i * size * lh, l, size, color, **kw)

    def rich(self, x, y, text, size, color, code_color, width, lh=1.4, weight=400, anchor="start"):
        """Text where `backticks` mark code (mono, code color); wraps to width. Returns the number of lines."""
        lines = rich_wrap(text, size, width, weight)
        for i, segs in enumerate(lines):
            spans = "".join(
                (f'<tspan font-family="{MONO}" fill="{code_color}" font-size="{n(size * 0.94)}">{escape(t, quote=False)}</tspan>' if code
                 else f'<tspan>{escape(t, quote=False)}</tspan>') for code, t in segs)
            a = f' text-anchor="{anchor}"' if anchor != "start" else ""
            w = f' font-weight="{weight}"' if weight != 400 else ""
            self.add(f'<text x="{n(x)}" y="{n(y + i * size * lh)}" font-size="{n(size)}" fill="{color}" font-family="{SANS}"{a}{w} '
                     f'xml:space="preserve" style="white-space:pre">{spans}</text>')
        return len(lines)

    def eyebrow(self, x, y, s, color, size=12, anchor="start"):
        self.text(x, y, s, size, color, weight=700, mono=True, ls="0.12em", upper=True, anchor=anchor)

    def rect(self, x, y, w, h, fill, r=12, stroke=None, sw=1.2, dash=None, opacity=None):
        a = f'<rect x="{n(x)}" y="{n(y)}" width="{n(w)}" height="{n(h)}" rx="{n(r)}" fill="{fill}"'
        if stroke: a += f' stroke="{stroke}" stroke-width="{n(sw)}"'
        if dash: a += f' stroke-dasharray="{dash}"'
        if opacity is not None: a += f' opacity="{opacity}"'
        self.add(a + "/>")

    def code(self, x, y, src, size, c, lh=1.6):
        """Highlighted Nalu, one <text> per line, spaces preserved."""
        color = {"keyword": c["action"], "interp": c["action"], "command": c["action"], "fail": c["fail"],
                 "string": c["ok"], "comment": c["ink-soft"], "type": c["ink-soft"], "program": c["ink"], "plain": c["ink"]}
        weight = {"command": 700, "fail": 700, "program": 700}
        line_toks: list[list[tuple[str, str]]] = [[]]
        for role, t in brand.tokenize(src):
            parts = t.split("\n")
            for k, part in enumerate(parts):
                if k:
                    line_toks.append([])
                if part:
                    line_toks[-1].append((role, part))
        for i, toks in enumerate(line_toks):
            spans = "".join(
                f'<tspan fill="{color[r]}"{" font-weight=" + chr(34) + str(weight[r]) + chr(34) if r in weight else ""}'
                f'{" font-style=" + chr(34) + "italic" + chr(34) if r == "comment" else ""}>{escape(t, quote=False)}</tspan>'
                for r, t in toks)
            self.add(f'<text x="{n(x)}" y="{n(y + i * size * lh)}" font-size="{n(size)}" font-family="{MONO}" '
                     f'xml:space="preserve" style="white-space:pre">{spans}</text>')
        return len(line_toks)

    def drop_title(self, shift: float = 34):
        """The README section heading already names the diagram: remove the eyebrow title (parts[1]) and
        move everything up. parts[0] must be the canvas rect."""
        canvas = self.parts[0]
        self.h = int(self.h - shift)
        self.parts = [re.sub(r'height="[\d.]+"', f'height="{self.h}"', canvas, count=1),
                      f'<g transform="translate(0 -{n(shift)})">'] + self.parts[2:] + ["</g>"]

    def done(self) -> str:
        return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" width="{self.w}" height="{self.h}" '
                f'role="img" aria-labelledby="t d">\n<title id="t">{escape(self.title)}</title>\n<desc id="d">{escape(self.desc)}</desc>\n'
                + "\n".join(self.parts) + "\n</svg>\n")


# ════════════════════════════ shapes ════════════════════════════
def swell_points(a, b, amp=4.0, waves=1.5, steps=48):
    """A soft S-curve from a to b with a sine ripple that fades in and out (the swell)."""
    (x0, y0), (x1, y1) = a, b
    pts = []
    for i in range(steps + 1):
        t = i / steps
        e = t * t * (3 - 2 * t)                       # smoothstep
        horiz = abs(x1 - x0) >= abs(y1 - y0)
        bx = x0 + (x1 - x0) * (t if horiz else e)
        by = y0 + (y1 - y0) * (e if horiz else t)
        dx = (x1 - x0) * (1 if horiz else 6 * t * (1 - t)) or 1e-6
        dy = (y1 - y0) * (6 * t * (1 - t) if horiz else 1) or 1e-6
        ln = math.hypot(dx, dy)
        nx, ny = -dy / ln, dx / ln
        r = amp * math.sin(2 * math.pi * waves * t) * math.sin(math.pi * t)
        pts.append((bx + nx * r, by + ny * r))
    return pts


def path_of(pts):
    return "M" + " L".join(f"{n(x)} {n(y)}" for x, y in pts)


def swell(svg: Svg, a, b, c, amp=4.0, waves=1.0, echoes=True, color=None, end_dot=True, width=2.4):
    """The connector: a soft S-curve; long ones carry a gentle ripple and two faint echoes (a swell set)."""
    col = color or c["swell"]
    length = math.hypot(b[0] - a[0], b[1] - a[1])
    if length < 170:
        amp, echoes = 0, False
    horiz = abs(b[0] - a[0]) >= abs(b[1] - a[1])
    if echoes and length >= 200:
        for off in (-7, 7):
            sh = (lambda p: (p[0], p[1] + off)) if horiz else (lambda p: (p[0] + off, p[1]))
            pts = swell_points(sh(a), sh(b), amp, waves)[6:-10]
            svg.add(f'<path d="{path_of(pts)}" fill="none" stroke="{c["echo"]}" stroke-width="1.2" stroke-linecap="round" opacity=".55"/>')
    svg.add(f'<path d="{path_of(swell_points(a, b, amp, waves))}" fill="none" stroke="{col}" stroke-width="{width}" stroke-linecap="round"/>')
    if end_dot:
        svg.add(f'<circle cx="{n(b[0])}" cy="{n(b[1])}" r="4.5" fill="{col}"/>')


def texture(svg: Svg, c, x, y, w, h, gap=22, opacity=1.0):
    """Faint swell lines across an area (the brand's background texture)."""
    lines = []
    for k, yy in enumerate(range(int(y) + gap // 2, int(y + h), gap)):
        ph = k * 0.9
        pts = [(x + i * w / 60, yy + 3.2 * math.sin(i / 60 * 2 * math.pi * 2.2 + ph)) for i in range(61)]
        lines.append(f'<path d="{path_of(pts)}"/>')
    svg.add(f'<g fill="none" stroke="{c["texture"]}" stroke-width="1.1" opacity="{opacity}">{"".join(lines)}</g>')


def mark(svg: Svg, x, y, size, color):
    svg.add(f'<path transform="translate({n(x)} {n(y)}) scale({n(size / 64)})" fill="{color}" d="{brand.MARK_D}"/>')


def lockup(svg: Svg, x, y, height, ink, mark_c):
    """The lockup with its top at y and the given height (mark height ≈ 53/62 of it)."""
    s = height / 62
    svg.add(f'<g transform="translate({n(x - 3 * s)} {n(y - 1 * s)}) scale({n(s)})">'
            f'<path fill="{mark_c}" d="{brand.MARK_D}"/>'
            f'<g transform="translate({n(brand.LOCK_X)} {n(brand.LOCK_Y)}) scale({brand.LOCK_S:.4f})"><path fill="{ink}" d="{brand.WORD_D}"/></g></g>')
    return (brand.LOCK_W - 3) * s


def card(svg: Svg, c, x, y, w, h, hot=False, shore=False, fail=False, r=14):
    if shore:
        svg.rect(x, y, w, h, c["shore"], r)
    elif fail:
        svg.rect(x, y, w, h, c["fail-bg"], r, c["fail"], 1.2)
    elif hot:
        svg.rect(x, y, w, h, c["hot"], r, c["hot-line"], 1.4)
    else:
        svg.rect(x, y, w, h, c["node"], r, c["node-line"], 1.2)


def node(svg: Svg, c, x, y, w, h, eyebrow, title, body=None, hot=False, shore=False, fail=False, title_mono=False, size=17):
    card(svg, c, x, y, w, h, hot, shore, fail)
    ink = c["shore-ink"] if shore else c["ink"]
    soft = c["shore-ink"] if shore else c["ink-soft"]
    eb = c["fail"] if fail else (c["shore-ink"] if shore else c["action"])
    yy = y + 28
    if eyebrow:
        svg.eyebrow(x + 18, yy, eyebrow, eb, 11)
        yy += 26
    svg.text(x + 18, yy, title, size, ink, weight=700 if not title_mono else 600, mono=title_mono)
    if body:
        svg.rich(x + 18, yy + 24, body, 14, soft, c["shore-ink"] if shore else c["action"], w - 36, 1.42)


# ════════════════════════════ words ════════════════════════════
T = {
    "en": {
        "tagline": "One wave, every shell.",
        "sub": "Write a script once. Ship native Bash and PowerShell.",
        "sub2": "Typed · fails loud · nothing to install on the target",
        "banner_title": "Nalu: one source compiled to native Bash and PowerShell scripts",
        "targets": [("Linux", "Bash 4.3+"), ("macOS", "Bash 3.2"), ("Windows", "PowerShell 5.1+")],
        # why
        "why_title": "Why Nalu exists",
        "why_left": "Today",
        "why_left_h": "Three scripts that drift apart",
        "why_left_items": ["`install.sh`, `install.ps1` and `install.cmd`, edited by hand", "quoting traps: one space splits an argument in two",
                           "failures pass in silence: `rm -rf $DIR/` with an empty `DIR`", "nothing is typed, nothing is tested"],
        "why_right": "With Nalu",
        "why_right_h": "One source, three native scripts",
        "why_right_items": ["one `.nalu` file, checked before it runs", "`{x}` is always one argument, always quoted",
                            "every failure stops the script, unless you write `catch`", "types, tests and `--dry-run` built in"],
        "why_foot": "Nothing to install where the script runs: the output is just a `.sh` or a `.ps1`.",
        # targets
        "tg_title": "One source, every target",
        "tg_source": "SOURCE", "tg_cmd": "nalu build deploy.nalu",
        "tg_shared": "Every script gets `--help`, `--dry-run` and `--verbose`, a dependency check up front, safe quoting and the same messages.",
        "tg_cols": [
            ("deploy.linux.sh", "Bash 4.3+", ["native maps (`declare -A`)", "lists passed by reference", "`${x,,}` and `wait -n`"]),
            ("deploy.macos.sh", "Bash 3.2", ["runs on the stock `/bin/bash`", "maps and case without forks", "BSD tools (`stat -f`, `date -r`)"]),
            ("deploy.windows.ps1", "PowerShell 5.1+", ["pure ASCII + CRLF + `.cmd`", "programs run via `Process`", "TLS 1.2, no progress bar"]),
        ],
        # errors
        "er_title": "Fail loud, recover with or",
        "er_fails": "SOMETHING FAILS",
        "er_fails_body": "a `$ command` exits non-zero · `fs.read` · `.to_int()` · `xs[9]` · `env.get` · one of your functions",
        "er_q": "Is there a `catch`?",
        "er_yes": "yes", "er_no": "no",
        "er_handlers": [("catch \"value\"", "use a default"), ("catch fail \"msg\"", "fail with a better message"),
                        ("catch ignore", "keep going"), ("catch { … }", "run a block with err")],
        "er_where": "Where is it?",
        "er_out": [("IN A FUNCTION", "The function fails.", "Its caller is now the one that failed: same question."),
                   ("IN try · retry · parallel", "Handled by the block.", "`try` jumps to its `catch`; `retry` tries again; `parallel` collects every error."),
                   ("AT THE TOP", "The script stops.", "`error: message` on stderr; exits with the error's code.")],
        "er_tip": "`nalu check --explain` lists every line that can fail, and what happens if it does.",
        # anatomy
        "an_title": "A script, annotated",
        "an_notes": [
            (1, "`param` + `///` become the `--dest` flag and its `--help` text"),
            (3, "`secret`: never printed, never in argv, also read from `NALU_TOKEN`"),
            (5, "`$( )` captures the output; `catch` recovers when it fails"),
            (6, "`{x}` is always one argument, spaces and all"),
            (7, "`defer` runs on exit, on failure and on Ctrl+C"),
            (8, "`parallel` runs the tasks at once and collects every error"),
            (11, "`os` is a compile-time constant: the other branch is not even generated"),
        ],
        # pipeline
        "pl_title": "Inside the compiler",
        "pl_rows": ["FRONTEND", "ONE TREE OF DECISIONS", "BACKENDS"],
        "pl_nodes": {
            "src": ("SOURCE", ".nalu", "plus `x_test.nalu` for tests"),
            "lex": ("LEXER", "Tokens", "`$ cmd` words, `{x}` interpolation, durations"),
            "par": ("PARSER", "Syntax tree", "resilient: many errors in one pass"),
            "chk": ("CHECKER", "Types & failures", "scopes, types, which calls can fail (fixpoint), E/W codes"),
            "low": ("LOWERING", "Flatten", "A-normal form, `os` resolved, every failure gets an `OnFail`"),
            "ir":  ("IR", "Typed, flat, explicit", "Both backends read the same decisions: Bash and PowerShell agree by construction, not by QA."),
            "bash": ("BASH BACKEND", "Bash", "linux (4.3+) and macos (3.2) variants"),
            "ps":  ("POWERSHELL BACKEND", "PowerShell", "5.1+, native commands via `Process`"),
            "rt":  ("RUNTIME", "Helpers on demand", "real `.sh`/`.ps1` files; only what the script uses"),
            "tool": ("TOOLING", "check --explain", "every line that can fail; `--json` for editors, CI and AI agents"),
        },
        "pl_out": ["linux.sh", "macos.sh", "windows.ps1 + .cmd"],
    },
    "pt": {
        "tagline": "Uma onda, todas as shells.",
        "sub": "Escreva o script uma vez. Gere Bash e PowerShell nativos.",
        "sub2": "Tipada · falha alto · nada para instalar no destino",
        "banner_title": "Nalu: um fonte compilado para scripts nativos de Bash e PowerShell",
        "targets": [("Linux", "Bash 4.3+"), ("macOS", "Bash 3.2"), ("Windows", "PowerShell 5.1+")],
        "why_title": "Por que a Nalu existe",
        "why_left": "Hoje",
        "why_left_h": "Três scripts que divergem",
        "why_left_items": ["`install.sh`, `install.ps1` e `install.cmd`, editados à mão", "armadilhas de aspas: um espaço parte um argumento em dois",
                           "falhas passam em silêncio: `rm -rf $DIR/` com `DIR` vazio", "nada é tipado, nada é testado"],
        "why_right": "Com a Nalu",
        "why_right_h": "Um fonte, três scripts nativos",
        "why_right_items": ["um arquivo `.nalu`, conferido antes de rodar", "`{x}` é sempre um argumento, sempre entre aspas",
                            "toda falha para o script, a menos que você escreva `catch`", "tipos, testes e `--dry-run` de fábrica"],
        "why_foot": "Nada para instalar onde o script roda: a saída é só um `.sh` ou um `.ps1`.",
        "tg_title": "Um fonte, todos os alvos",
        "tg_source": "FONTE", "tg_cmd": "nalu build deploy.nalu",
        "tg_shared": "Todo script ganha `--help`, `--dry-run` e `--verbose`, checagem de dependências no início, aspas seguras e as mesmas mensagens.",
        "tg_cols": [
            ("deploy.linux.sh", "Bash 4.3+", ["mapas nativos (`declare -A`)", "listas por referência", "`${x,,}` e `wait -n`"]),
            ("deploy.macos.sh", "Bash 3.2", ["roda no `/bin/bash` de fábrica", "mapas e caixa sem processos", "ferramentas BSD (`stat -f`)"]),
            ("deploy.windows.ps1", "PowerShell 5.1+", ["só ASCII + CRLF + `.cmd`", "programas via `Process`", "TLS 1.2, sem barra de progresso"]),
        ],
        "er_title": "Falha alto, recupera com or",
        "er_fails": "ALGO FALHA",
        "er_fails_body": "um `$ comando` sai com código ≠ 0 · `fs.read` · `.to_int()` · `xs[9]` · `env.get` · uma função sua",
        "er_q": "Tem um `catch`?",
        "er_yes": "sim", "er_no": "não",
        "er_handlers": [("catch \"valor\"", "usa um padrão"), ("catch fail \"msg\"", "falha com mensagem melhor"),
                        ("catch ignore", "segue adiante"), ("catch { … }", "roda um bloco com err")],
        "er_where": "Onde está?",
        "er_out": [("NUMA FUNÇÃO", "A função falha.", "Quem a chamou é quem falhou agora: mesma pergunta."),
                   ("EM try · retry · parallel", "O bloco trata.", "`try` pula para o `catch`; `retry` tenta de novo; `parallel` junta os erros."),
                   ("NO TOPO", "O script para.", "`error: mensagem` no stderr; sai com o código do erro.")],
        "er_tip": "`nalu check --explain` lista cada linha que pode falhar e o que acontece se falhar.",
        "an_title": "Um script, anotado",
        "an_notes": [
            (1, "`param` + `///` viram a opção `--dest` e o texto do `--help`"),
            (3, "`secret`: nunca impresso, nunca em argv, também lido de `NALU_TOKEN`"),
            (5, "`$( )` captura a saída; `catch` recupera se falhar"),
            (6, "`{x}` é sempre um argumento, mesmo com espaços"),
            (7, "`defer` roda ao sair, em falha e no Ctrl+C"),
            (8, "`parallel` roda as tarefas juntas e junta os erros"),
            (11, "`os` é constante de compilação: o outro ramo nem é gerado"),
        ],
        "pl_title": "Por dentro do compilador",
        "pl_rows": ["FRONTEND", "UMA ÁRVORE DE DECISÕES", "BACKENDS"],
        "pl_nodes": {
            "src": ("FONTE", ".nalu", "mais o `x_test.nalu` nos testes"),
            "lex": ("LEXER", "Tokens", "palavras de `$ cmd`, `{x}`, durações"),
            "par": ("PARSER", "Árvore sintática", "resiliente: vários erros numa passada"),
            "chk": ("CHECKER", "Tipos e falhas", "escopos, tipos, o que pode falhar (ponto fixo), códigos E/W"),
            "low": ("LOWERING", "Achatar", "forma A-normal, `os` resolvido, toda falha ganha um `OnFail`"),
            "ir":  ("IR", "Tipada, plana, explícita", "Os dois backends leem as mesmas decisões: Bash e PowerShell concordam por construção, não por QA."),
            "bash": ("BACKEND BASH", "Bash", "variantes linux (4.3+) e macos (3.2)"),
            "ps":  ("BACKEND POWERSHELL", "PowerShell", "5.1+, comandos nativos via `Process`"),
            "rt":  ("RUNTIME", "Helpers sob demanda", "arquivos `.sh`/`.ps1` reais; só o que o script usa"),
            "tool": ("FERRAMENTAS", "check --explain", "cada linha que pode falhar; `--json` para editores, CI e agentes de IA"),
        },
        "pl_out": ["linux.sh", "macos.sh", "windows.ps1 + .cmd"],
    },
}

BANNER_CODE = {
    "en": """\
/// where to deploy
param target: Target = Target.dev

let tag = $(git describe --tags) catch "v0.0.0"
$ docker build -t app:{tag} .
$ docker push app:{tag} catch fail "registry down"
log.info("shipped {tag} to {target}")""",
    "pt": """\
/// onde publicar
param amb: Amb = Amb.dev

let tag = $(git describe --tags) catch "v0.0.0"
$ docker build -t app:{tag} .
$ docker push app:{tag} catch fail "registry fora"
log.info("{tag} publicada em {amb}")""",
}

ANATOMY_CODE = {
    "en": """\
/// where to install
param dest: path = "~/.local/bin"
param token: secret

let tag = $(git describe --tags) catch "v0.0.0"
$ tar -czf {dest / "app-{tag}.tgz"} dist
defer fs.rm(tmp)
parallel for s in services {
    $ docker push {s}:{tag}
}
if os == "windows" { log.info("done") }""",
    "pt": """\
/// onde instalar
param dest: path = "~/.local/bin"
param token: secret

let tag = $(git describe --tags) catch "v0.0.0"
$ tar -czf {dest / "app-{tag}.tgz"} dist
defer fs.rm(tmp)
parallel for s in servicos {
    $ docker push {s}:{tag}
}
if os == "windows" { log.info("pronto") }""",
}


# ════════════════════════════ pieces ════════════════════════════
def banner(lang: str, theme: str) -> str:
    """The thesis at README width: one .nalu, three long swells, three shores. Real code lives in the README text."""
    t, c = T[lang], palette(theme)
    W, H = 1280, 520
    s = Svg(W, H, t["banner_title"], t["sub"])
    s.rect(0, 0, W, H, c["canvas"], 28)
    texture(s, c, 620, 0, W - 620, H, gap=26)
    lockup(s, 72, 74, 72, c["ink"], c["mark"])
    head = t["tagline"].split(", ", 1)
    s.text(72, 236, head[0] + ",", 64, c["ink"], weight=800, ls="-0.025em")
    s.text(72, 304, head[1], 64, c["ink"], weight=800, ls="-0.025em")
    s.lines(72, 366, wrap(t["sub"], 24, 500), 24, c["ink-soft"], lh=1.38)
    s.eyebrow(72, 466, t["sub2"], c["action"], 15)
    # the source
    px, py, pw, ph = 640, 222, 236, 76
    s.rect(px, py, pw, ph, c["code"], 18, c["hot-line"], 1.4)
    mark(s, px + 20, py + 18, 40, c["mark"])
    s.text(px + 72, py + 46, "deploy.nalu", 21, c["ink"], mono=True, weight=700)
    # three swells to three shores
    sx, sw_, sh_ = 1036, 196, 84
    for i, (os_, sh) in enumerate(t["targets"]):
        y = 92 + i * 120
        start = (px + pw + 8, py + ph / 2 + (i - 1) * 18)
        swell(s, start, (sx - 12, y + sh_ / 2), c, amp=5, waves=1.0)
        s.rect(sx, y, sw_, sh_, c["shore"], 16)
        s.text(sx + 22, y + 37, os_, 23, c["shore-ink"], weight=700)
        s.text(sx + 22, y + 64, sh, 16, c["shore-ink"], mono=True)
    return s.done()


def social() -> str:
    c = palette("dark")
    W, H = 1280, 640
    s = Svg(W, H, "Nalu", T["en"]["sub"])
    s.rect(0, 0, W, H, c["bg"], 0)
    texture(s, c, 0, 0, W, H, gap=26, opacity=0.9)
    mark(s, 905, 58, 600, c["mark"])          # the mark at full scale, bleeding off the edge
    ws = 96 / brand.BASE                      # the wordmark alone on the left (the mark is already there)
    s.add(f'<g transform="translate(96 150) scale({n(ws)})"><path fill="{c["ink"]}" d="{brand.WORD_D}"/></g>')
    s.text(96, 380, T["en"]["tagline"], 76, c["ink"], weight=800, ls="-0.02em")
    s.text(96, 448, T["en"]["sub"], 30, c["ink-soft"])
    x = 96
    for os_, sh in T["en"]["targets"]:
        label = f"{os_} · {sh}"
        w = tw(label, 20, weight=600) + 40
        s.rect(x, 500, w, 46, c["shore"], 23)
        s.text(x + 20, 530, label, 20, c["shore-ink"], weight=600)
        x += w + 14
    return s.done()


def why(lang: str, theme: str) -> str:
    t, c = T[lang], palette(theme)
    W = 960
    lx, ly, lw, rx = 40, 80, 410, 510
    size, lh, gap = 14.5, 1.38, 14

    def items_height(items):
        return sum(rich_height(i, size, lw - 58, lh) + gap for i in items)
    ch = 92 + max(items_height(t["why_left_items"]), items_height(t["why_right_items"])) + 8
    H = int(ly + ch + 76)
    s = Svg(W, H, t["why_title"], t["why_foot"])
    s.rect(0, 0, W, H, c["canvas"], 22)
    s.eyebrow(40, 52, t["why_title"], c["action"], 13)
    for x, key, hot in ((lx, "left", False), (rx, "right", True)):
        card(s, c, x, ly, lw, ch, hot=hot)
        s.eyebrow(x + 22, ly + 34, t[f"why_{key}"], c["fail"] if not hot else c["action"], 11)
        s.text(x + 22, ly + 64, t[f"why_{key}_h"], 20, c["ink"], weight=700)
        yy = ly + 100
        for item in t[f"why_{key}_items"]:
            if hot:
                s.add(f'<path d="M{x + 23} {n(yy - 5)} l4 4 l8 -9" fill="none" stroke="{c["ok"]}" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>')
            else:
                s.add(f'<path d="M{x + 24} {n(yy - 9)} l8 8 M{x + 32} {n(yy - 9)} l-8 8" stroke="{c["fail"]}" stroke-width="2" stroke-linecap="round"/>')
            k = s.rich(x + 46, yy, item, size, c["ink"] if hot else c["ink-soft"], c["action"] if hot else c["fail"], lw - 58, lh)
            yy += k * size * lh + gap
    mid = ly + ch / 2
    s.add(f'<path d="M{lx + lw + 16} {n(mid)} h{rx - lx - lw - 34}" stroke="{c["swell"]}" stroke-width="2.4" stroke-linecap="round"/>'
          f'<path d="M{rx - 24} {n(mid - 7)} l7 7 l-7 7" fill="none" stroke="{c["swell"]}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>')
    s.rich(40, ly + ch + 44, t["why_foot"], 15, c["ink-soft"], c["action"], W - 80)
    s.drop_title()
    return s.done()


def targets(lang: str, theme: str) -> str:
    t, c = T[lang], palette(theme)
    W, H = 960, 560
    s = Svg(W, H, t["tg_title"], t["tg_shared"])
    s.rect(0, 0, W, H, c["canvas"], 22)
    s.eyebrow(40, 52, t["tg_title"], c["action"], 13)
    # source
    sx, sy, sw_, sh_ = 330, 76, 300, 92
    card(s, c, sx, sy, sw_, sh_, hot=True)
    s.eyebrow(sx + 20, sy + 30, t["tg_source"], c["action"], 11)
    mark(s, sx + sw_ - 58, sy + 18, 40, c["mark"])
    s.text(sx + 20, sy + 66, t["tg_cmd"], 15, c["ink"], mono=True, weight=600)
    # three columns on the shore
    colw, gap, top = 272, 22, 250
    x0 = (W - (3 * colw + 2 * gap)) / 2
    for i, (fname, shell, items) in enumerate(t["tg_cols"]):
        x = x0 + i * (colw + gap)
        swell(s, (sx + sw_ / 2 + (i - 1) * 70, sy + sh_ + 6), (x + colw / 2, top - 10), c, echoes=False)
        s.rect(x, top, colw, 210, c["shore"], 16)
        s.text(x + 20, top + 36, fname, 15, c["shore-ink"], mono=True, weight=700)
        s.eyebrow(x + 20, top + 62, shell, c["shore-ink"], 11)
        for k, item in enumerate(items):
            yy = top + 100 + k * 34
            s.add(f'<circle cx="{n(x + 24)}" cy="{n(yy - 5)}" r="3" fill="{c["shore-ink"]}"/>')
            s.rich(x + 36, yy, item, 14, c["shore-ink"], c["shore-ink"], colw - 52)
    s.rich(40, 506, t["tg_shared"], 14.5, c["ink-soft"], c["action"], W - 80)
    s.drop_title()
    return s.done()


def errors(lang: str, theme: str) -> str:
    t, c = T[lang], palette(theme)
    W = 960
    s_parts = []
    # medir a coluna da direita
    ww, oh_pad = 410, 62
    outs = [(eb, head, body, oh_pad + rich_height(body, 13.5, ww - 40, 1.38)) for eb, head, body in t["er_out"]]
    hy, hh, hgap = 300, 58, 12
    left_h = 4 * hh + 3 * hgap
    right_h = sum(o[3] for o in outs) + 2 * hgap
    bottom = hy + max(left_h, right_h)
    H = int(bottom + 120)
    s = Svg(W, H, t["er_title"], t["er_tip"])
    s.rect(0, 0, W, H, c["canvas"], 22)
    s.eyebrow(40, 52, t["er_title"], c["action"], 13)
    fx, fy, fw, fh = 40, 76, 880, 86
    card(s, c, fx, fy, fw, fh, fail=True)
    s.eyebrow(fx + 20, fy + 30, t["er_fails"], c["fail"], 11)
    s.rich(fx + 20, fy + 60, t["er_fails_body"], 15, c["ink"], c["fail"], fw - 40)
    qx, qy = W / 2, 222
    s.add(f'<path d="M{n(qx)} {fy + fh} V{qy - 26}" stroke="{c["rule"]}" stroke-width="2"/>')
    qw = tw(t["er_q"].replace("`", ""), 18, weight=700) + 56
    s.rect(qx - qw / 2, qy - 26, qw, 50, c["node"], 25, c["node-line"], 1.2)
    s.rich(qx, qy + 6, t["er_q"], 18, c["ink"], c["fail"], qw, weight=700, anchor="middle")
    hx, hw = 40, 410
    s.add(f'<path d="M{n(qx - qw / 2)} {qy} H{hx + hw / 2} V{hy - 10}" fill="none" stroke="{c["swell"]}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>'
          f'<circle cx="{hx + hw / 2}" cy="{hy - 10}" r="4.5" fill="{c["swell"]}"/>')
    s.text(qx - qw / 2 - 14, qy - 9, t["er_yes"], 13, c["action"], weight=700, mono=True, anchor="end")
    for i, (code, what) in enumerate(t["er_handlers"]):
        y = hy + i * (hh + hgap)
        card(s, c, hx, y, hw, hh, hot=True)
        s.text(hx + 20, y + 36, code, 16, c["fail"], weight=700, mono=True)
        s.text(hx + hw - 20, y + 36, what, 14.5, c["ink-soft"], anchor="end")
    wx = 510
    s.add(f'<path d="M{n(qx + qw / 2)} {qy} H{wx + ww / 2} V{hy - 10}" fill="none" stroke="{c["fail"]}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>'
          f'<circle cx="{wx + ww / 2}" cy="{hy - 10}" r="4.5" fill="{c["fail"]}"/>')
    s.text(qx + qw / 2 + 14, qy - 9, t["er_no"], 13, c["fail"], weight=700, mono=True)
    y = hy
    first_mid = None
    for i, (eb, head, body, oh) in enumerate(outs):
        card(s, c, wx, y, ww, oh, fail=(i == 2))
        s.eyebrow(wx + 20, y + 26, eb, c["fail"] if i == 2 else c["action"], 10.5)
        s.text(wx + 20, y + 50, head, 16, c["ink"], weight=700)
        s.rich(wx + 20, y + 72, body, 13.5, c["ink-soft"], c["action"], ww - 40, 1.38)
        if first_mid is None:
            first_mid = y + oh / 2
        y += oh + hgap
    # a function that fails makes its caller's call fail: back to the top
    lx = wx + ww + 16
    s.add(f'<path d="M{wx + ww} {n(first_mid)} H{lx} V{fy + fh / 2} H{fx + fw}" fill="none" stroke="{c["echo"]}" stroke-width="1.6" '
          f'stroke-dasharray="5 5" stroke-linecap="round" stroke-linejoin="round"/>')
    ty = bottom + 36
    s.rect(40, ty, 880, 60, c["code"], 14, c["rule"], 1)
    s.rich(64, ty + 37, t["er_tip"], 14.5, c["ink"], c["action"], 830)
    s.drop_title()
    return s.done()


def anatomy(lang: str, theme: str) -> str:
    t, c = T[lang], palette(theme)
    src = ANATOMY_CODE[lang]
    nlines = src.count("\n") + 1
    W = 960
    size, lh = 14.5, 1.75
    top = 96
    H = int(top + nlines * size * lh + 72)
    s = Svg(W, H, t["an_title"], " · ".join(x for _, x in t["an_notes"]))
    s.rect(0, 0, W, H, c["canvas"], 22)
    s.eyebrow(40, 52, t["an_title"], c["action"], 13)
    cw = 470
    s.rect(40, 72, cw, H - 112, c["code"], 16, c["rule"], 1)
    s.code(64, top + 16, src, size, c, lh=lh)
    nx = 560
    nw = W - nx - 66
    prev_bottom = 0.0
    for k, (line, note) in enumerate(t["an_notes"]):
        ly = top + 16 + (line - 1) * size * lh - 5          # the code line's middle
        y = max(ly, prev_bottom + 18)                         # notes push each other down, never overlap
        nlines = len(rich_wrap(note, 14, nw))
        if y == ly:
            s.add(f'<path d="M{40 + cw - 12} {n(ly)} H{nx - 8}" stroke="{c["rule"]}" stroke-width="1.2" stroke-dasharray="2 4"/>')
        else:
            s.add(f'<path d="M{40 + cw - 12} {n(ly)} H{nx - 30} L{nx - 8} {n(y)}" fill="none" stroke="{c["rule"]}" stroke-width="1.2" stroke-dasharray="2 4"/>')
        s.add(f'<circle cx="{nx + 6}" cy="{n(y)}" r="11" fill="{c["hot"]}" stroke="{c["hot-line"]}" stroke-width="1.2"/>')
        s.text(nx + 6, y + 4.5, str(k + 1), 12, c["action"], weight=700, anchor="middle", mono=True)
        s.rich(nx + 26, y + 5, note, 14, c["ink"], c["action"], nw, 1.3)
        prev_bottom = y + (nlines - 1) * 14 * 1.3 + 6
    s.drop_title()
    return s.done()


def pipeline(lang: str, theme: str) -> str:
    t, c = T[lang], palette(theme)
    N = t["pl_nodes"]
    W, H = 960, 820
    s = Svg(W, H, t["pl_title"], N["ir"][2])
    s.rect(0, 0, W, H, c["canvas"], 22)
    s.eyebrow(40, 52, t["pl_title"], c["action"], 13)
    # row 1: frontend (4 nodes)
    w4, gap, x0, y1, h1 = 200, 26, 40, 108, 138
    s.eyebrow(40, 92, t["pl_rows"][0], c["ink-soft"], 10.5)
    keys = ["src", "lex", "par", "chk"]
    for i, k in enumerate(keys):
        x = x0 + i * (w4 + gap)
        eb, title, body = N[k]
        node(s, c, x, y1, w4, h1, eb, title, body, hot=(k == "src"), title_mono=(k == "src"))
        if i:
            swell(s, (x - gap + 3, y1 + h1 / 2), (x - 4, y1 + h1 / 2), c, amp=0, echoes=False, width=2.2)
    # row 2: lowering → IR, tooling hangs off the checker
    y2, h2 = 316, 160
    chk_x = x0 + 3 * (w4 + gap)
    swell(s, (chk_x + w4 / 2, y1 + h1 + 4), (x0 + w4 / 2, y2 - 6), c, amp=6, waves=1.0, echoes=True)
    eb, title, body = N["low"]
    node(s, c, x0, y2, w4, h2, eb, title, body)
    irx, irw = x0 + w4 + gap, 2 * w4 + gap
    eb, title, body = N["ir"]
    card(s, c, irx, y2, irw, h2, hot=True, r=16)
    s.eyebrow(irx + 22, y2 + 30, eb, c["action"], 11)
    s.text(irx + 22, y2 + 62, title, 22, c["ink"], weight=800)
    s.lines(irx + 22, y2 + 92, wrap(body, 14.5, irw - 44), 14.5, c["ink-soft"], lh=1.42)
    swell(s, (x0 + w4 + 3, y2 + h2 / 2), (irx - 4, y2 + h2 / 2), c, amp=0, echoes=False, width=2.2)
    tx = irx + irw + gap
    eb, title, body = N["tool"]
    node(s, c, tx, y2, w4, h2, eb, title, body, title_mono=True, size=15)
    s.add(f'<path d="M{chk_x + w4 - 30} {y1 + h1} V{y2 - 6}" fill="none" stroke="{c["echo"]}" stroke-width="1.6" stroke-dasharray="5 5"/>'
          f'<circle cx="{chk_x + w4 - 30}" cy="{y2 - 6}" r="3.5" fill="{c["echo"]}"/>')
    # row 3: backends + runtime → shore
    y3 = 540
    s.eyebrow(40, 524, t["pl_rows"][2], c["ink-soft"], 10.5)
    bw, bh = 300, 104
    eb, title, body = N["bash"]
    node(s, c, 40, y3, bw, bh, eb, title, body)
    eb, title, body = N["ps"]
    node(s, c, 40, y3 + bh + 16, bw, bh, eb, title, body)
    ir_bottom = (irx + irw / 2, y2 + h2 + 4)
    swell(s, ir_bottom, (40 + bw / 2 + 60, y3 - 6), c, amp=5, waves=1.0)
    rx, rw = 40 + bw + 60, 220
    eb, title, body = N["rt"]
    node(s, c, rx, y3 + 40, rw, 144, eb, title, body)
    for k in range(2):
        yy = y3 + bh / 2 + k * (bh + 16)
        s.add(f'<path d="M{40 + bw} {n(yy)} C{40 + bw + 30} {n(yy)}, {rx - 30} {y3 + 112}, {rx - 4} {y3 + 112}" fill="none" stroke="{c["swell"]}" stroke-width="2.2"/>')
    s.add(f'<circle cx="{rx - 4}" cy="{y3 + 112}" r="4.5" fill="{c["swell"]}"/>')
    ox = rx + rw + 60
    for i, out in enumerate(t["pl_out"]):
        y = y3 + 6 + i * 72
        swell(s, (rx + rw + 4, y3 + 112), (ox - 6, y + 28), c, amp=3, waves=1.0, echoes=False)
        s.rect(ox, y, W - 40 - ox, 56, c["shore"], 14)
        s.text(ox + 20, y + 35, out, 16, c["shore-ink"], weight=700, mono=True)
    s.drop_title()
    return s.done()


BADGES = [
    ("targets", "Bash · PowerShell"),
    ("runtime", "none"),
    ("tests", "bash 3.2 · bash 5 · pwsh"),
    ("license", "MIT"),
]


def badge(label: str, value: str) -> str:
    c = palette("dark")
    size = 12
    lw = tw(label, size, mono=True) + 24
    vw = tw(value, size, weight=700) + 24
    W, H = int(lw + vw + 1), 26
    mk = 14
    s = Svg(W + mk + 6, H, f"{label}: {value}")
    s.rect(0, 0, W + mk + 6, H, M["moana"], 13)
    mark(s, 10, (H - mk) / 2, mk, M["lagoon"])
    s.text(mk + 16, 17.5, label, size, "#a9c3c9", mono=True)
    s.rect(lw + mk + 6, 2, vw - 2, H - 4, M["abyss"], 11)
    s.text(lw + mk + 6 + vw / 2 - 1, 17.5, value, size, M["lagoon"], weight=700, anchor="middle")
    return s.done()


M = brand.M
DIAGRAMS = {"banner": banner, "why": why, "targets": targets, "errors": errors, "anatomy": anatomy, "pipeline": pipeline}
# a smaller copy of the targets flow for the brand guide
ALIASES = {"flow": "targets"}


def gallery() -> str:
    """docs/art/index.html: every piece on GitHub's own backgrounds (light #ffffff, dark #0d1117), to review."""
    rows = []
    badges = "".join(f'<img src="badges/{b}.svg" alt="{b}">' for b, _ in BADGES)
    rows.append(f'<section class="dark"><h2>badges</h2>{badges}</section><section class="light">{badges}</section>')
    for lang in ("en", "pt"):
        for name in DIAGRAMS:
            for theme in ("dark", "light"):
                rows.append(f'<section class="{theme}" id="{lang}-{name}-{theme}"><h2>{lang} · {name} · {theme}</h2>'
                            f'<img src="{lang}/{name}-{theme}.svg" alt="{name}"></section>')
    rows.append('<section class="dark"><h2>social card (1280 × 640)</h2><img src="social.svg" alt="social"></section>')
    return ("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width'>"
            "<title>Nalu art</title><style>body{margin:0;font:14px ui-monospace,monospace}section{padding:24px}"
            ".dark{background:#0d1117;color:#8b949e}.light{background:#fff;color:#57606a}h2{font-size:12px;font-weight:600;margin:0 0 12px}"
            "img{max-width:830px;width:100%;display:block;margin-bottom:6px}section img[alt=targets],section img[alt=runtime],"
            "section img[alt=tests],section img[alt=license]{width:auto;display:inline-block;margin-right:8px}</style>"
            + "".join(rows) + "</html>\n")


def main() -> int:
    count = 0
    for lang in ("en", "pt"):
        out = HERE / lang
        out.mkdir(parents=True, exist_ok=True)
        for name, fn in DIAGRAMS.items():
            for theme in ("light", "dark"):
                (out / f"{name}-{theme}.svg").write_text(fn(lang, theme))
                count += 1
        for alias, target in ALIASES.items():
            for theme in ("light", "dark"):
                (out / f"{alias}-{theme}.svg").write_text(DIAGRAMS[target](lang, theme))
    (HERE / "badges").mkdir(exist_ok=True)
    for label, value in BADGES:
        (HERE / "badges" / f"{label}.svg").write_text(badge(label, value))
    (HERE / "social.svg").write_text(social())
    (HERE / "index.html").write_text(gallery())
    print(f"{count} diagrams, {len(BADGES)} badges, social.svg, index.html")
    return 0


if __name__ == "__main__":
    sys.exit(main())
