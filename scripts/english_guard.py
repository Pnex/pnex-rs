#!/usr/bin/env python3
"""english_guard.py — no French in code comments and code messages.

The rule (CLAUDE.md "Langues"): every comment, doc-comment and log/error
message added to the code is English. It was broken over and over, and every
catch-up pass costs a full sweep of the repo, so this guard checks ADDED
lines only (a ratchet: old debt never blocks, new debt never lands).

Modes:
  diff <base>   lines added since <base> (CI: the push/PR base)
  staged        lines added in the index
  hook          Claude Code PostToolUse hook: reads the tool JSON on stdin,
                checks the edited file's lines added vs HEAD, exit 2 + reasons
                on stderr so the agent fixes them right away
  files <p...>  whole files (audit)
  report        whole repo, counts per directory (audit of the debt left)
  selftest      built-in checks of the detector

Exemptions: paths in .english-guard-ignore (glob + reason), or a line
carrying `english-guard: allow (<reason>)` (on the line or the line above).
French that is DATA (fold tables, French keywords, fixtures, .ftl, fr:
translations of the packs) is not a comment: put it in an exempt path or
mark it.
"""

import fnmatch
import json
import os
import re
import subprocess
import sys

ROOT = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True
).stdout.strip() or "."

C_LIKE = {".rs", ".js", ".mjs", ".ts", ".tsx", ".cpp", ".cc", ".c", ".h", ".hpp", ".ino"}
HASH = {".py", ".sh", ".toml", ".yml", ".yaml", ".star", ".ini", ".cfg", ".nix"}
HASH_NAMES = {"Dockerfile", "Makefile", "Taskfile.yml", ".env.example"}

ACCENTS = re.compile(r"[àâäçéèêëîïôöùûüÿœæÀÂÇÉÈÊËÎÏÔÙÛÜŒ]")
# Words that are French and never English; two of them make a French phrase.
STOPWORDS = {
    "le", "la", "les", "des", "du", "une", "pour", "avec", "dans", "est",
    "sont", "qui", "que", "pas", "sur", "aux", "ce", "cette", "ces", "sinon",
    "jamais", "toujours", "mais", "donc", "car", "sans", "selon", "depuis",
    "lorsque", "quand", "puis", "ici", "peut", "doit", "chaque", "tous",
    "leur", "leurs", "nous", "vous", "il", "elle", "ils", "ou", "et", "si",
    "mettre", "faire", "fait", "aussi", "encore", "comme", "entre", "vers",
    "chez", "par", "au", "en", "un",
}
# Single words strong enough alone (no English homograph).
STRONG = {
    "jamais", "toujours", "sinon", "lorsque", "parce", "puisque", "cependant",
    "introuvable", "illisible", "erreur", "échec", "vide", "ligne", "fichier",
    "aucun", "aucune", "déjà", "être", "très",
}
WORD = re.compile(r"[a-zàâäçéèêëîïôöùûüÿœæ']+", re.I)
ALLOW = "english-guard: allow"

MSG_CALL = re.compile(
    r"(tracing::|toasts::|\b(info|warn|error|debug|trace|eprintln|println|print|eprint|panic|"
    r"bail|anyhow|unreachable|todo|unimplemented)!|\.expect\(|\.context\(|with_context|#\[error\(|console\."
    r"(log|warn|error|info)|throw new Error|Serial\.print|echo )"
)
STRING = re.compile(r'"((?:[^"\\]|\\.)*)"')


def looks_french(text):
    if ALLOW in text:
        return False
    raw = WORD.findall(text)
    # An accent in a lowercase word is French prose; a capitalised one is a
    # proper noun (Élodie, Bézier, France Compétences) and passes.
    if any(ACCENTS.search(w) and w[0].islower() for w in raw):
        return True
    words = [w.lower() for w in raw]
    if any(w in STRONG for w in words):
        return True
    return len({w for w in words if w in STOPWORDS}) >= 2 and len(words) >= 3


def comment_part(line, ext, name):
    """The comment of a code line, or None. Heuristic, string-blind but URL-safe."""
    s = line.strip()
    if ext in C_LIKE:
        if s.startswith(("/*", "*", "*/")):
            return s
        m = re.search(r"(?<![:\"'\\])//", line)
        return line[m.start():] if m else None
    if ext in HASH or name in HASH_NAMES:
        if s.startswith("#!") or s.startswith("#[") or s.startswith("#{"):
            return None
        if s.startswith("#"):
            return s
        m = re.search(r"\s#\s", line)
        return line[m.start():] if m else None
    return None


def message_part(line, ext):
    if ext not in C_LIKE and ext not in {".sh", ".py"}:
        return None
    if not MSG_CALL.search(line):
        return None
    lits = STRING.findall(line)
    return " ".join(lits) if lits else None


def load_ignore():
    pats = []
    path = os.path.join(ROOT, ".english-guard-ignore")
    if os.path.exists(path):
        for raw in open(path, encoding="utf-8"):
            raw = raw.split("#", 1)[0].strip()
            if raw:
                pats.append(raw)
    return pats


IGNORE = load_ignore()


def ignored(path):
    return any(fnmatch.fnmatch(path, p) for p in IGNORE)


def check_line(path, line, prev=""):
    """Reason string if the line carries French in a comment or a message."""
    if ALLOW in line or ALLOW in prev:
        return None
    name = os.path.basename(path)
    ext = os.path.splitext(name)[1]
    for kind, part in (("comment", comment_part(line, ext, name)), ("message", message_part(line, ext))):
        if part and looks_french(part):
            return kind
    return None


def added_lines(diff_args):
    """(path, lineno, text, prev) for every added line of `git diff <args>`."""
    out = subprocess.run(
        ["git", "diff", "-U1", "--no-color", "--no-ext-diff", *diff_args],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    path, n, prev = None, 0, ""
    for l in out.splitlines():
        if l.startswith("+++ "):
            path = l[6:] if l.startswith("+++ b/") else None
        elif l.startswith("@@"):
            n = int(re.search(r"\+(\d+)", l).group(1))
            prev = ""
        elif path is None:
            continue
        elif l.startswith("+"):
            yield path, n, l[1:], prev
            prev, n = l[1:], n + 1
        elif l.startswith(" "):
            prev, n = l[1:], n + 1


def whole_file(path):
    prev = ""
    try:
        with open(os.path.join(ROOT, path), encoding="utf-8") as f:
            for i, l in enumerate(f, 1):
                yield path, i, l.rstrip("\n"), prev
                prev = l
    except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
        return


def scan(rows):
    hits = []
    for path, n, text, prev in rows:
        if ignored(path):
            continue
        kind = check_line(path, text, prev)
        if kind:
            hits.append(f"{path}:{n}: French {kind}: {text.strip()[:160]}")
    return hits


def finish(hits, hint=True):
    if not hits:
        return 0
    print("\n".join(hits), file=sys.stderr)
    if hint:
        print(
            f"\nenglish_guard: {len(hits)} line(s) in French. Code comments and "
            "messages are English (CLAUDE.md). French DATA: mark the line "
            f"`{ALLOW} (<reason>)` or list the path in .english-guard-ignore.",
            file=sys.stderr,
        )
    return 1


def untracked():
    out = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    return [p for p in out.splitlines() if p]


def hook():
    try:
        data = json.load(sys.stdin)
    except json.JSONDecodeError:
        return 0
    ti = data.get("tool_input", {})
    path = ti.get("file_path") or ti.get("notebook_path")
    if not path:
        return 0
    rel = os.path.relpath(os.path.abspath(path), ROOT)
    if rel.startswith(".."):
        return 0
    if rel in untracked():
        rows = whole_file(rel)
    else:
        rows = added_lines(["HEAD", "--", rel])
    hits = scan(rows)
    if not hits:
        return 0
    print("\n".join(hits[:30]), file=sys.stderr)
    print(
        f"english_guard: translate these {len(hits)} line(s) to English now "
        f"(French data: `{ALLOW} (<reason>)`).",
        file=sys.stderr,
    )
    return 2


def report():
    files = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True).stdout.split()
    counts = {}
    for p in files:
        if ignored(p):
            continue
        n = len(scan(whole_file(p)))
        if n:
            key = "/".join(p.split("/")[:3])
            counts[key] = counts.get(key, 0) + n
    for k, v in sorted(counts.items(), key=lambda kv: -kv[1]):
        print(f"{v:6} {k}")
    print(f"{sum(counts.values()):6} total")
    return 0


def selftest():
    fr = [
        ("a.rs", "    // Ouvre le fichier avant la lecture"),
        ("a.rs", "let x = 1; // déjà fait"),
        ("a.rs", '    tracing::warn!("fichier introuvable : {p}");'),
        ("a.rs", "/// Renvoie la liste des devices pour une org"),
        ("a.py", "# on ne relance jamais ici"),
        ("a.js", '  console.warn("échec du rendu");'),
        ("a.rs", '    .expect("fichier .ftl valide");'),
        ("Taskfile.yml", "  # Construit le front pour la prod"),
    ]
    en = [
        ("a.rs", "    // Opens the file before reading it"),
        ("a.rs", 'let url = "https://example.com/le/la"; // the base URL'),
        ("a.rs", '    let s = "président de sas";'),
        ("a.rs", "#[derive(Debug)]"),
        ("a.rs", '    tracing::warn!("file not found: {p}");'),
        ("a.rs", "    // FOLD table: é → e  english-guard: allow (data)"),
        ("a.py", "#!/usr/bin/env python3"),
        ("a.rs", "    // finds Élodie under France Compétences (D447)"),
        ("a.rs", '    assert_eq!(t.subject, "Usine à Lyon");'),
        ("a.sh", "#   PNEX_EXTRA_SANS  optional extra SANs for the local leaf"),
        ("a.ftl", "# Ouvre le fichier"),
    ]
    for p, l in fr:
        assert check_line(p, l), f"missed French: {p}: {l}"
    for p, l in en:
        assert not check_line(p, l), f"false positive: {p}: {l}"
    print("selftest ok")
    return 0


def main(argv):
    mode = argv[1] if len(argv) > 1 else "staged"
    if mode == "diff":
        return finish(scan(added_lines([f"{argv[2]}...HEAD" if len(argv) > 2 else "HEAD"])))
    if mode == "staged":
        return finish(scan(added_lines(["--cached"])))
    if mode == "hook":
        return hook()
    if mode == "files":
        return finish(scan(r for p in argv[2:] for r in whole_file(p)))
    if mode == "report":
        return report()
    if mode == "selftest":
        return selftest()
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
