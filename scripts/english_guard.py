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
A glob followed by `[comments]` is only checked on its whole-line `#`
comments, YAML block scalars (`key: |`) skipped: packs keep their `fr:`
values and page template bodies, their comments stay English.
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

C_LIKE = {
    ".rs", ".js", ".mjs", ".ts", ".tsx", ".cpp", ".cc", ".c", ".h", ".hpp", ".ino",
    ".css", ".go", ".kt", ".java", ".swift",
}
HASH = {
    ".py", ".sh", ".toml", ".yml", ".yaml", ".star", ".ini", ".cfg", ".nix",
    ".ps1", ".template", ".conf",
}
HASH_NAMES = {"Dockerfile", "Makefile", "Taskfile.yml", ".env.example", ".gitignore", ".dockerignore"}
DASH = {".sql", ".lua"}
# Languages where single quotes and backticks delimit strings (else char literals).
QUOTE_EXT = {".js", ".mjs", ".ts", ".tsx", ".py", ".sh", ".go", ".kt", ".swift"}

ACCENTS = re.compile(r"[àâäçéèêëîïôöùûüÿœæÀÂÇÉÈÊËÎÏÔÙÛÜŒ]")
# Words that are French and never English; two of them make a French phrase.
STOPWORDS = {
    "le", "la", "les", "des", "du", "une", "pour", "avec", "dans", "est",
    "sont", "qui", "que", "pas", "sur", "aux", "ce", "cette", "ces", "sinon",
    "jamais", "toujours", "mais", "donc", "car", "sans", "selon", "depuis",
    "lorsque", "quand", "puis", "ici", "peut", "doit", "chaque", "tous",
    "leur", "leurs", "nous", "vous", "il", "elle", "ils", "ou", "et", "si",
    "mettre", "faire", "fait", "aussi", "encore", "comme", "entre", "vers",
    "chez", "par", "au", "en", "un", "de", "ne",
}
# Single words strong enough alone (no English homograph).
STRONG = {
    "jamais", "toujours", "sinon", "lorsque", "parce", "puisque", "cependant",
    "introuvable", "illisible", "erreur", "échec", "vide", "ligne", "fichier",
    "aucun", "aucune", "déjà", "être", "très",
    # Unaccented spellings (accents dropped to dodge the detector, or ASCII-only code).
    "echec", "deja", "inconnu", "inconnue", "inconnus", "invalide", "invalides",
    "manquant", "manquante", "manquants", "manquantes", "attendu", "attendue",
    "attendus", "attendues", "valeur", "valeurs", "nouvelle", "requete", "utilisateur", "utilisateurs",
}
# Whole words only: identifiers (`filiale_de`, `a_pour_site`, `x2`) are code, not prose.
WORD = re.compile(r"(?<!\w)[a-zàâäçéèêëîïôöùûüÿœæ]+(?!\w)", re.I)
# French elision (c' d' l' n' qu' j') or "impossible de": French alone.
FRENCH_MARK = re.compile(
    r"(?<![a-zàâäçéèêëîïôöùûüÿœæ])(?:c|d|l|n|qu|j)['’][aeiouyhàâäéèêëîïôöùûüœæ]"
    r"|\bimpossible d(?:e\b|['’])",
    re.I,
)
ALLOW = "english-guard: allow"

MSG_CALL = re.compile(
    r"(tracing::|toasts::|\b(info|warn|error|debug|trace|eprintln|println|print|eprint|panic|"
    r"bail|anyhow|unreachable|todo|unimplemented|format|assert|assert_eq|assert_ne)!|\.expect\(|"
    r"\.context\(|with_context|#\[error\(|\.ok_or(_else)?\(|\.map_err\(|\bErr\(|"
    r"DbErr::Custom\(|Error::string\(|Error::Message\(|\bprint\(|\bprintf\(|\bESP_LOG[EWIDV]\(|"
    r"\bfilelog\b|console\.(log|warn|error|info)|throw new Error|Serial\.print|echo )"
)
STRING = re.compile(r'"((?:[^"\\]|\\.)*)"')
CHAR_LIT = re.compile(r"'(?:\\.|[^\\'])'")
# Leading arguments of an assertion are compared values (data), not the message.
ASSERT_SKIP = {"assert!": 1, "assert_eq!": 2, "assert_ne!": 2}


def looks_french(text):
    if ALLOW in text:
        return False
    raw = WORD.findall(text)
    # An accent in a lowercase word is French prose; a capitalised one is a
    # proper noun (Élodie, Bézier, France Compétences) and passes.
    if any(ACCENTS.search(w) and w[0].islower() for w in raw):
        return True
    words = [w.lower() for w in raw]
    if any(w in STRONG for w in words) or FRENCH_MARK.search(text):
        return True
    return len({w for w in words if w in STOPWORDS}) >= 2 and len(words) >= 3


def comment_part(line, ext, name):
    """The comment of a code line, or None. Heuristic, string-blind but URL-safe."""
    s = line.strip()
    if ext in C_LIKE:
        if s.startswith(("/*", "*", "*/")):
            return s
        if ext == ".css":  # no // comments in CSS (and base64 data has //)
            m = re.search(r"/\*", line)
            return line[m.start():] if m else None
        m = re.search(r"(?<![:\"'\\])//|(?<![:\"'\\*/])/\*", line)
        return line[m.start():] if m else None
    if ext == ".html":
        m = re.search(r"<!--|(?<![:\"'\\])//|(?<![:\"'\\*/])/\*", line)
        return line[m.start():] if m else None
    if ext in DASH:
        if s.startswith("--"):
            return s
        m = re.search(r"\s--\s", line)
        return line[m.start():] if m else None
    if ext in HASH or name in HASH_NAMES:
        if s.startswith("#!") or s.startswith("#[") or s.startswith("#{"):
            return None
        if s.startswith("#"):
            return s
        m = re.search(r"\s#\s", line)
        return line[m.start():] if m else None
    return None


def message_part(line, ext, call=None):
    """(string literals of message calls, open call state). A call left open
    at the end of the line ([depth, arg index, args to skip]) carries over, so
    a message on the next line of a multi-line call is checked too."""
    if ext not in C_LIKE and ext not in {".sh", ".py"}:
        return None, None
    quotes = "\"'`" if ext in QUOTE_EXT else '"'
    out, i, n = [], 0, len(line)
    while True:
        if call is None:
            m = MSG_CALL.search(line, i)
            if not m:
                break
            j = line.find("(", m.start())
            if m.group(0) == "echo " or j < 0:
                out += STRING.findall(line[m.end():])
                break
            call, i = [1, 0, ASSERT_SKIP.get(m.group(0), 0)], j + 1
        depth, arg, skip = call
        while i < n and depth > 0:
            c = line[i]
            if c in quotes:
                k = i + 1
                while k < n and line[k] != c:
                    k += 2 if line[k] == "\\" else 1
                if arg >= skip:
                    out.append(line[i + 1:k])
                i = k + 1
                continue
            if c == "'":
                lit = CHAR_LIT.match(line, i)
                i = lit.end() if lit else i + 1
                continue
            if ext in C_LIKE and line.startswith("//", i):
                i = n
                break
            if c in "([{":
                depth += 1
            elif c in ")]}":
                depth -= 1
            elif c == "," and depth == 1:
                arg += 1
            i += 1
        if depth > 0:
            call = [depth, arg, skip]
            break
        call = None
    return (" ".join(out) or None), call


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
COMMENTS_ONLY = "[comments]"


def mode_of(path):
    """None (checked), "skip" (exempt) or "comments" (whole-line comments only)."""
    for p in IGNORE:
        glob, _, flag = p.partition(" ")
        if fnmatch.fnmatch(path, glob):
            return "comments" if flag.strip() == COMMENTS_ONLY else "skip"
    return None


def ignored(path):
    return mode_of(path) == "skip"


BLOCK_SCALAR = re.compile(r":\s*[|>][-+0-9]*\s*(#.*)?$")


class FileState:
    """Per-file scanner: carries an open message call and a YAML block scalar
    from one line to the next (reset on a gap between line numbers)."""

    def __init__(self, path):
        self.name = os.path.basename(path)
        self.ext = os.path.splitext(self.name)[1]
        self.path, self.mode = path, mode_of(path)
        self.call, self.block, self.last = None, None, None

    def check(self, n, line, prev=""):
        """Reason string if the line carries French in a comment or a message."""
        if self.last is not None and n != self.last + 1:
            self.call, self.block = None, None
        self.last = n
        allow = ALLOW in line or ALLOW in prev
        if self.mode == "comments":
            return None if allow else self.yaml_comment(line)
        msg, self.call = message_part(line, self.ext, self.call)
        if allow:
            return None
        for kind, part in (("comment", comment_part(line, self.ext, self.name)), ("message", msg)):
            if part and looks_french(part):
                return kind
        return None

    def yaml_comment(self, line):
        indent = len(line) - len(line.lstrip())
        if self.block is not None:
            if not line.strip() or indent > self.block:
                return None
            self.block = None
        if BLOCK_SCALAR.search(line):
            self.block = indent
        s = line.strip()
        return "comment" if s.startswith("#") and looks_french(s) else None


def check_line(path, line, prev=""):
    return FileState(path).check(1, line, prev)


def added_lines(diff_args):
    """(path, lineno, text, prev, added) for every added and context line of
    `git diff <args>` (context lines only feed the scanner state)."""
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
            yield path, n, l[1:], prev, True
            prev, n = l[1:], n + 1
        elif l.startswith(" "):
            yield path, n, l[1:], prev, False
            prev, n = l[1:], n + 1


def whole_file(path):
    prev = ""
    try:
        with open(os.path.join(ROOT, path), encoding="utf-8") as f:
            for i, l in enumerate(f, 1):
                yield path, i, l.rstrip("\n"), prev, True
                prev = l
    except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
        return


def scan(rows):
    hits, state = [], None
    for path, n, text, prev, added in rows:
        if state is None or state.path != path:
            state = FileState(path)
        if state.mode == "skip":
            continue
        kind = state.check(n, text, prev)
        if kind and added:
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
    out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, text=True).stdout
    files = [p for p in out.split("\0") if p]
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
        # unaccented STRONG words, alone or in a two-word message
        ("a.rs", '    .map_err(|e| format!("settings.rauthy invalide : {e}"))'),
        ("a.rs", '    .ok_or_else(|| "langage inconnu".to_string())?;'),
        ("a.rs", '    DbErr::Custom("latitude invalide".into())'),
        ("a.rs", '    Err(Error::string(&format!("conversion_type inconnu : {o}")))'),
        ("a.cpp", '    Serial.println(ok ? " OK" : " ECHEC");'),
        ("a.cpp", '    ESP_LOGE(TAG, "valeur manquante");'),
        ("a.rs", '    loco_rs::Error::Message("config manquante".into())'),
        ("a.rs", '    assert!(ok, "deja vu");'),
        ("a.rs", '    assert_eq!(a, b, "nouvelle valeur");'),
        ("a.rs", '    filelog::log("upload: requete utilisateur");'),
        ("a.rs", '    // impossible de lire'),
        # elisions
        ("a.rs", "    // l'ordre est garanti"),
        ("a.rs", "    // qu'on garde"),
        ("a.js", "    // n'importe quel item"),
        # de / ne are stopwords now
        ("a.rs", "    // pas de cache ici"),
        # trailing block comment, new extensions
        ("a.js", "  } catch (e) { /* lng/lat invalide : item suivant */ }"),
        ("a.css", "  color: red; /* couleur de la barre */"),
        (".gitignore", "# Outillage local (caches générés)"),
        ("a.sql", "-- table des utilisateurs"),
        ("a.lua", "local x = 1 -- valeur par défaut"),
        ("a.conf", "# fichier de config"),
        ("a.html", "  <!-- le bouton de connexion -->"),
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
        ("a.rs", '    assert_eq!(s, "valeur de base");'),
        ("a.rs", "    // don't use it, it's the user's own"),
        ("a.rs", "fn f<'a>(x: &'a str) -> char { 'l' }"),
        ("a.rs", '    let glob = "src/**/*.rs"; // every source'),
        ("a.rs", '    format!("{}-{}", lang, region)'),
        ("a.rs", "    /// API path of version n's bytes"),
        ("a.rs", "//! (`filiale_de.etat = a_valider`, `a_pour_site.statut = x`) become"),
        ("a.css", "src:url(data:font/woff2;base64,d09G//l'a+/x)"),
        ("a.rs", '    .map_err(|e| format!("invalid settings: {e}"))'),
        ("a.rs", '    let c = \'"\'; info!("quote {c} seen");'),
    ]
    for p, l in fr:
        assert check_line(p, l), f"missed French: {p}: {l}"
    for p, l in en:
        assert not check_line(p, l), f"false positive: {p}: {l}"

    def rows(path, lines):
        return [(path, i, l, "", True) for i, l in enumerate(lines, 1)]

    # multi-line message call: the string on the next line is checked
    assert scan(rows("a.rs", ["    tracing::warn!(", '        "fichier absent",', "    );"]))
    assert scan(rows("a.rs", ["    return Err(Error::string(", '        "valeur inconnue",', "    ));"]))
    # the call closes: a later plain string is not a message
    assert not scan(rows("a.rs", ["    warn!(", '        "{}", x', "    );", '    let s = "valeur de base";']))
    # multi-line assert: compared values skipped, the message checked
    assert not scan(rows("a.rs", ["    assert_eq!(", "        t.subject,", '        "Usine à Lyon",', "    );"]))
    assert scan(rows("a.rs", ["    assert_eq!(", "        a,", "        b,", '        "valeur attendue"', "    );"]))
    # [comments] mode (packs): comments checked, block scalars and fr: skipped
    global IGNORE
    saved, IGNORE = IGNORE, ["p/* [comments]"]
    try:
        assert scan(rows("p/x.yaml", ["# Pack des entreprises françaises"]))
        assert not scan(rows("p/x.yaml", ["  fr: Entité juridique de la société"]))
        assert not scan(rows("p/x.yaml", [
            "body: |", "  # Fiche de l'entreprise", "  Les dirigeants de la société", "next: 1",
        ]))
        assert scan(rows("p/x.yaml", ["body: |", "  # Titre", "# Commentaire de la clé suivante"]))
        assert not scan(rows("p/x.yaml", ["# Country pack of the companies"]))
    finally:
        IGNORE = saved
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
