//! Évaluateur d'expressions du nœud `calc` (Phase 6 ETL) — équivalent Rust
//! du `safe_eval` Django (docs/phase0/etl-es-metrics.md §2, contrat :
//! whitelist opérateurs/fonctions/constantes, ternaire, rejet de tout le
//! reste). Pur, wasm-safe, **zéro dépendance**, jamais de panic : toute
//! erreur remonte en [`CalcError`] (message en français affiché tel quel,
//! règle PRD « violations jamais traduites »).
//!
//! Même fonction utilisée côté éditeur (validation live dans le navigateur,
//! via pnex-core wasm) et côté runtime (nœud `pnex-calc`) — l'expression
//! validée à la sauvegarde est exactement celle évaluée en exécution.
//!
//! Langage : nombres, variables (identifiants `[A-Za-z_][A-Za-z0-9_]*`),
//! `+ - * / %` , puissance `^` (droite-associative, plus prioritaire que le
//! moins unaire : `-2^2 = -4`), comparaisons `== != < <= > >=` (→ 1/0),
//! `&& ||` (logique sur != 0), ternaire `cond ? a : b`, constantes `pi`/`e`,
//! fonctions `abs round floor ceil sqrt pow min max log log10 log2 exp sin
//! cos tan asin acos atan atan2` (min/max variadiques ≥ 1, round(x[,n]),
//! pow/atan2 binaires).

use std::collections::HashMap;
use std::fmt;

/// Catégorie d'erreur calc — permet à l'éditeur de distinguer « à corriger
/// maintenant » (lexical/syntaxe/fonction) d'« exécution » (variable absente
/// du payload, division par zéro…).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcErrorKind {
    Lexical,
    Syntax,
    UnknownFunction,
    Arity,
    UnknownVariable,
    DivisionByZero,
    MathRange,
}

/// Erreur calc : position en octets dans l'expression (0 quand non
/// localisable — ex. variable absente à l'exécution) + message français.
#[derive(Debug, Clone, PartialEq)]
pub struct CalcError {
    pub kind: CalcErrorKind,
    pub pos: usize,
    pub message: String,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pos > 0 {
            write!(f, "{} (position {})", self.message, self.pos)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl CalcError {
    fn new(kind: CalcErrorKind, pos: usize, message: impl Into<String>) -> Self {
        Self { kind, pos, message: message.into() }
    }
}

/// Résultat d'évaluation non fini (NaN/infini) — débordements inclus.
fn finite(x: f64) -> Result<f64, CalcError> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err(CalcError::new(
            CalcErrorKind::MathRange,
            0,
            "résultat hors domaine (infini ou NaN)",
        ))
    }
}

// ─────────────────────────────── Analyse lexicale ───────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Comma,
    Question,
    Colon,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
}

#[derive(Clone)]
struct Token {
    tok: Tok,
    pos: usize,
}

fn lex(src: &str) -> Result<Vec<Token>, CalcError> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let (pos, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Nombre : `12`, `12.5`, `.5` (héritage Python).
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            let mut seen_dot = false;
            while i < chars.len() {
                let (_, ch) = chars[i];
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            let text: String = chars[start..i].iter().map(|(_, ch)| ch).collect();
            let value: f64 = text.parse().map_err(|_| {
                CalcError::new(CalcErrorKind::Lexical, pos, format!("nombre invalide « {text} »"))
            })?;
            out.push(Token { tok: Tok::Num(value), pos: start });
            continue;
        }
        // Identifiant.
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() {
                let (_, ch) = chars[i];
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            let text: String = chars[start..i].iter().map(|(_, ch)| ch).collect();
            out.push(Token { tok: Tok::Ident(text), pos: start });
            continue;
        }
        // Opérateurs multi-caractères puis single-caractère.
        let (tok, len) = match (c, chars.get(i + 1).map(|(_, ch)| *ch)) {
            ('&', Some('&')) => (Tok::AndAnd, 2),
            ('|', Some('|')) => (Tok::OrOr, 2),
            ('=', Some('=')) => (Tok::Eq, 2),
            ('!', Some('=')) => (Tok::Ne, 2),
            ('<', Some('=')) => (Tok::Le, 2),
            ('>', Some('=')) => (Tok::Ge, 2),
            ('+', _) => (Tok::Plus, 1),
            ('-', _) => (Tok::Minus, 1),
            ('*', _) => (Tok::Star, 1),
            ('/', _) => (Tok::Slash, 1),
            ('%', _) => (Tok::Percent, 1),
            ('^', _) => (Tok::Caret, 1),
            ('(', _) => (Tok::LParen, 1),
            (')', _) => (Tok::RParen, 1),
            (',', _) => (Tok::Comma, 1),
            ('?', _) => (Tok::Question, 1),
            (':', _) => (Tok::Colon, 1),
            ('<', _) => (Tok::Lt, 1),
            ('>', _) => (Tok::Gt, 1),
            (other, _) => {
                return Err(CalcError::new(
                    CalcErrorKind::Lexical,
                    pos,
                    format!("caractère inattendu « {other} »"),
                ));
            }
        };
        out.push(Token { tok, pos });
        i += len;
    }
    Ok(out)
}

// ─────────────────────────────────── AST ───────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    EqCmp,
    NeCmp,
    LtCmp,
    LeCmp,
    GtCmp,
    GeCmp,
    And,
    Or,
}

#[derive(Debug, Clone)]
enum Expr {
    Num(f64),
    /// Variable + position (pour l'erreur « variable inconnue » localisée).
    Var(String, usize),
    Neg(Box<Expr>),
    Binary { op: BinOp, left: Box<Expr>, right: Box<Expr>, pos: usize },
    Call { name: String, args: Vec<Expr>, pos: usize },
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

// ─────────────────────────────────── Parsing ───────────────────────────────────

struct Parser {
    toks: Vec<Token>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|t| &t.tok)
    }

    fn peek_pos(&self) -> usize {
        self.toks.get(self.i).map(|t| t.pos).unwrap_or(0)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.toks.get(self.i).cloned();
        if t.is_some() {
            self.i += 1;
        }
        t
    }

    fn expect(&mut self, tok: Tok, what: &str) -> Result<(), CalcError> {
        match self.bump() {
            Some(t) if t.tok == tok => Ok(()),
            Some(t) => Err(CalcError::new(
                CalcErrorKind::Syntax,
                t.pos,
                format!("{what} attendu, reçu « {} »", describe(&t.tok)),
            )),
            None => Err(CalcError::new(
                CalcErrorKind::Syntax,
                self.peek_pos(),
                format!("{what} manquant en fin d'expression"),
            )),
        }
    }

    /// Ternaire (branche haute) — `cond ? a : b`, branches recursives.
    fn parse_expr(&mut self) -> Result<Expr, CalcError> {
        let cond = self.parse_binary(0)?;
        if self.peek() == Some(&Tok::Question) {
            self.bump();
            let then = self.parse_expr()?;
            self.expect(Tok::Colon, "« : » du ternaire")?;
            let els = self.parse_expr()?; // droite-associatif
            return Ok(Expr::Ternary(Box::new(cond), Box::new(then), Box::new(els)));
        }
        Ok(cond)
    }

    /// Grimpée de précédence des binaires (+ court-circuit &&/|| sémantique
    /// en évaluation uniquement — le parseur est aveugle à la valeur).
    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, CalcError> {
        let mut left = self.parse_unary()?;
        while let Some((prec, op)) = self.peek().and_then(binary_op) {
            if prec < min_prec {
                break;
            }
            self.bump();
            let pos = self.toks[self.i - 1].pos;
            let right = self.parse_binary(prec + 1)?;
            left = Expr::Binary { op, left: Box::new(left), right: Box::new(right), pos };
        }
        Ok(left)
    }

    /// Moins unaire — plus faible que `^` : `-2^2 = -4` (convention maths).
    fn parse_unary(&mut self) -> Result<Expr, CalcError> {
        if self.peek() == Some(&Tok::Minus) {
            self.bump();
            let inner = self.parse_unary()?;
            return Ok(Expr::Neg(Box::new(inner)));
        }
        if self.peek() == Some(&Tok::Plus) {
            self.bump();
            return self.parse_unary();
        }
        self.parse_pow()
    }

    /// Puissance droite-associative : `2^3^2 = 2^(3^2)`, exposant repasse
    /// par unaire pour accepter `2^-3`.
    fn parse_pow(&mut self) -> Result<Expr, CalcError> {
        let base = self.parse_primary()?;
        if self.peek() == Some(&Tok::Caret) {
            self.bump();
            let pos = self.toks[self.i - 1].pos;
            let exp = self.parse_unary()?;
            return Ok(Expr::Binary { op: BinOp::Pow, left: Box::new(base), right: Box::new(exp), pos });
        }
        Ok(base)
    }

    fn parse_primary(&mut self) -> Result<Expr, CalcError> {
        let Some(t) = self.bump() else {
            return Err(CalcError::new(
                CalcErrorKind::Syntax,
                self.peek_pos(),
                "opérande manquant en fin d'expression",
            ));
        };
        match t.tok {
            Tok::Num(v) => Ok(Expr::Num(v)),
            Tok::Ident(name) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.bump();
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.peek() == Some(&Tok::Comma) {
                                self.bump();
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(Tok::RParen, "parenthèse fermante")?;
                    return Ok(Expr::Call { name, args, pos: t.pos });
                }
                // Constantes connues → valeurs ; sinon variable.
                match name.as_str() {
                    "pi" => Ok(Expr::Num(std::f64::consts::PI)),
                    "e" => Ok(Expr::Num(std::f64::consts::E)),
                    _ => Ok(Expr::Var(name, t.pos)),
                }
            }
            Tok::LParen => {
                let inner = self.parse_expr()?;
                self.expect(Tok::RParen, "parenthèse fermante")?;
                Ok(inner)
            }
            other => Err(CalcError::new(
                CalcErrorKind::Syntax,
                t.pos,
                format!("opérande inattendu « {} »", describe(&other)),
            )),
        }
    }
}

fn binary_op(tok: &Tok) -> Option<(u8, BinOp)> {
    match tok {
        Tok::OrOr => Some((1, BinOp::Or)),
        Tok::AndAnd => Some((2, BinOp::And)),
        Tok::Eq => Some((3, BinOp::EqCmp)),
        Tok::Ne => Some((3, BinOp::NeCmp)),
        Tok::Lt => Some((3, BinOp::LtCmp)),
        Tok::Le => Some((3, BinOp::LeCmp)),
        Tok::Gt => Some((3, BinOp::GtCmp)),
        Tok::Ge => Some((3, BinOp::GeCmp)),
        Tok::Plus => Some((4, BinOp::Add)),
        Tok::Minus => Some((4, BinOp::Sub)),
        Tok::Star => Some((5, BinOp::Mul)),
        Tok::Slash => Some((5, BinOp::Div)),
        Tok::Percent => Some((5, BinOp::Rem)),
        _ => None,
    }
}

fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Num(_) => "nombre".into(),
        Tok::Ident(_) => "identifiant".into(),
        Tok::Plus => "+".into(),
        Tok::Minus => "-".into(),
        Tok::Star => "*".into(),
        Tok::Slash => "/".into(),
        Tok::Percent => "%".into(),
        Tok::Caret => "^".into(),
        Tok::LParen => "(".into(),
        Tok::RParen => ")".into(),
        Tok::Comma => ",".into(),
        Tok::Question => "?".into(),
        Tok::Colon => ":".into(),
        Tok::Eq => "==".into(),
        Tok::Ne => "!=".into(),
        Tok::Lt => "<".into(),
        Tok::Le => "<=".into(),
        Tok::Gt => ">".into(),
        Tok::Ge => ">=".into(),
        Tok::AndAnd => "&&".into(),
        Tok::OrOr => "||".into(),
    }
}

// ─────────────────────── Analyse statique (validation) ───────────────────────

/// Signature d'une fonction : (nom, arity min, arity max) — max None =
/// variadique. `round(x[,n])` 1..=2 ; `pow`/`atan2` 2 ; min/max ≥ 1.
const FUNCS: &[(&str, usize, Option<usize>)] = &[
    ("abs", 1, Some(1)),
    ("round", 1, Some(2)),
    ("floor", 1, Some(1)),
    ("ceil", 1, Some(1)),
    ("sqrt", 1, Some(1)),
    ("pow", 2, Some(2)),
    ("min", 1, None),
    ("max", 1, None),
    ("log", 1, Some(1)),
    ("log10", 1, Some(1)),
    ("log2", 1, Some(1)),
    ("exp", 1, Some(1)),
    ("sin", 1, Some(1)),
    ("cos", 1, Some(1)),
    ("tan", 1, Some(1)),
    ("asin", 1, Some(1)),
    ("acos", 1, Some(1)),
    ("atan", 1, Some(1)),
    ("atan2", 2, Some(2)),
];

/// Erreurs **statiques** de l'expression (sans connaître les variables) —
/// toutes collectées, pas seulement la première : l'éditeur surligne tout.
fn analyze(expr: &Expr, out: &mut Vec<CalcError>) {
    match expr {
        Expr::Num(_) | Expr::Var(..) => {}
        Expr::Neg(inner) => analyze(inner, out),
        Expr::Ternary(c, a, b) => {
            analyze(c, out);
            analyze(a, out);
            analyze(b, out);
        }
        Expr::Binary { op: _, left, right, .. } => {
            analyze(left, out);
            analyze(right, out);
        }
        Expr::Call { name, args, pos } => {
            match FUNCS.iter().find(|(n, _, _)| *n == name.as_str()) {
                None => out.push(CalcError::new(
                    CalcErrorKind::UnknownFunction,
                    *pos,
                    format!("fonction inconnue « {name} »"),
                )),
                Some((_, min, max)) => {
                    let (min, max) = (*min, *max);
                    let ok = args.len() >= min && max.map(|m| args.len() <= m).unwrap_or(true);
                    if !ok {
                        let attendu = match max {
                            Some(m) if m == min => format!("{min} argument(s) attendu(s)"),
                            Some(m) => format!("{min} à {m} arguments attendus"),
                            None => format!("au moins {min} argument(s) attendu(s)"),
                        };
                        out.push(CalcError::new(
                            CalcErrorKind::Arity,
                            *pos,
                            format!("fonction « {name} » : {attendu}, reçu {}", args.len()),
                        ));
                    }
                }
            }
            for a in args {
                analyze(a, out);
            }
        }
    }
}

// ─────────────────────────────── Évaluation ───────────────────────────────

fn truthy(x: f64) -> bool {
    x != 0.0
}

fn eval(expr: &Expr, vars: &HashMap<String, f64>) -> Result<f64, CalcError> {
    match expr {
        Expr::Num(v) => Ok(*v),
        Expr::Var(name, pos) => vars.get(name).copied().ok_or_else(|| {
            CalcError::new(
                CalcErrorKind::UnknownVariable,
                *pos,
                format!("variable inconnue « {name} » (clé absente du payload device)"),
            )
        }),
        Expr::Neg(inner) => eval(inner, vars).map(|v| -v),
        Expr::Ternary(c, a, b) => {
            if truthy(eval(c, vars)?) {
                eval(a, vars)
            } else {
                eval(b, vars)
            }
        }
        Expr::Binary { op, left, right, pos } => {
            let l = eval(left, vars)?;
            let r = eval(right, vars)?;
            let v = match op {
                BinOp::Add => l + r,
                BinOp::Sub => l - r,
                BinOp::Mul => l * r,
                BinOp::Div => {
                    if r == 0.0 {
                        return Err(CalcError::new(CalcErrorKind::DivisionByZero, *pos, "division par zéro"));
                    }
                    l / r
                }
                BinOp::Rem => {
                    if r == 0.0 {
                        return Err(CalcError::new(CalcErrorKind::DivisionByZero, *pos, "modulo par zéro"));
                    }
                    l % r
                }
                BinOp::Pow => l.powf(r),
                BinOp::EqCmp => bool_to_f64(l == r),
                BinOp::NeCmp => bool_to_f64(l != r),
                BinOp::LtCmp => bool_to_f64(l < r),
                BinOp::LeCmp => bool_to_f64(l <= r),
                BinOp::GtCmp => bool_to_f64(l > r),
                BinOp::GeCmp => bool_to_f64(l >= r),
                BinOp::And => bool_to_f64(truthy(l) && truthy(r)),
                BinOp::Or => bool_to_f64(truthy(l) || truthy(r)),
            };
            finite(v)
        }
        Expr::Call { name, args, pos } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval(a, vars)?);
            }
            call(name, &vals, *pos)
        }
    }
}

fn bool_to_f64(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn call(name: &str, args: &[f64], pos: usize) -> Result<f64, CalcError> {
    let x = args.first().copied().unwrap_or(f64::NAN);
    let v = match (name, args.len()) {
        ("abs", 1) => x.abs(),
        ("round", 1) => x.round(),
        ("round", 2) => {
            let n = args[1];
            let mult = 10f64.powi(n as i32);
            (x * mult).round() / mult
        }
        ("floor", 1) => x.floor(),
        ("ceil", 1) => x.ceil(),
        ("sqrt", 1) => x.sqrt(),
        ("pow", 2) => x.powf(args[1]),
        ("min", _) => args.iter().copied().fold(f64::INFINITY, f64::min),
        ("max", _) => args.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ("log", 1) => x.ln(),
        ("log10", 1) => x.log10(),
        ("log2", 1) => x.log2(),
        ("exp", 1) => x.exp(),
        ("sin", 1) => x.sin(),
        ("cos", 1) => x.cos(),
        ("tan", 1) => x.tan(),
        ("asin", 1) => x.asin(),
        ("acos", 1) => x.acos(),
        ("atan", 1) => x.atan(),
        ("atan2", 2) => x.atan2(args[1]),
        _ => {
            return Err(CalcError::new(
                CalcErrorKind::UnknownFunction,
                pos,
                format!("fonction inconnue « {name} »"),
            ));
        }
    };
    finite(v)
}

// ─────────────────────────────── API publique ───────────────────────────────

/// Évalue l'expression avec les variables fournies (clés de payload device).
/// Échoue proprement (`CalcError`) sur variable absente, division par zéro,
/// hors domaine — jamais de panic.
pub fn eval_calc(expr: &str, vars: &HashMap<String, f64>) -> Result<f64, CalcError> {
    let toks = lex(expr)?;
    if toks.is_empty() {
        return Err(CalcError::new(CalcErrorKind::Syntax, 0, "l'expression est vide"));
    }
    let mut p = Parser { toks, i: 0 };
    let ast = p.parse_expr()?;
    // Résidu : tout token restant est une erreur (ex. « 1 2 »).
    if let Some(t) = p.bump() {
        return Err(CalcError::new(
            CalcErrorKind::Syntax,
            t.pos,
            format!("symbole inattendu « {} » après la fin de l'expression", describe(&t.tok)),
        ));
    }
    eval(&ast, vars)
}

/// Erreurs statiques de l'expression (syntaxe, fonctions inconnues, arités)
/// — tolère les variables non définies (elles viennent du payload au
/// runtime). Vide = expression valide.
pub fn validate_calc(expr: &str) -> Vec<CalcError> {
    let mut errors = Vec::new();
    let toks = match lex(expr) {
        Ok(t) => t,
        Err(e) => return vec![e],
    };
    if toks.is_empty() {
        errors.push(CalcError::new(CalcErrorKind::Syntax, 0, "l'expression est vide"));
        return errors;
    }
    let mut p = Parser { toks, i: 0 };
    match p.parse_expr() {
        Err(e) => errors.push(e),
        Ok(ast) => {
            if let Some(t) = p.bump() {
                errors.push(CalcError::new(
                    CalcErrorKind::Syntax,
                    t.pos,
                    format!("symbole inattendu « {} » après la fin de l'expression", describe(&t.tok)),
                ));
            }
            analyze(&ast, &mut errors);
        }
    }
    errors
}

/// Variables référencées par l'expression (triées, dédupliquées) — alimente
/// l'aide de l'inspecteur (« cette expression attend : a, b »).
pub fn calc_variables(expr: &str) -> Vec<String> {
    let mut vars: Vec<String> = Vec::new();
    let Ok(toks) = lex(expr) else { return vars };
    for (i, t) in toks.iter().enumerate() {
        if let Tok::Ident(name) = &t.tok {
            let is_constant = matches!(name.as_str(), "pi" | "e");
            // Un identifiant suivi de « ( » est un appel de fonction.
            let is_call = toks.get(i + 1).map(|n| n.tok == Tok::LParen).unwrap_or(false);
            if !is_constant && !is_call && !vars.contains(name) {
                vars.push(name.clone());
            }
        }
    }
    vars.sort();
    vars.dedup();
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn arithmetique_et_precedence() {
        let v = vars(&[]);
        assert_eq!(eval_calc("1 + 2 * 3", &v).unwrap(), 7.0);
        assert_eq!(eval_calc("(1 + 2) * 3", &v).unwrap(), 9.0);
        assert_eq!(eval_calc("2 ^ 3 ^ 2", &v).unwrap(), 512.0);
        assert_eq!(eval_calc("-2^2", &v).unwrap(), -4.0);
        assert_eq!(eval_calc("2^-3", &v).unwrap(), 0.125);
        assert_eq!(eval_calc("7 % 3", &v).unwrap(), 1.0);
        assert_eq!(eval_calc("-x", &vars(&[("x", 2.5)])).unwrap(), -2.5);
        assert_eq!(eval_calc(".5 + 1", &v).unwrap(), 1.5);
    }

    #[test]
    fn comparaisons_logique_ternaire() {
        let v = vars(&[("t", 21.5)]);
        assert_eq!(eval_calc("t > 20 ? 1 : 0", &v).unwrap(), 1.0);
        assert_eq!(eval_calc("t < 20 ? 1 : 0", &v).unwrap(), 0.0);
        assert_eq!(eval_calc("t >= 21.5 && t <= 22", &v).unwrap(), 1.0);
        assert_eq!(eval_calc("t == 21.5 || t == 99", &v).unwrap(), 1.0);
        // Comparaison non chaînée : (t > 20) < 1 → 0.
        assert_eq!(eval_calc("t > 20 < 1", &v).unwrap(), 0.0);
    }

    #[test]
    fn fonctions_et_constantes() {
        let v = vars(&[]);
        assert_eq!(eval_calc("abs(-3)", &v).unwrap(), 3.0);
        assert_eq!(eval_calc("round(2.567, 2)", &v).unwrap(), 2.57);
        assert_eq!(eval_calc("round(2.5)", &v).unwrap(), 3.0);
        assert_eq!(eval_calc("min(3, 1, 2)", &v).unwrap(), 1.0);
        assert_eq!(eval_calc("max(3, 1, 2)", &v).unwrap(), 3.0);
        assert_eq!(eval_calc("pow(2, 10)", &v).unwrap(), 1024.0);
        assert_eq!(eval_calc("sqrt(9)", &v).unwrap(), 3.0);
        assert_eq!(eval_calc("floor(2.9) + ceil(2.1)", &v).unwrap(), 5.0);
        assert!((eval_calc("log(exp(1))", &v).unwrap() - 1.0).abs() < 1e-9);
        assert!((eval_calc("log10(1000)", &v).unwrap() - 3.0).abs() < 1e-9);
        assert!((eval_calc("sin(pi/2)", &v).unwrap() - 1.0).abs() < 1e-9);
        assert!((eval_calc("atan2(1, 1) * 4", &v).unwrap() - std::f64::consts::PI).abs() < 1e-9);
    }

    #[test]
    fn erreurs_propres_sans_panic() {
        let v = vars(&[]);
        // Variable absente.
        let e = eval_calc("a + b", &v).unwrap_err();
        assert_eq!(e.kind, CalcErrorKind::UnknownVariable);
        // Division par zéro.
        assert_eq!(eval_calc("1 / 0", &v).unwrap_err().kind, CalcErrorKind::DivisionByZero);
        assert_eq!(eval_calc("1 % 0", &v).unwrap_err().kind, CalcErrorKind::DivisionByZero);
        // Hors domaine.
        assert_eq!(eval_calc("sqrt(-1)", &v).unwrap_err().kind, CalcErrorKind::MathRange);
        assert_eq!(eval_calc("log(0)", &v).unwrap_err().kind, CalcErrorKind::MathRange);
        // Overflow → MathRange.
        assert_eq!(eval_calc("pow(10, 400)", &v).unwrap_err().kind, CalcErrorKind::MathRange);
    }

    #[test]
    fn erreurs_lexicales_et_syntaxe() {
        let v = vars(&[]);
        assert_eq!(eval_calc("1 € 2", &v).unwrap_err().kind, CalcErrorKind::Lexical);
        assert_eq!(eval_calc("(1 + 2", &v).unwrap_err().kind, CalcErrorKind::Syntax);
        assert_eq!(eval_calc("1 +", &v).unwrap_err().kind, CalcErrorKind::Syntax);
        assert_eq!(eval_calc("f(1", &v).unwrap_err().kind, CalcErrorKind::Syntax);
        assert_eq!(eval_calc("t ? 1", &v).unwrap_err().kind, CalcErrorKind::Syntax);
        assert_eq!(eval_calc("", &v).unwrap_err().kind, CalcErrorKind::Syntax);
        assert_eq!(eval_calc("   ", &v).unwrap_err().kind, CalcErrorKind::Syntax);
    }

    #[test]
    fn validation_statique_tolere_les_variables() {
        // Variables inconnues ≠ erreurs de validation (elles viennent du payload).
        assert!(validate_calc("a + b * 2").is_empty());
        // Fonction inconnue.
        let errs = validate_calc("foo(1) + 1");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].kind, CalcErrorKind::UnknownFunction);
        // Arité.
        assert_eq!(validate_calc("min()").len(), 1);
        assert_eq!(validate_calc("pow(2)").len(), 1);
        assert_eq!(validate_calc("round(1, 2, 3)").len(), 1);
        // Toutes les erreurs collectées.
        assert_eq!(validate_calc("foo(1) + bar(2)").len(), 2);
        assert_eq!(validate_calc("").len(), 1);
    }

    #[test]
    fn variables_extraites() {
        assert_eq!(calc_variables("(a + b) / min(a, 2)"), vec!["a", "b"]);
        assert_eq!(calc_variables("pi * r^2"), vec!["r"]);
        assert_eq!(calc_variables("sin(x) + cos(x)"), vec!["x"]);
        assert!(calc_variables("1 + 2").is_empty());
        assert!(calc_variables("foo(").is_empty()); // lexing échoue → liste vide
    }
}
