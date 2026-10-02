"""Guarded `dx fmt` for the frontend's rsx (see docs/architecture/rsx-fmt.md).

dx fmt 0.7 has splicing bugs: on some constructs it moves or duplicates
comments, drops tokens or never converges. This wrapper never trusts it
blindly:

  check  every file must already be a `dx fmt` fixpoint (read-only: dx runs
         on stdin, never on the file — `dx fmt --check` rewrites files).
  write  format each file, but only write the result when it is a fixpoint,
         keeps the exact comment sequence and keeps the token stream
         (modulo the token changes dx legitimately makes). Otherwise the file
         is left untouched and reported, with the construct to rewrite.

Usage: python3 scripts/rsx_fmt.py check|write [files...]  (default: src/**)
"""

import re
import subprocess
import sys
from pathlib import Path

RAW_STR = re.compile(r'(?:b|c)?r(#*)"')
NUMBER = re.compile(r"[0-9][0-9a-zA-Z_.]*")


def is_ident_char(ch):
    return ch.isalnum() or ch == "_"


def lex(src):
    """Yield (kind, text) for every token and line comment of a Rust file.
    kind: 'comment' (non-doc `//`), 'doc', 'tok'. Block comments are skipped."""
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c.isspace():
            i += 1
            continue
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            text = src[i:j].rstrip()
            doc = (text.startswith("///") and not text.startswith("////")) or text.startswith("//!")
            yield ("doc" if doc else "comment", text)
            i = j
            continue
        if src.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            continue
        if c in "bcr" and (i == 0 or not is_ident_char(src[i - 1])):
            m = RAW_STR.match(src, i)
            if m:
                end = '"' + m.group(1)
                j = src.find(end, m.end())
                j = n if j < 0 else j + len(end)
                yield ("tok", src[i:j])
                i = j
                continue
        if c == '"' or (c in "bc" and src.startswith('"', i + 1)):
            j = i + (1 if c == '"' else 2)
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            yield ("tok", src[i : j + 1])
            i = j + 1
            continue
        if c == "'":
            if i + 1 < n and src[i + 1] == "\\":
                j = src.find("'", i + 2)
                yield ("tok", src[i : j + 1])
                i = j + 1
                continue
            if i + 2 < n and src[i + 2] == "'":
                yield ("tok", src[i : i + 3])
                i += 3
                continue
            j = i + 1
            while j < n and is_ident_char(src[j]):
                j += 1
            yield ("tok", src[i:j])  # lifetime
            i = j
            continue
        if c.isdigit():
            m = NUMBER.match(src, i)
            yield ("tok", m.group(0))
            i = m.end()
            continue
        if is_ident_char(c):
            j = i
            while j < n and is_ident_char(src[j]):
                j += 1
            yield ("tok", src[i:j])
            i = j
            continue
        yield ("tok", c)
        i += 1


def comments(src):
    return [text for kind, text in lex(src) if kind == "comment"]


def tokens(src):
    """Token stream normalised for the changes dx fmt legitimately makes:
    commas, `r#` raw identifiers, `x: x` field shorthand, and `{ }` wrapped
    around a match-arm or closure body."""
    raw = [text for kind, text in lex(src) if kind != "comment"]
    out, transparent_closers, stack = [], 0, []
    for k, t in enumerate(raw):
        if t == ",":
            continue
        if t == "#" and out and out[-1] == "r" and k + 1 < len(raw) and is_ident_char(raw[k + 1][0]):
            out.pop()
            continue
        if t == "{":
            after_arrow = len(out) >= 2 and out[-2] == "=" and out[-1] == ">"
            after_closure = bool(out) and out[-1] == "|"
            stack.append(after_arrow or after_closure)
            if not stack[-1]:
                out.append(t)
            continue
        if t == "}":
            if not (stack and stack.pop()):
                out.append(t)
            continue
        if (
            len(out) >= 2
            and out[-1] == ":"
            and out[-2] == t
            and is_ident_char(t[0])
            and not (len(out) >= 3 and out[-3] == ":")
            and not (k + 1 < len(raw) and raw[k + 1] in (":", "(", "!"))
        ):
            out.pop()
            continue
        out.append(t)
    return out


def dx_fmt(src):
    """Format through stdin/stdout: dx never touches the file itself."""
    res = subprocess.run(["dx", "fmt", "-f", "-"], input=src, capture_output=True, text=True)
    if res.returncode != 0:
        raise RuntimeError(res.stderr.strip() or "dx fmt failed")
    return res.stdout


def rsx_files(args):
    if args:
        return [Path(a) for a in args]
    root = Path(__file__).resolve().parent.parent / "src"
    return sorted(p for p in root.rglob("*.rs") if "rsx!" in p.read_text())


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ("check", "write"):
        sys.exit(__doc__)
    mode, files = sys.argv[1], rsx_files(sys.argv[2:])
    bad = []
    for path in files:
        src = path.read_text()
        out = dx_fmt(src)
        if out == src:
            continue
        if mode == "check":
            bad.append((path, "not formatted (run `task fmt`)"))
            continue
        problems = []
        try:
            converges = dx_fmt(out) == out
        except RuntimeError:
            converges = False  # dx produced code it cannot parse back
        if not converges:
            problems.append("dx fmt does not converge")
        if comments(out) != comments(src):
            problems.append("dx fmt moves, drops or duplicates comments")
        if tokens(out) != tokens(src):
            problems.append("dx fmt changes code tokens")
        if problems:
            bad.append((path, "; ".join(problems) + " — file left untouched"))
        else:
            path.write_text(out)
    for path, why in bad:
        print(f"{path}: {why}")
    if bad:
        print(
            f"\n{len(bad)} file(s) need attention — rewrite the construct dx fmt "
            "mishandles (docs/architecture/rsx-fmt.md), then re-run.",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
