//! `gsfmt` — a formatter for Google Sheets formulas.
//!
//! # Why this works on tokens, not a semantic AST
//!
//! Layout runs on a raw token stream with shallow nesting (calls, paren
//! groups, arrays, arguments) rather than a semantic AST. This is
//! deliberate: a semantic AST necessarily discards detail this formatter
//! is contractually required to preserve.
//!
//! Both mature formula parsers were evaluated and both are lossy for this
//! purpose. `@borgar/fx` (JS) drops redundant parentheses;
//! `formualizer-parse` (Rust) additionally collapses a blank argument into
//! an empty-string literal, which is a *semantic* change — `IF(A1,,"x")`
//! and `IF(A1,"","x")` are different formulas in Sheets. Both also
//! normalise literal spelling (`1.50` → `1.5`).
//!
//! Here every leaf is the raw source slice, so the only thing that can
//! change is whitespace between tokens. A blank argument is simply an
//! argument holding zero tokens; a redundant paren group is a node;
//! identifier case and string bytes are copied verbatim.
//!
//! Two sanctioned exceptions, both mirroring what the Sheets editor
//! itself does on entry: in the dot locale, `;` argument separators are
//! rewritten to `,` (see `normalize_separators`; array row separators
//! are semantic and stay untouched), and — opt-in — call heads are
//! uppercased (see `uppercase_function_heads`). A leading UTF-8 BOM is
//! outside the contract entirely: it is a file-encoding artifact, not a
//! token, and both entry points drop it (see `strip_bom`).

use std::fmt;

/// Default target line width; see the README's Configuration section.
pub const DEFAULT_WIDTH: usize = 82;

/// Which character is the decimal mark — and therefore what `,` means.
///
/// Google Sheets picks this from the spreadsheet's locale. It cannot be
/// inferred from the formula text: a US formula's `{1,2;3,4}` already
/// contains both characters, so presence of `;` proves nothing. It is
/// therefore an explicit input, supplied by the CLI/config boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decimal {
    /// `1.5`, arguments separated by `,` (and `;` for array rows).
    #[default]
    Dot,
    /// `1,5`, arguments separated by `;` — de/fr/es and similar locales.
    Comma,
}

impl Decimal {
    fn mark(self) -> char {
        match self {
            Decimal::Dot => '.',
            Decimal::Comma => ',',
        }
    }
}

/// Everything the formatter needs beyond the source text.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Target line width. Not a hard ceiling — an unbreakable token wins.
    pub width: usize,
    pub decimal: Decimal,
    /// Rewrite call heads to uppercase (`sum(` → `SUM(`), as the Sheets
    /// editor itself does to builtin names on entry. Names bound by a
    /// `LET` or `LAMBDA` are left as authored within the binding's scope —
    /// see `uppercase_function_heads`. Off by default: token text is
    /// otherwise copied verbatim.
    pub uppercase_functions: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            decimal: Decimal::default(),
            uppercase_functions: false,
        }
    }
}

/// Indent added per nesting level.
const INDENT: usize = 2;

/// Hard ceiling on how far right the widest key may push the shared value
/// column of a `LET`/`IFS`/`SWITCH`, so one long name cannot shove every
/// value off the screen. See [`pairs_align`], which also scales the cap to
/// the window.
const ALIGN_MAX: usize = 40;

/// Groups nested deeper than this are rejected at parse time. Real formulas
/// nest shallowly — the fixtures peak around ten levels, the deepest
/// regression test uses forty. The cap exists because parsing recurses per
/// level (unbounded depth overflows the stack around ten thousand) and
/// layout cost grows super-linearly with depth, so a hostile paren tower
/// could hang an editor on save. The worst measured shape is pair layout
/// (nested `LET`), whose cost is roughly cubic in depth: ~0.45s at depth
/// 100 in a release build, ~4s at 199. One hundred keeps that worst case
/// under half a second.
const MAX_DEPTH: usize = 100;

// ───────────────────────────────────────────────────────────── errors ──

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    /// Character offset into the source formula.
    pub pos: usize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at character {})", self.msg, self.pos + 1)
    }
}

impl std::error::Error for Error {}

fn err<T>(msg: impl Into<String>, pos: usize) -> Result<T, Error> {
    Err(Error {
        msg: msg.into(),
        pos,
    })
}

// ──────────────────────────────────────────────────────────── tokenize ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Number, name, cell/range reference, `'quoted sheet'`, `#REF!`, or a
    /// name glued to a table selector / chip field (`Table1[Column 1]`,
    /// `A1.[email]`) — the selector bytes ride inside the token.
    Atom,
    /// `"..."` string literal, stored with its quotes and `""` escapes intact.
    Str,
    /// `+ - * / ^ & = < > <= >= <> % : !`
    Op,
    /// `(` or `{`
    Open,
    /// `)` or `}`
    Close,
    /// `,` or `;`
    Sep,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub text: String,
    pub kind: Kind,
    /// A blank line stood before this token in the source. Only consulted
    /// for `LET`-style pair layout, where it reproduces the author's
    /// logical grouping. Never synthesised.
    pub blank_before: bool,
    pub pos: usize,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '_' | '$' | '\\')
}

fn is_ident_body(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '\\' | '.')
}

/// Break a formula into tokens, preserving every byte of every token.
///
/// # Errors
///
/// Returns an error for an unterminated string or quoted sheet name, a
/// character that cannot start any token, or a `,` used as an argument
/// separator under [`Decimal::Comma`].
// One dispatch chain over the token kinds; splitting it into per-kind
// helpers would scatter the shared cursor and read worse.
#[allow(clippy::too_many_lines)]
pub fn tokenize(src: &str, decimal: Decimal) -> Result<Vec<Token>, Error> {
    let dec = decimal.mark();
    let c: Vec<char> = src.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let mut newlines = 0usize;

    while i < c.len() {
        let ch = c[i];

        // Whitespace: skipped, but a blank line is remembered for the next token.
        if ch.is_whitespace() {
            if ch == '\n' {
                newlines += 1;
            }
            i += 1;
            continue;
        }

        let start = i;
        let blank_before = newlines >= 2;
        newlines = 0;

        let (text, kind) = if ch == '"' {
            // String literal. `""` is an escaped quote, not a terminator.
            i += 1;
            loop {
                if i >= c.len() {
                    return err("unterminated string literal", start);
                }
                if c[i] == '"' {
                    if i + 1 < c.len() && c[i + 1] == '"' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            (c[start..i].iter().collect::<String>(), Kind::Str)
        } else if ch == '\'' {
            // Single-quoted sheet name, e.g. `'my sheet'!A1`. `''` escapes.
            i += 1;
            loop {
                if i >= c.len() {
                    return err("unterminated quoted sheet name", start);
                }
                if c[i] == '\'' {
                    if i + 1 < c.len() && c[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if ch == '#' {
            // Error literal: #REF! #N/A #DIV/0! #NAME? #NULL! #NUM! #VALUE!
            i += 1;
            while i < c.len() && (c[i].is_alphanumeric() || matches!(c[i], '_' | '/' | '.')) {
                i += 1;
            }
            if i < c.len() && matches!(c[i], '!' | '?') {
                i += 1;
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if ch.is_ascii_digit()
            // A leading decimal mark (`.5`) is ordinary US shorthand, but a
            // leading `,5` under Decimal::Comma is indistinguishable from a
            // stray separator. Requiring a digit in front means mixed-locale
            // input is reported rather than quietly reinterpreted.
            || (decimal == Decimal::Dot
                && ch == dec
                && i + 1 < c.len()
                && c[i + 1].is_ascii_digit())
        {
            // Number, including a trailing exponent. Under Decimal::Comma the
            // mark is `,`, so `1,5` is one token and never gets a space
            // inserted into it.
            //
            // At most one decimal mark: without this the scan is greedy and
            // `1,5,2,5` becomes a single bogus token instead of a number
            // followed by a stray separator (and `1.5.2` likewise in dot
            // mode). Stopping here lets the caller report the real problem.
            let mut seen_mark = false;
            while i < c.len() {
                if c[i].is_ascii_digit() {
                    i += 1;
                } else if c[i] == dec && !seen_mark {
                    seen_mark = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if i < c.len()
                && matches!(c[i], 'e' | 'E')
                && i + 1 < c.len()
                && (c[i + 1].is_ascii_digit()
                    || (matches!(c[i + 1], '+' | '-')
                        && i + 2 < c.len()
                        && c[i + 2].is_ascii_digit()))
            {
                i += if matches!(c[i + 1], '+' | '-') { 2 } else { 1 };
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if is_ident_start(ch) {
            // Name or reference. `!` and `:` are emitted as their own tight
            // operators, so `stock_data!P3` and `B6:B` rejoin on output.
            while i < c.len() && is_ident_body(c[i]) {
                i += 1;
            }
            // Structured table reference — Table1[Column 1], Table1[#ALL],
            // Table1[[#ALL],[Col 1]:[Col 3]] — and chip extraction, the
            // `.[field]` postfix (also on cells: A1.[email]). The whole
            // bracketed selector is swallowed into this atom, bytes
            // verbatim: inner commas, colons, and spaces are selector
            // syntax, not formula syntax, so layout must never reflow
            // them and the comma-locale separator guard must not see
            // them. The outer `while` glues each `.[field]` postfix (the
            // ident scan above already consumed the dot of a cell chip,
            // since `.` is an identifier-body character).
            //
            // Only balance and termination are checked — selector *shape*
            // (empty `[]`, a specifier in chip position) is not. The
            // grammar rejects those; a formatter preserves them, same
            // contract split as the `\` tolerance note in grammar.js.
            while i < c.len() && c[i] == '[' {
                let open = i;
                let mut depth = 0usize;
                while i < c.len() {
                    match c[i] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if i >= c.len() {
                    return err("unterminated table selector", open);
                }
                i += 1;
                if i + 1 < c.len() && c[i] == '.' && c[i + 1] == '[' {
                    i += 1;
                }
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if i + 1 < c.len() && matches!((ch, c[i + 1]), ('<', '=' | '>') | ('>', '=')) {
            i += 2;
            (c[start..i].iter().collect::<String>(), Kind::Op)
        } else if matches!(
            ch,
            '+' | '-' | '*' | '/' | '^' | '&' | '=' | '<' | '>' | '%' | ':' | '!'
        ) {
            i += 1;
            (ch.to_string(), Kind::Op)
        } else if matches!(ch, '(' | '{') {
            i += 1;
            (ch.to_string(), Kind::Open)
        } else if matches!(ch, ')' | '}') {
            i += 1;
            (ch.to_string(), Kind::Close)
        } else if ch == ';' {
            i += 1;
            (ch.to_string(), Kind::Sep)
        } else if ch == ',' {
            match decimal {
                Decimal::Dot => {
                    i += 1;
                    (ch.to_string(), Kind::Sep)
                }
                // Reaching here means a `,` that is not part of a number, so
                // the input mixes locales. Guessing either way would corrupt
                // the formula silently; say so instead.
                Decimal::Comma => {
                    return err(
                        "',' is the decimal mark in this locale — separate arguments with ';' \
                         (or set decimal = dot)",
                        start,
                    );
                }
            }
        } else {
            return err(format!("unexpected character {ch:?}"), start);
        };

        out.push(Token {
            text,
            kind,
            blank_before,
            pos: start,
        });
    }

    Ok(out)
}

// ─────────────────────────────────────────────────────────────── parse ──

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(Token),
    Group(Group),
}

/// A call (`SUM(...)`), a bare paren group (`(a + b)`), or an array (`{1, 2}`).
#[derive(Debug, Clone)]
pub struct Group {
    /// Function name for a call; `None` for a bare paren group or array.
    pub head: Option<Token>,
    pub open: Token,
    /// One entry per argument. An empty entry is a blank argument and is
    /// reproduced as such — `IF(x,,y)` never becomes `IF(x,"",y)`.
    pub args: Vec<Vec<Node>>,
    /// Separator tokens between arguments, so array row breaks (`;`) survive.
    pub seps: Vec<Token>,
    pub close: Token,
    /// True when the head is a `LET`/`LAMBDA`-bound name at this position in
    /// evaluation order (see [`mark_bound_heads`]): the call resolves to the
    /// passed value, not any builtin it shadows, so name-driven treatment —
    /// uppercase rewriting, pair layout — must not apply. Set by a pass over
    /// the parsed tree, not by the parser itself.
    pub(crate) bound_head: bool,
}

impl Group {
    /// True for a call written with no arguments at all, e.g. `NOW()`.
    fn is_empty_call(&self) -> bool {
        self.seps.is_empty() && self.args.len() == 1 && self.args[0].is_empty()
    }

    fn is_array(&self) -> bool {
        self.open.text == "{"
    }

    fn name_upper(&self) -> String {
        self.head
            .as_ref()
            .map(|h| h.text.to_ascii_uppercase())
            .unwrap_or_default()
    }
}

/// Parse a token stream into a nesting tree.
///
/// # Errors
///
/// Returns an error for unbalanced or mismatched brackets, a stray closing
/// bracket, or nesting deeper than `MAX_DEPTH` levels.
pub fn parse(toks: &[Token]) -> Result<Vec<Node>, Error> {
    let (items, next) = parse_items(toks, 0, 0)?;
    if next < toks.len() {
        let t = &toks[next];
        return err(format!("unexpected {:?}", t.text), t.pos);
    }
    Ok(items)
}

fn parse_items(toks: &[Token], mut i: usize, depth: usize) -> Result<(Vec<Node>, usize), Error> {
    let mut out = Vec::new();
    while i < toks.len() {
        match toks[i].kind {
            Kind::Sep | Kind::Close => break,
            Kind::Open => {
                let (g, ni) = parse_group(toks, i, None, depth)?;
                out.push(Node::Group(g));
                i = ni;
            }
            Kind::Atom
                if i + 1 < toks.len()
                    && toks[i + 1].kind == Kind::Open
                    && toks[i + 1].text == "(" =>
            {
                let (g, ni) = parse_group(toks, i + 1, Some(toks[i].clone()), depth)?;
                out.push(Node::Group(g));
                i = ni;
            }
            _ => {
                out.push(Node::Leaf(toks[i].clone()));
                i += 1;
            }
        }
    }
    Ok((out, i))
}

fn parse_group(
    toks: &[Token],
    open_i: usize,
    head: Option<Token>,
    depth: usize,
) -> Result<(Group, usize), Error> {
    let open = toks[open_i].clone();
    if depth >= MAX_DEPTH {
        return err(format!("nesting exceeds {MAX_DEPTH} levels"), open.pos);
    }
    let closer = if open.text == "(" { ")" } else { "}" };
    let mut args = Vec::new();
    let mut seps = Vec::new();
    let mut i = open_i + 1;

    loop {
        let (items, ni) = parse_items(toks, i, depth + 1)?;
        args.push(items);
        i = ni;

        if i >= toks.len() {
            return err(
                format!("unbalanced {:?} — missing {closer:?}", open.text),
                open.pos,
            );
        }
        match toks[i].kind {
            Kind::Sep => {
                seps.push(toks[i].clone());
                i += 1;
            }
            Kind::Close => {
                if toks[i].text != closer {
                    return err(
                        format!("mismatched {:?} — expected {closer:?}", toks[i].text),
                        toks[i].pos,
                    );
                }
                return Ok((
                    Group {
                        head,
                        open,
                        args,
                        seps,
                        close: toks[i].clone(),
                        bound_head: false,
                    },
                    i + 1,
                ));
            }
            _ => unreachable!("parse_items stops only at Sep or Close"),
        }
    }
}

fn first_token(items: &[Node]) -> Option<&Token> {
    match items.first()? {
        Node::Leaf(t) => Some(t),
        Node::Group(g) => Some(g.head.as_ref().unwrap_or(&g.open)),
    }
}

/// Rewrite `;` argument separators to `,` inside calls and paren groups.
///
/// Sheets itself accepts `;` between arguments in the dot locale and
/// rewrites it to `,` the moment the formula is entered; gsfmt mirrors
/// that. This is the one sanctioned exception to "token bytes are copied
/// verbatim". Array row separators are semantic — `{1;2}` is a column,
/// `{1,2}` a row — so array groups keep theirs, and under
/// [`Decimal::Comma`] nothing is touched because `;` is the only argument
/// separator there.
fn normalize_separators(items: &mut [Node]) {
    for node in items {
        if let Node::Group(g) = node {
            if !g.is_array() {
                for sep in &mut g.seps {
                    if sep.text == ";" {
                        sep.text = ",".into();
                    }
                }
            }
            for arg in &mut g.args {
                normalize_separators(arg);
            }
        }
    }
}

/// Rewrite call heads to their uppercase form, mirroring what the Sheets
/// editor does to builtin function names the moment a formula is entered.
/// The second sanctioned exception to "token bytes are copied verbatim",
/// and opt-in ([`Options::uppercase_functions`]).
///
/// Bound-name matching uses Unicode case *folding* (see [`fold_name`]),
/// not ASCII or plain uppercase conversion: Sheets names are
/// case-insensitive beyond ASCII too, and a weaker fold would treat a
/// bound call site as unbound and mangle it — `näme` under an ASCII
/// fold, or `k` bound as the Kelvin sign `K` under uppercase conversion.
///
/// Names bound by a `LET` or `LAMBDA` are user identifiers, not builtins:
/// rewriting an invocation `myTax(2)` while its binding site stays an
/// argument token would tear the two apart visually, so bound names keep
/// their authored case. Bindings follow Sheets' evaluation-order scope —
/// computed once by [`mark_bound_heads`], which must have run first: a
/// `LET` name is visible only to *subsequent* value expressions and the
/// final expression — `sum` in `LET(sum, sum(A1:A2), sum)` is a builtin
/// call in the value position, so it uppercases — and a `LAMBDA`
/// parameter is visible only in its body. Outside the binder the same
/// name is a builtin again. Named functions (defined at the spreadsheet
/// level) are indistinguishable from builtins here and are uppercased too;
/// the Sheets named-function editor already forces their names uppercase.
fn uppercase_function_heads(items: &mut [Node]) {
    for node in items {
        let Node::Group(g) = node else { continue };
        if !g.bound_head {
            if let Some(h) = &mut g.head {
                h.text = h.text.to_uppercase();
            }
        }
        for arg in &mut g.args {
            uppercase_function_heads(arg);
        }
    }
}

/// Case-insensitive matching key for a name.
///
/// `str::to_uppercase` alone is case *conversion*, not case *folding*:
/// U+212A KELVIN SIGN uppercases to itself yet folds to `k`, and `ẞ`
/// lowercases to `ß` while `ß` uppercases to `SS` — single round-trips
/// key those pairs differently. Lowercase → uppercase → lowercase routes
/// every variant through the expanded uppercase form first, keying all
/// the known conversion/folding divergences identically (`ẞ`/`ß` → `ss`,
/// Kelvin → `k`, final sigma → `σ`, Cherokee). Still an approximation of
/// UCD full case folding — exact folding needs the Unicode tables this
/// dependency-free crate deliberately avoids (same stance as [`cols`]) —
/// but both sides of every comparison go through this same key, so any
/// residual divergence is at least consistent.
fn fold_name(s: &str) -> String {
    s.to_lowercase().to_uppercase().to_lowercase()
}

/// The binding-site name of `arg`, case-folded via [`fold_name`]. A
/// binding site is a single atom when the formula is well-formed;
/// anything else yields nothing rather than guessing.
fn binding_name(arg: &[Node]) -> Option<String> {
    match arg {
        [Node::Leaf(t)] if t.kind == Kind::Atom => Some(fold_name(&t.text)),
        _ => None,
    }
}

/// Walk `items` setting [`Group::bound_head`], with `scope` as the stack
/// of `LET`/`LAMBDA` names bound at this point in evaluation order.
///
/// `LET(n1, v1, n2, v2, …, body)` binds each `nᵢ` starting *after* `vᵢ`
/// (a name is not visible in its own value expression);
/// `LAMBDA(p1, …, pn, body)` binds all but the last argument, visible in
/// the body. Either way the bindings pop when the binder's group ends —
/// they never leak to siblings or ancestors.
///
/// A call whose head is itself a bound name resolves to the passed value,
/// not the builtin it shadows — Google documents `LAMBDA` placeholders as
/// taking precedence over builtin names — so a bound `let(…)`/`lambda(…)`
/// is an ordinary user-function call: it neither binds its arguments nor
/// receives any other name-driven treatment (uppercasing, pair layout).
fn mark_bound_heads(items: &mut [Node], scope: &mut Vec<String>) {
    for node in items {
        let Node::Group(g) = node else { continue };
        g.bound_head = g
            .head
            .as_ref()
            .is_some_and(|h| scope.contains(&fold_name(&h.text)));
        let n = g.args.len();
        let depth = scope.len();
        let binder = if g.bound_head {
            String::new()
        } else {
            g.name_upper()
        };
        match binder.as_str() {
            "LET" if n >= 3 => {
                let mut pending = None;
                for (i, arg) in g.args.iter_mut().enumerate() {
                    mark_bound_heads(arg, scope);
                    if i + 1 < n && i % 2 == 0 {
                        pending = binding_name(arg);
                    } else if let Some(name) = pending.take() {
                        scope.push(name);
                    }
                }
            }
            "LAMBDA" if n >= 2 => {
                for (i, arg) in g.args.iter_mut().enumerate() {
                    mark_bound_heads(arg, scope);
                    if i + 1 < n {
                        if let Some(name) = binding_name(arg) {
                            scope.push(name);
                        }
                    }
                }
            }
            _ => {
                for arg in &mut g.args {
                    mark_bound_heads(arg, scope);
                }
            }
        }
        scope.truncate(depth);
    }
}

// ────────────────────────────────────────────────────── inline render ──

/// Operators printed with no surrounding space. `:` and `!` are part of a
/// reference (`B6:B`, `stock_data!P3`); `^` follows spreadsheet idiom (`9^99`).
fn is_tight(op: &str) -> bool {
    matches!(op, ":" | "!" | "^")
}

/// True when `out` ends inside an unterminated error literal — a `#`
/// followed only by characters its lexer scan consumes (alphanumerics,
/// `_`, `/`, `.`). `#N/A` qualifies; `#REF!` does not (the `!` closed it).
/// Appending another consumable character to one (`#N/A/A1`) would re-lex
/// as a single, different token.
fn ends_in_open_error_literal(out: &str) -> bool {
    let mut rev = out.chars().rev().peekable();
    while let Some(&c) = rev.peek() {
        if c.is_alphanumeric() || matches!(c, '_' | '/' | '.') {
            rev.next();
        } else {
            break;
        }
    }
    matches!(rev.peek(), Some('#'))
}

/// In minified output, keep adjacent tokens from fusing into a different
/// token: `<` `=` would re-lex as `<=`, and anything the `#` scan consumes
/// would be swallowed by a preceding open error literal.
fn guard_minified(out: &mut String, next: &str) {
    let (Some(l), Some(r)) = (out.chars().last(), next.chars().next()) else {
        return;
    };
    if matches!(l, '<' | '>') && matches!(r, '=' | '>') {
        out.push(' ');
        return;
    }
    if (r.is_alphanumeric() || matches!(r, '_' | '/' | '.' | '!' | '?'))
        && ends_in_open_error_literal(out)
    {
        out.push(' ');
    }
}

/// Drop a leading UTF-8 BOM. It is an artifact of the file's encoding, not
/// formula text — Windows editors and spreadsheet exports routinely prepend
/// one, and the tokenizer would otherwise reject the whole formula over it.
/// It is stripped, not preserved: output is identical whether or not the
/// source carried one.
fn strip_bom(src: &str) -> &str {
    src.strip_prefix('\u{feff}').unwrap_or(src)
}

/// Render a sequence on one line, also reporting where each item starts.
/// The reported offsets are *byte* offsets into the returned string — meant
/// for slicing; measure the slices with [`cols`] for column arithmetic.
fn render_inline(items: &[Node], minify: bool) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut offs = Vec::with_capacity(items.len());
    let mut prev_operand = false;
    let mut prev_leaf = false;
    let mut pending_space = false;

    for node in items {
        match node {
            Node::Leaf(t) if t.kind == Kind::Op => {
                let op = t.text.as_str();
                if op == "%" {
                    // Postfix: hugs the operand it modifies.
                    offs.push(out.len());
                    out.push('%');
                    prev_operand = true;
                    prev_leaf = false;
                    pending_space = false;
                } else if is_tight(op) {
                    // A tight `!` glued onto an open error literal would be
                    // consumed by its `#` scan on re-lex (`#N/A!` is one
                    // token); keep it a separate token in both modes.
                    if op == "!" && ends_in_open_error_literal(&out) {
                        out.push(' ');
                    }
                    offs.push(out.len());
                    out.push_str(op);
                    prev_operand = false;
                    pending_space = false;
                } else if matches!(op, "-" | "+") && !prev_operand {
                    // Unary sign: binds tightly to what follows.
                    if pending_space {
                        out.push(' ');
                    }
                    offs.push(out.len());
                    out.push_str(op);
                    prev_operand = false;
                    pending_space = false;
                } else {
                    if minify {
                        guard_minified(&mut out, op);
                    } else {
                        out.push(' ');
                    }
                    offs.push(out.len());
                    out.push_str(op);
                    prev_operand = false;
                    pending_space = !minify;
                }
            }
            _ => {
                // Two operand tokens with nothing between them (`A1 B1`,
                // `"a" "b"`) keep a separating space in both modes: gluing
                // them merges tokens on re-lex — `A1B1` is one atom, and
                // `"a""b"` even re-lexes as a single string with an escaped
                // quote. The same goes for a leaf operand followed by a
                // *headed* group: `A1 SUM(1)` glued becomes the call
                // `A1SUM(1)`. Sheets rejects such input (it has no space-
                // intersection operator), so this is preservation of
                // garbage, not endorsement. Headless groups stay glued to
                // whatever precedes them: `LAMBDA(x, x)(1000)` is a real
                // invocation.
                let leaf_operand = matches!(node, Node::Leaf(_));
                let headed_group = matches!(node, Node::Group(g) if g.head.is_some());
                if pending_space || (prev_operand && (leaf_operand || (prev_leaf && headed_group)))
                {
                    out.push(' ');
                }
                let s = match node {
                    Node::Leaf(t) => t.text.clone(),
                    Node::Group(g) => render_group_inline(g, minify),
                };
                if minify {
                    guard_minified(&mut out, &s);
                }
                offs.push(out.len());
                out.push_str(&s);
                prev_operand = true;
                prev_leaf = leaf_operand;
                pending_space = false;
            }
        }
    }

    (out, offs)
}

fn render_group_inline(g: &Group, minify: bool) -> String {
    let mut s = String::new();
    if let Some(h) = &g.head {
        s.push_str(&h.text);
    }
    s.push_str(&g.open.text);
    if !g.is_empty_call() {
        for (i, a) in g.args.iter().enumerate() {
            if i > 0 {
                s.push_str(g.seps.get(i - 1).map_or(",", |t| t.text.as_str()));
                if !minify {
                    s.push(' ');
                }
            }
            s.push_str(&render_inline(a, minify).0);
        }
    }
    s.push_str(&g.close.text);
    s
}

// ────────────────────────────────────────────────────────────── layout ──

fn ind(n: usize) -> String {
    " ".repeat(n)
}

/// Display width of a rendered fragment, in characters.
///
/// Layout arithmetic must never use byte length: multibyte content —
/// strings, Unicode names, quoted sheet names — would measure wider than it
/// displays and break lines far too early, and pair alignment would pad in
/// bytes and land values on crooked columns. Characters are still an
/// approximation (a combining mark or a double-width CJK glyph counts as
/// one column), but the error is small and consistent, and doing better
/// needs Unicode width tables this dependency-free crate deliberately
/// avoids.
///
/// Byte offsets (as returned by `render_inline`) remain the right tool for
/// *slicing*; this is only for column arithmetic.
fn cols(s: &str) -> usize {
    s.chars().count()
}

/// Widest physical line of a fragment emitted at column `col`.
///
/// A newline inside a string literal is content, not layout (see
/// [`format()`]): such a fragment already spans physical lines. Only the
/// first depends on `col`; the rest restart at column 0 and no layout
/// decision can narrow them. Measuring the fragment with [`cols`] instead
/// counts the whole span as one line, and an operator chain around a
/// multi-line string was being split at its operators even though every
/// physical line fit — a break that shortens nothing.
///
/// Backs [`emitted_span`], and through it every fit decision in pair
/// layout and the prefix/suffix columns in [`min_chunk_width`] and
/// [`layout_items`].
///
/// What deliberately does *not* use it: the inline shortcuts in
/// [`layout_items`], [`layout_group`] and [`format`], which keep the
/// full-span measure. Those ask whether a fragment is a single line at
/// all, not whether it fits — a formula carrying a newline inside a string
/// literal is not one line, and collapsing the structure around such a
/// string buries the nesting a reader needs, however comfortably each
/// physical line happens to fit.
fn emitted_widest(col: usize, s: &str) -> usize {
    match s.split_once('\n') {
        None => col + cols(s),
        Some((first, rest)) => rest
            .split('\n')
            .map(cols)
            .fold(col + cols(first), usize::max),
    }
}

/// Column the final emitted line of `s` ends at, placed at `col`. A
/// separator a caller appends lands there, not on the widest line — for a
/// fragment carrying a newline inside a string literal the two differ.
fn emitted_last(col: usize, s: &str) -> usize {
    match s.rsplit_once('\n') {
        None => col + cols(s),
        Some((_, last)) => cols(last),
    }
}

/// Widest physical line `s` occupies placed at `col`, counting `pending`
/// columns appended to its final line.
///
/// Every fit decision in pair layout goes through here, deliberately. A
/// newline inside a string literal is content, so a fragment can already
/// span lines before layout touches it: measuring its whole span as one
/// line reports a width nothing would ever emit. And what a caller appends
/// lands on the final line, which is not always the widest one. Three
/// separate fit tests in this module were written with `cols()` in this
/// position and all three were wrong in exactly that way — for a value
/// beside its key, for the same value hung below it, and for the key.
fn emitted_span(col: usize, s: &str, pending: usize) -> usize {
    emitted_widest(col, s).max(emitted_last(col, s) + pending)
}

/// Where a pair-laid group's keys end and its values begin.
///
/// `ends[p]` is the column at which key `p`'s **final physical line** ends —
/// not `arg_indent + cols(key)`, which reads a key carrying a newline
/// inside a string literal (`SWITCH` and `IFS` match on strings) as one
/// continuous line and reports a column no line reaches. For a single-line
/// key the two are the same number, so this generalises the arithmetic
/// rather than changing it.
///
/// `value_col(p)` is where that key's value starts when it sits beside the
/// key: a shared column derived from the widest key when the group aligns,
/// otherwise one space past this key's own end.
///
/// Computed here once and used by both [`min_pairs_width`] and
/// [`layout_pairs`], because every time this arithmetic has been written
/// twice the two copies have eventually disagreed.
struct PairKeys {
    ends: Vec<usize>,
    aligned: bool,
    shared_col: usize,
}

impl PairKeys {
    fn new(keys: &[String], arg_indent: usize, width: usize) -> Self {
        let ends: Vec<usize> = keys.iter().map(|k| emitted_last(arg_indent, k)).collect();
        let widest = ends.iter().copied().max().unwrap_or(arg_indent);
        let aligned = pairs_align(widest.saturating_sub(arg_indent), arg_indent, width);
        Self {
            ends,
            aligned,
            shared_col: widest + 2,
        }
    }

    fn value_col(&self, p: usize) -> usize {
        if self.aligned {
            self.shared_col
        } else {
            self.ends[p] + 2
        }
    }
}

/// Width of the line a key is left alone on when its value hangs below it:
/// the key, with the separator that follows it on its final line. Keys are
/// not always one line — `SWITCH` and `IFS` match on string literals, which
/// may carry a newline.
fn key_line_width(key: &str, arg_indent: usize, sep: usize) -> usize {
    emitted_span(arg_indent, key, sep)
}

/// A block's width bound. `widest` is the widest line the block could need;
/// `last` is where its final line ends — the column at which a separator
/// appended by the caller would land. `last <= widest` always holds.
#[derive(Clone, Copy)]
struct MinWidth {
    widest: usize,
    last: usize,
}

impl MinWidth {
    /// Widest line once `sep` columns are appended to the final line.
    fn with_sep(self, sep: usize) -> usize {
        self.widest.max(self.last + sep)
    }
}

/// Narrowest width this group could occupy with its body starting at `col` —
/// the width it would take if every breakable construct broke. Nothing can
/// make it narrower, so comparing this against the target answers "is the
/// natural indent hopeless here?" without laying anything out.
///
/// Every non-final argument carries its separator on the last line of its
/// block, exactly as `layout_group` appends it — measuring the bare argument
/// reads one column narrow, and at the edge that one omitted comma is the
/// difference between clamping and overflowing.
///
/// `width` is not a budget to spend here; it is passed through because one
/// layout choice — whether a pair's value hangs below its key, see
/// [`pair_value_col`] — depends on it, and the bound has to model the
/// layout the formatter will actually emit at *this* width.
fn min_group_width(g: &Group, col: usize, width: usize) -> MinWidth {
    let head = g.head.as_ref().map_or(0, |h| cols(&h.text)) + cols(&g.open.text);
    let last = col + cols(&g.close.text);
    let widest = (col + head).max(last);
    let arg_indent = col + INDENT;

    // Pair-laid groups pin each key and its value to the same line, at a
    // column derived from the widest key. Measuring their arguments
    // independently underestimates: it reports a layout the formatter will
    // never emit, and the caller then declines to clamp a body that
    // genuinely does not fit. When `layout_group` abandons pairs for the
    // plain per-argument layout (breakable oversized key), this figure
    // over-reports — the safe direction: the real layout only gets narrower.
    if !g.is_array() && !g.bound_head {
        if let Some(lead) = pair_lead(&g.name_upper()) {
            if g.args.len() > lead + 1 {
                return MinWidth {
                    widest: widest.max(min_pairs_width(g, lead, arg_indent, width)),
                    last,
                };
            }
        }
    }

    // A blank argument gets no block of its own: layout hangs its
    // punctuation off the previous line (`, ,`), so its columns accumulate
    // onto the preceding block's final-line figure — or onto the opening
    // line when the group starts blank. A final blank argument appends
    // nothing (layout drops even the joining space), so it counts nothing.
    let n = g.args.len();
    let mut widest = widest;
    let mut idx = 0;
    let mut open_line = col + head;
    while idx < n && g.args[idx].is_empty() {
        if idx + 1 < n {
            if idx > 0 {
                open_line += 1;
            }
            open_line += sep_len(g, idx);
        }
        idx += 1;
    }
    widest = widest.max(open_line);
    while idx < n {
        // This argument's separator, plus every non-final blank argument
        // hanging off its final line (a joining space and a separator each).
        let end_cols = trailing_cols(g, idx);
        let mut j = idx + 1;
        while j < n && g.args[j].is_empty() {
            j += 1;
        }
        widest = widest.max(min_items_width(&g.args[idx], arg_indent, width).with_sep(end_cols));
        idx = j;
    }
    MinWidth { widest, last }
}

/// Columns the separator after argument `i` occupies, mirroring the
/// `sep_after` closures in the layout functions.
fn sep_len(g: &Group, i: usize) -> usize {
    g.seps.get(i).map_or(1, |t| cols(&t.text))
}

/// Columns that land on argument `i`'s final line once the group is laid
/// out: its own separator, plus the punctuation of every non-final blank
/// argument hanging off it, since layout gives a blank argument no line of
/// its own. Zero for the final argument — the closing bracket takes a new
/// line, so nothing follows it there.
///
/// The bound counts these through `with_sep` and the layout charges them as
/// `pending`; sharing the arithmetic is what keeps the two agreeing.
fn trailing_cols(g: &Group, i: usize) -> usize {
    let n = g.args.len();
    if i + 1 == n {
        return 0;
    }
    let mut out = sep_len(g, i);
    let mut j = i + 1;
    while j < n && g.args[j].is_empty() {
        if j + 1 < n {
            out += 1 + sep_len(g, j);
        }
        j += 1;
    }
    out
}

/// Whether a pair-laid group's values share one alignment column, keyed off
/// the widest key.
///
/// [`ALIGN_MAX`] alone is a claim about the *window* — "off the screen" —
/// enforced without reference to one. At the default width its 40 columns
/// are about half the room; at `--width 30` they exceed the window
/// entirely, and a 26-column key claimed a 28-column gutter that opened the
/// value column exactly on the right margin. Nothing can be laid out at a
/// column the window does not contain, so require the value column to be
/// inside it. That is the literal reading of the rule, and it fires only
/// where the fixed cap stops describing anything: a gutter that fits the
/// window is left alone, however narrow the window, so the shape at 82 —
/// and at every width where alignment was doing real work — is unchanged.
///
/// Both the bound ([`min_pairs_width`]) and the layout ([`layout_pairs`])
/// route through here, for the same reason they share [`pair_value_col`] —
/// a bound that disagrees with the layout it is bounding is the bug class
/// this module keeps relearning.
fn pairs_align(widest_key: usize, arg_indent: usize, width: usize) -> bool {
    widest_key + 2 <= ALIGN_MAX && arg_indent + widest_key + 2 < width
}

/// Whether `layout_items` will keep `value` on one block placed at `col`.
///
/// This mirrors the renderer's decision rather than improving on it. The
/// inline shortcut asks the *span* question — is this fragment a single
/// line at all — and a value carrying a newline inside a string literal is
/// not, so a value holding a breakable group is opened out there however
/// well its physical lines fit. Predicting otherwise is how a value came to
/// be placed beside its key only for the renderer to expand it into a tower
/// at that column: the bound-disagrees-with-layout fault the rest of this
/// module exists to prevent.
///
/// The second clause covers what the shortcut does not govern: with no
/// group to open, `layout_items` emits the value exactly as rendered
/// whichever way the shortcut went — an operator chain included, since a
/// chain splits only under width pressure measured on physical lines. So
/// its physical lines are what reach the page.
fn stays_inline(value: &[Node], inline: &str, col: usize, width: usize, pending: usize) -> bool {
    if contains_authored_grouping(value) {
        return false;
    }
    let renderer_keeps_it =
        col + cols(inline) <= width && emitted_last(col, inline) + pending <= width;
    renderer_keeps_it || (!has_openable_group(value) && emitted_span(col, inline, pending) <= width)
}

/// Where a pair's value block starts: beside its key at `aligned_col`, or
/// hanging on its own line one step in from the key.
///
/// Alignment is a *first-line* affordance. A value that stays on one line
/// may sit at the aligned column — that is padding, and padding has no
/// geometry. A value that has to break has nothing left to align, and
/// carrying the aligned column into its body would indent by horizontal
/// position rather than by nesting depth: the skinny right-margin towers
/// `layout_items` already refuses to emit, and the deeper the value the
/// narrower they get. Hanging it restores the indent grid, which is what
/// every modern formatter does with a binding whose value will not fit
/// beside it.
///
/// Hanging costs a line, so it has to buy one. A value that can break buys
/// room for its own body and always takes it. A value that cannot break —
/// a single long reference, a bare number — buys nothing unless the whole
/// run then fits on the line below, *and* the key fits the line it is left
/// alone on: otherwise the pair overflows wherever the value goes, and
/// moving it only spends the line (the stance `pair_layout_fits` takes on
/// an oversized key). A key so short that hanging would not move the value
/// left has nothing to gain either.
///
/// Reading "cannot break" as "cannot be helped" is the trap here. A bare
/// `0.55` under a 23-column key overflows a narrow window only because the
/// key's gutter pushed it there; one line down it fits with room to spare.
/// The thing that had to move was the value, not anything inside it.
///
/// The decision reads only this level: an inline render and a scan for
/// breakable positions, never a recursive width probe. Measuring the value
/// at both columns to pick the better one doubles the bound's work at
/// every nesting level, which on a deep `LET` chain is exponential.
fn pair_value_col(
    value: &[Node],
    aligned_col: usize,
    arg_indent: usize,
    width: usize,
    pending: usize,
    key_line: usize,
) -> usize {
    let hang = arg_indent + INDENT;
    if hang >= aligned_col {
        return aligned_col;
    }
    // Both columns are judged the same way, on the physical lines each
    // would emit: a newline inside a string literal is content, so such a
    // value already spans lines wherever it is put, and measuring its whole
    // span as one line reports a width neither column would produce. The
    // separator lands on the final line, which is not always the widest.
    let inline = render_inline(value, false).0;
    if stays_inline(value, &inline, aligned_col, width, pending) {
        return aligned_col;
    }
    // It cannot stay beside the key, so hanging has to buy something for
    // the line it costs. A value that can break buys room for its own body.
    // One that cannot buys nothing unless the whole run then fits — and
    // only if the key fits the line it is left alone on, since otherwise
    // the pair overflows wherever the value goes.
    if is_breakable(value)
        || (key_line <= width && stays_inline(value, &inline, hang, width, pending))
    {
        hang
    } else {
        aligned_col
    }
}

/// True when a sequence has somewhere to break: a spaced binary operator,
/// or a group that can open out. Everything else is one unbreakable run,
/// and no column it is placed at changes how many lines it takes.
fn is_breakable(items: &[Node]) -> bool {
    !binary_op_positions(items).is_empty() || has_openable_group(items)
}

/// True when a sequence holds a group that can open out — the only thing
/// `layout_items` expands once its inline shortcut has declined.
///
/// Operators are not enough. A chain breaks only under real width
/// pressure, measured on physical lines, so a chain whose lines already fit
/// is emitted exactly as rendered even though the shortcut turned it down
/// on its span. Treating "breakable" as "will expand" hung such values for
/// nothing.
fn has_openable_group(items: &[Node]) -> bool {
    items
        .iter()
        .any(|n| matches!(n, Node::Group(g) if !g.is_empty_call()))
}

/// Minimum width of a pair-aligned body, mirroring `layout_pairs`' columns —
/// including the separator each lead argument and non-final pair carries,
/// and the key line a hanging value leaves behind.
fn min_pairs_width(g: &Group, lead: usize, arg_indent: usize, width: usize) -> usize {
    let n = g.args.len();
    let pair_count = (n - lead) / 2;
    let has_tail = (n - lead) % 2 == 1;
    let key_of = |p: usize| lead + p * 2;

    let keys: Vec<String> = (0..pair_count)
        .map(|p| render_inline(&g.args[key_of(p)], false).0)
        .collect();
    let pk = PairKeys::new(&keys, arg_indent, width);

    let mut widest = arg_indent;
    for i in 0..lead {
        widest = widest.max(min_items_width(&g.args[i], arg_indent, width).with_sep(sep_len(g, i)));
    }
    for p in 0..pair_count {
        let ki = key_of(p);
        let vi = ki + 1;
        let aligned_col = pk.value_col(p);
        let sep = if p + 1 == pair_count && !has_tail {
            0
        } else {
            sep_len(g, vi)
        };
        let col = pair_value_col(
            &g.args[vi],
            aligned_col,
            arg_indent,
            width,
            sep,
            key_line_width(&keys[p], arg_indent, sep_len(g, ki)),
        );
        // A hanging value leaves its key alone on the line above, carrying
        // only the separator that follows it.
        if col < aligned_col {
            widest = widest.max(key_line_width(&keys[p], arg_indent, sep_len(g, ki)));
        }
        widest = widest.max(min_items_width(&g.args[vi], col, width).with_sep(sep));
    }
    if has_tail {
        widest = widest.max(min_items_width(&g.args[n - 1], arg_indent, width).widest);
    }
    widest
}

/// Same, for a sequence.
///
/// A sequence only ever breaks at spaced binary operators, so everything
/// between two of them is one atom as far as width goes. Treating each item
/// as separately placeable underestimates badly: `sheet!$AA:$AA` is five
/// tokens joined by tight operators that can never be split, and measuring
/// them individually reported 80 columns for something that needs 88.
fn min_items_width(items: &[Node], col: usize, width: usize) -> MinWidth {
    let ops = binary_op_positions(items);
    let Some(&first) = ops.first() else {
        return min_chunk_width(items, col, width);
    };

    // Mirror the chain layout in `layout_items` exactly: the first chunk
    // sits at `col`, and every operator starts a continuation line indented
    // one step, with its operand beginning after the operator and a space.
    // Measuring a chunk *from* its operator at the original column instead
    // renders a leading `+` as a unary sign — no space, no continuation
    // indent — and the bound comes in short, skipping a clamp the emitted
    // line needed.
    let cont = col + INDENT;
    let mut widest = min_chunk_width(&items[..first], col, width).widest;
    let mut last = 0;
    for (n, &start) in ops.iter().enumerate() {
        let end = ops.get(n + 1).copied().unwrap_or(items.len());
        let Node::Leaf(op) = &items[start] else {
            unreachable!("op position is a leaf")
        };
        let operand_col = cont + cols(&op.text) + 1;
        let m = min_chunk_width(&items[start + 1..end], operand_col, width);
        widest = widest.max(m.widest);
        last = m.last;
    }
    MinWidth { widest, last }
}

/// Narrowest width for one unbreakable run. Its leading tokens must all sit
/// on one line; only a trailing group can open out, and at best its body is
/// clamped back to `col + INDENT`. Anything after that group rides on its
/// closing line, so it widens both the bound and the final-line column.
fn min_chunk_width(items: &[Node], col: usize, width: usize) -> MinWidth {
    let (inline, offs) = render_inline(items, false);
    let Some(gi) = items.iter().rposition(|n| matches!(n, Node::Group(_))) else {
        // Nothing here can open out, so this run is emitted exactly as
        // rendered — physical lines and all. A newline inside a string
        // literal already put it on several, and reporting their total as
        // one line is a width the formatter will never produce. Reachable
        // from pair layout through `min_pairs_width`, where it rejected
        // pair layouts whose widest emitted line fitted comfortably.
        return MinWidth {
            widest: emitted_widest(col, &inline),
            last: emitted_last(col, &inline),
        };
    };
    let Node::Group(g) = &items[gi] else {
        unreachable!("rposition matched a group")
    };
    let head = g.head.as_ref().map_or(0, |h| cols(&h.text)) + cols(&g.open.text);
    // `offs` and the rendered group length are byte offsets: slice with
    // them, then measure the slices in columns.
    let glen = render_group_inline(g, false).len();
    // Physical lines: a prefix carrying a newline inside a string literal
    // does not push the group right by its whole span — the group opens
    // after the prefix's *final* line, which may start at column zero. A
    // suffix rides the closing line and may carry its own newlines away
    // from it. `layout_items` derives the same two columns the same way.
    let prefix_str = &inline[..offs[gi]];
    let suffix_str = &inline[offs[gi] + glen..];
    let prefix_end = emitted_last(col, prefix_str);
    let pushed = prefix_end.saturating_sub(col);
    // The opening line is unavoidable; the body can at best be clamped —
    // to `col + prefix` capped at one indent step, exactly the cap
    // `layout_items` applies. Measuring the body a full step in from `col`
    // regardless of the prefix inflates the bound by INDENT at *every*
    // nesting level, so a deeply nested value reads far wider than the
    // layout it will actually get.
    let body = min_group_width(g, col + pushed.min(INDENT), width);
    let last = emitted_last(body.last, suffix_str);
    MinWidth {
        widest: emitted_widest(col, prefix_str)
            .max(prefix_end + head)
            .max(body.widest)
            .max(emitted_widest(body.last, suffix_str)),
        last,
    }
}

/// `LET`/`IFS`/`SWITCH` bind (key, value) pairs that read best one pair per
/// line with the values column-aligned, and the rest of the builtins whose
/// variadic tail repeats in twos — the `*IFS` (criteria range, criterion)
/// aggregations, `SORT`/`SORTN` (column, ascending), `AVERAGE.WEIGHTED`
/// (values, weights), `GETPIVOTDATA` (column, item) — read the same way.
/// Returns how many leading arguments sit alone before the pairs begin:
/// none for the pure-pair forms, the aggregated range or switched
/// expression for the lead-1 forms, `GETPIVOTDATA`'s value name and anchor
/// cell, `SORTN`'s range, count, and ties mode.
fn pair_lead(name_upper: &str) -> Option<usize> {
    match name_upper {
        "LET" | "IFS" | "COUNTIFS" | "AVERAGE.WEIGHTED" => Some(0),
        "SWITCH" | "SUMIFS" | "AVERAGEIFS" | "MAXIFS" | "MINIFS" | "COUNTUNIQUEIFS" | "SORT" => {
            Some(1)
        }
        "GETPIVOTDATA" => Some(2),
        "SORTN" => Some(3),
        _ => None,
    }
}

/// Whether pair layout is usable for this break, or the group should take
/// the plain per-argument layout instead. Pair layout renders each key
/// inline, so the one shape it cannot handle sends the group to the plain
/// layout, where every piece can wrap: a key that overflows the width
/// *and* would get narrower broken — a criteria range built from a call,
/// say `INDIRECT(…)`.
///
/// A (key, value) that does not fit side by side used to be a second such
/// shape, because pair layout pinned the two to one line. It no longer
/// does — [`pair_value_col`] hangs the value on the line below — so the
/// pairing survives a value too big to sit beside its key, and falling
/// back is not needed to fit it.
///
/// When neither layout can make the group fit — a lone `LET` binding name
/// longer than the width — pair layout stays: falling back would tear the
/// pair apart without fixing the overshoot, and width is a target, not a
/// ceiling. Hanging follows the same rule, in [`pair_value_col`].
fn pair_layout_fits(g: &Group, lead: usize, arg_indent: usize, width: usize) -> bool {
    let n = g.args.len();
    let pair_count = (n - lead) / 2;
    let keys_inline = (0..pair_count).all(|p| {
        let key = &g.args[lead + p * 2];
        // Physical lines again: a key carrying a newline inside a string
        // literal is not the width of its span, and the value's gap lands
        // after its final line. `emitted_widest` is what breaking the key
        // would have to beat to be worth doing.
        let rendered = render_inline(key, false).0;
        emitted_span(arg_indent, &rendered, 2) <= width
            || min_items_width(key, arg_indent, width).widest
                >= emitted_widest(arg_indent, &rendered)
    });
    if !keys_inline {
        return false;
    }
    if min_pairs_width(g, lead, arg_indent, width) <= width {
        return true;
    }
    // Pairs overflow: fall back only if the plain layout genuinely fits.
    // The final argument carries no separator.
    let plain = g
        .args
        .iter()
        .enumerate()
        .filter(|(_, a)| !a.is_empty())
        .map(|(i, a)| {
            let sep = if i + 1 == n { 0 } else { sep_len(g, i) };
            min_items_width(a, arg_indent, width).with_sep(sep)
        })
        .max()
        .unwrap_or(arg_indent);
    plain > width
}

/// `LET` always breaks, however short it is. It exists to name intermediate
/// values for readability, so collapsing one onto a single line defeats the
/// reason it was written. Ordinary short calls (`SUM(a, b)`) still stay
/// inline — including a bound user function that happens to be named `let`.
fn always_breaks(g: &Group) -> bool {
    !g.bound_head && g.name_upper() == "LET" && g.args.len() >= 3
}

/// A blank line between arguments is an explicit grouping the author wrote,
/// and it can only exist in a broken layout. So a group carrying one stays
/// broken even when it would fit on a single line — collapsing it would
/// silently discard that grouping.
fn has_authored_grouping(g: &Group) -> bool {
    g.args
        .iter()
        .skip(1)
        .any(|a| first_token(a).is_some_and(|t| t.blank_before))
}

/// True if this group, or anything nested inside it, carries authored
/// grouping — an enclosing call cannot collapse to one line either.
fn group_forces_break(g: &Group) -> bool {
    always_breaks(g)
        || has_authored_grouping(g)
        || g.args.iter().any(|a| contains_authored_grouping(a))
}

fn contains_authored_grouping(items: &[Node]) -> bool {
    items
        .iter()
        .any(|n| matches!(n, Node::Group(g) if group_forces_break(g)))
}

/// A broken `LAMBDA` always gives its body its own block, even when the body
/// would fit on one line. Keeps `LAMBDA(x, y, IF(...))` readable rather than
/// trailing a dangling close paren. A bound user function named `lambda`
/// has no body argument, so the rule skips it.
fn force_arg_break(g: &Group, arg_index: usize) -> bool {
    !g.bound_head && g.name_upper() == "LAMBDA" && g.args.len() > 1 && arg_index + 1 == g.args.len()
}

/// Positions of the *spaced* binary operators in a flat sequence — the only
/// legal places to break a line.
///
/// Tight operators are excluded on purpose: `:` and `!` are part of a
/// reference (`stock_data!$A$3:INDEX(...)`), `^` follows spreadsheet idiom,
/// and a `%` or a unary sign binds to the operand it modifies. Breaking at
/// any of those would split a single value across lines.
fn binary_op_positions(items: &[Node]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev_operand = false;
    for (i, node) in items.iter().enumerate() {
        match node {
            Node::Leaf(t) if t.kind == Kind::Op => {
                let op = t.text.as_str();
                if op == "%" {
                    prev_operand = true;
                } else if is_tight(op) || (matches!(op, "-" | "+") && !prev_operand) {
                    prev_operand = false;
                } else {
                    out.push(i);
                    prev_operand = false;
                }
            }
            _ => prev_operand = true,
        }
    }
    out
}

/// Lay out a sequence. Line 0 carries no indent (the caller places it at
/// `start_col` — the leading `=` at top level, or the argument indent);
/// every later line carries its full absolute indent, derived from
/// `indent`. The two differ only at the top level, where `=` occupies a
/// column that is not indentation.
///
/// `force` suppresses only the inline shortcut at *this* level.
///
/// `pending` is what the caller will append to this block's final line —
/// an argument separator, a suffix riding a closing bracket, or both. A
/// fit test that cannot see it accepts a block ending exactly at `width`
/// and then the comma lands one column past it, which was a known gap
/// until the width bound learned to count the same columns (`with_sep`);
/// the two now measure the same thing. It is charged to the *final* line
/// specifically, which for a fragment carrying a newline inside a string
/// literal is not the widest one — hence [`emitted_last`].
fn layout_items(
    items: &[Node],
    start_col: usize,
    indent: usize,
    width: usize,
    force: bool,
    pending: usize,
) -> Vec<String> {
    let (inline, offs) = render_inline(items, false);
    // Two measures, deliberately: the span decides whether the fragment is
    // one line, and only the *final* line carries what the caller appends.
    // Folding `pending` into the span charges a multi-line string literal's
    // whole width for a comma that lands after its last byte.
    // Two questions here, and only one of them is "does it fit". The span
    // asks whether this fragment is a single line at all: a formula
    // carrying a newline inside a string literal is not, and collapsing the
    // structure around such a string buries the nesting a reader needs —
    // see `operator_chain_around_multiline_string_stays_inline`. `pending`
    // is then charged against the final line, which is where it lands.
    if !force
        && !contains_authored_grouping(items)
        && start_col + cols(&inline) <= width
        && emitted_last(start_col, &inline) + pending <= width
    {
        return vec![inline];
    }

    // An operator chain breaks at its operators, with the operator leading
    // each continuation line. Without this a long chain stays on one line and
    // the trailing-group break below indents from its full width, producing a
    // staircase that marches off the right edge.
    //
    // Gated on real width pressure, not on `force`. `force` only means "do
    // not take the inline shortcut here" (a LAMBDA body gets its own block,
    // an authored blank line holds a group open); splitting a short `v * 2`
    // across lines because of that would be gratuitous, and it also strips
    // the very grouping that forced the break.
    let ops = binary_op_positions(items);
    let emitted =
        emitted_widest(start_col, &inline).max(emitted_last(start_col, &inline) + pending);
    if emitted > width && !ops.is_empty() {
        let cont = indent + INDENT;
        let mut lines = layout_items(&items[..ops[0]], start_col, indent, width, false, 0);
        for (n, &start) in ops.iter().enumerate() {
            let end = ops.get(n + 1).copied().unwrap_or(items.len());
            let Node::Leaf(op) = &items[start] else {
                unreachable!("op position is a leaf")
            };
            let operand = &items[start + 1..end];
            let head = format!("{}{} ", ind(cont), op.text);
            let last_operand = n + 1 == ops.len();
            let carried = if last_operand { pending } else { 0 };
            let mut block = layout_items(operand, cols(&head), cols(&head), width, false, carried);
            block[0] = format!("{head}{}", block[0]);
            lines.extend(block);
        }
        return lines;
    }

    // No operators: break the last group, keeping what precedes it on the
    // opening line and what follows it on the closing line.
    let Some(gi) = items.iter().rposition(|n| matches!(n, Node::Group(_))) else {
        return vec![inline];
    };
    let Node::Group(g) = &items[gi] else {
        unreachable!("rposition matched a group")
    };

    // Byte offsets slice; columns measure.
    let prefix = inline[..offs[gi]].to_string();
    let glen = render_group_inline(g, false).len();
    let suffix = inline[offs[gi] + glen..].to_string();

    // Two different columns, which must not be conflated:
    //
    //   start_col  where the group's opening line actually begins, after the
    //              prefix. Fit decisions must use this or they lie.
    //   body       where its continuation lines and closing bracket sit.
    //
    // Passing the clamped value as both made an overlong group look like it
    // fitted inline, emitting a line far past the width.
    //
    // The body is capped at the block indent (`indent + INDENT`) — a
    // prefix shorter than one indent level keeps its natural column, a
    // longer one never drags the body past the cap. Hanging the body under
    // the bracket was the one place layout indented by horizontal position
    // instead of nesting, and it produced skinny right-margin towers the
    // moment a chain like `INDEX(…):INDEX(…)` broke its tail — the deeper
    // the prefix, the less width every argument had. One rule also means
    // one shape at every width: narrowing the window changes where lines
    // break, not the geometry. `min_chunk_width` models exactly this cap,
    // so the width bound and the emitted layout agree by construction.
    //
    // The group's opening line begins at `group_col` (which includes the
    // `=` column at top level); its body indents from `indent` (which does
    // not — `=` is a column, not indentation, and the close bracket must
    // stay on the indent grid).
    // Same two columns as `min_chunk_width`, derived the same way: the
    // group opens where the prefix's final line ends, and how far that
    // pushed us is measured from there, not from the prefix's whole span.
    let group_col = emitted_last(start_col, &prefix);
    let body = (indent + group_col.saturating_sub(start_col)).min(indent + INDENT);
    let mut lines = layout_group(g, group_col, body, width, force, cols(&suffix) + pending);
    lines[0] = format!("{prefix}{}", lines[0]);
    if !suffix.is_empty() {
        let last = lines.len() - 1;
        lines[last].push_str(&suffix);
    }
    lines
}

/// `start_col` is where this group's opening line begins — used for every
/// "does it fit?" decision. `indent` is where its continuation lines and
/// closing bracket sit. They differ only when a prefix pushed the group right
/// far enough that the body had to be clamped back.
fn layout_group(
    g: &Group,
    start_col: usize,
    indent: usize,
    width: usize,
    force: bool,
    pending: usize,
) -> Vec<String> {
    let inline = render_group_inline(g, false);
    if !force
        && !group_forces_break(g)
        && start_col + cols(&inline) <= width
        && emitted_last(start_col, &inline) + pending <= width
    {
        return vec![inline];
    }
    if g.is_empty_call() {
        return vec![inline];
    }

    let head = g.head.as_ref().map_or(String::new(), |h| h.text.clone());
    let open = format!("{head}{}", g.open.text);
    let arg_indent = indent + INDENT;

    if !g.is_array() && !g.bound_head {
        if let Some(lead) = pair_lead(&g.name_upper()) {
            if g.args.len() > lead + 1 && pair_layout_fits(g, lead, arg_indent, width) {
                return layout_pairs(g, &open, lead, indent, width);
            }
        }
    }

    // Lay out every argument, then decide between packing the simple leading
    // ones onto the opening line or giving each its own line.
    let laid: Vec<Vec<String>> = g
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if a.is_empty() {
                vec![String::new()]
            } else {
                layout_items(
                    a,
                    arg_indent,
                    arg_indent,
                    width,
                    force_arg_break(g, i),
                    trailing_cols(g, i),
                )
            }
        })
        .collect();

    let sep_after = |i: usize| -> &str { g.seps.get(i).map_or(",", |t| t.text.as_str()) };

    let mut lines: Vec<String> = Vec::new();
    // A one-element block can still span physical lines: a string literal
    // with embedded newlines renders inline but is multi-line on screen,
    // and packing simple arguments ahead of it reads the same as packing
    // them ahead of any other multi-line block.
    let first_multi = laid.iter().position(|l| l.len() > 1 || l[0].contains('\n'));

    // Hybrid break: `SCAN(firstFriday, SEQUENCE(...),` then the lambda below.
    let packed = match first_multi {
        Some(k) if k > 0 => {
            let mut head_line = open.clone();
            for i in 0..k {
                if i > 0 {
                    head_line.push(' ');
                }
                head_line.push_str(&laid[i][0]);
                head_line.push_str(sep_after(i));
            }
            (start_col + cols(&head_line) <= width).then_some((k, head_line))
        }
        _ => None,
    };

    let start = if let Some((k, head_line)) = packed {
        lines.push(head_line);
        k
    } else {
        lines.push(open.clone());
        0
    };

    for i in start..laid.len() {
        // A blank argument carries no text, so giving it a line of its own
        // leaves a stranded `,`. Hang it off the previous line instead:
        // `IF(\n  d = \"\", ,\n  SUMPRODUCT(...)\n)`. A *final* blank
        // argument appends nothing at all — the line already ends with the
        // previous argument's separator, which re-parses to the same blank,
        // and the joining space would be trailing whitespace for editors to
        // strip and re-add forever.
        if g.args[i].is_empty() {
            if let Some(prev) = lines.last_mut() {
                if i + 1 < laid.len() {
                    if !prev.ends_with(&g.open.text) {
                        prev.push(' ');
                    }
                    prev.push_str(sep_after(i));
                }
                continue;
            }
        }
        let mut block = laid[i].clone();
        block[0] = format!("{}{}", ind(arg_indent), block[0]);
        if i + 1 < laid.len() {
            let last = block.len() - 1;
            block[last].push_str(sep_after(i));
        }
        // `i > 0`, not `lines.len() > 1`: when leading arguments were packed
        // onto the opening line, `lines` still holds only that line, and an
        // authored blank before the next argument was being dropped.
        if i > 0 && first_token(&g.args[i]).is_some_and(|t| t.blank_before) {
            lines.push(String::new());
        }
        lines.extend(block);
    }

    lines.push(format!("{}{}", ind(indent), g.close.text));
    lines
}

/// One (key, value) pair per line, values column-aligned, with any blank
/// lines the author wrote between logical groups preserved. A value too big
/// to sit beside its key takes the line below instead, one step in — see
/// [`pair_value_col`].
fn layout_pairs(g: &Group, open: &str, lead: usize, indent: usize, width: usize) -> Vec<String> {
    let arg_indent = indent + INDENT;
    let n = g.args.len();
    let pair_count = (n - lead) / 2;
    let has_tail = (n - lead) % 2 == 1;

    let key_of = |p: usize| lead + p * 2;
    let keys: Vec<String> = (0..pair_count)
        .map(|p| render_inline(&g.args[key_of(p)], false).0)
        .collect();

    let pk = PairKeys::new(&keys, arg_indent, width);

    // The separator that followed argument `i` in the source. Locales that
    // use `;` instead of `,` must round-trip untouched — substituting one
    // for the other is not a whitespace change.
    let sep_after = |i: usize| g.seps.get(i).map_or(",", |t| t.text.as_str());

    let mut lines = vec![open.to_string()];

    // Emit one logical element, re-attaching its source separator unless it
    // closes the call.
    let emit = |lines: &mut Vec<String>, mut block: Vec<String>, items: &[Node], sep: &str| {
        if !sep.is_empty() {
            let i = block.len() - 1;
            block[i].push_str(sep);
        }
        if lines.len() > 1 && first_token(items).is_some_and(|t| t.blank_before) {
            lines.push(String::new());
        }
        lines.extend(block);
    };

    for i in 0..lead {
        let sep = sep_after(i);
        let mut block = layout_items(&g.args[i], arg_indent, arg_indent, width, false, cols(sep));
        block[0] = format!("{}{}", ind(arg_indent), block[0]);
        emit(&mut lines, block, &g.args[i], sep);
    }

    for p in 0..pair_count {
        let ki = key_of(p);
        let vi = ki + 1;
        let key = &keys[p];
        let key_sep = sep_after(ki);
        let aligned_col = pk.value_col(p);
        let last = !has_tail && p + 1 == pair_count;
        let value_sep = if last { "" } else { sep_after(vi) };
        let col = pair_value_col(
            &g.args[vi],
            aligned_col,
            arg_indent,
            width,
            cols(value_sep),
            key_line_width(key, arg_indent, cols(key_sep)),
        );

        let mut block = layout_items(&g.args[vi], col, col, width, false, cols(value_sep));
        if col < aligned_col {
            block[0] = format!("{}{}", ind(col), block[0]);
            block.insert(0, format!("{}{key}{key_sep}", ind(arg_indent)));
        } else {
            let pad = col.saturating_sub(pk.ends[p] + cols(key_sep)).max(1);
            block[0] = format!("{}{key}{key_sep}{}{}", ind(arg_indent), ind(pad), block[0]);
        }
        emit(&mut lines, block, &g.args[ki], value_sep);
    }

    if has_tail {
        let i = n - 1;
        let mut block = layout_items(&g.args[i], arg_indent, arg_indent, width, false, 0);
        block[0] = format!("{}{}", ind(arg_indent), block[0]);
        emit(&mut lines, block, &g.args[i], "");
    }

    lines.push(format!("{}{}", ind(indent), g.close.text));
    lines
}

// ────────────────────────────────────────────────────────────── public ──

fn split_leading_eq(toks: &[Token]) -> (bool, &[Token]) {
    match toks.first() {
        Some(t) if t.kind == Kind::Op && t.text == "=" => (true, &toks[1..]),
        _ => (false, toks),
    }
}

/// Format a formula across as many lines as readability wants.
///
/// Only whitespace between tokens changes; token text is copied verbatim,
/// with two sanctioned exceptions — in the dot locale, `;` argument
/// separators normalize to `,` (see `normalize_separators`), and under
/// [`Options::uppercase_functions`] call heads are uppercased (see
/// `uppercase_function_heads`). A newline inside a string literal is
/// content and survives untouched. A leading UTF-8 BOM is a file-encoding
/// artifact, not a token, and is dropped.
///
/// # Errors
///
/// Returns an error if the formula cannot be tokenized or parsed.
pub fn format(src: &str, opts: &Options) -> Result<String, Error> {
    let src = strip_bom(src);
    let width = opts.width;
    if src.trim().is_empty() {
        return Ok(String::new());
    }
    let toks = tokenize(src, opts.decimal)?;
    let (eq, rest) = split_leading_eq(&toks);
    let mut items = parse(rest)?;
    if opts.decimal == Decimal::Dot {
        normalize_separators(&mut items);
    }
    mark_bound_heads(&mut items, &mut Vec::new());
    if opts.uppercase_functions {
        uppercase_function_heads(&mut items);
    }

    let lead = usize::from(eq);
    let inline = render_inline(&items, false).0;
    let lines = if !contains_authored_grouping(&items) && lead + cols(&inline) <= width {
        vec![inline]
    } else {
        // Line 0 starts after the `=`, which is a column but not indent.
        layout_items(&items, lead, 0, width, true, 0)
    };

    let mut out = String::new();
    if eq {
        out.push('=');
    }
    // Trailing whitespace on a line is never a token (nothing follows it on
    // that line), so trimming it is a pure whitespace change — and it keeps
    // pair layouts with blank values (`LET(a, 1, x,)` pads the missing
    // value) from emitting lines that end in spaces.
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out.push('\n');
    Ok(out)
}

/// Collapse a formula's layout onto a single line.
///
/// A newline inside a string literal is content, not layout, and is
/// preserved — such a formula minifies to more than one physical line.
/// Dot-locale `;` argument separators normalize to `,` here too, a
/// leading UTF-8 BOM is dropped here too, and
/// [`Options::uppercase_functions`] applies here too, so `minify` and
/// [`format()`] always agree on token text.
///
/// # Errors
///
/// Returns an error if the formula cannot be tokenized or parsed.
pub fn minify(src: &str, opts: &Options) -> Result<String, Error> {
    let src = strip_bom(src);
    if src.trim().is_empty() {
        return Ok(String::new());
    }
    let toks = tokenize(src, opts.decimal)?;
    let (eq, rest) = split_leading_eq(&toks);
    let mut items = parse(rest)?;
    if opts.decimal == Decimal::Dot {
        normalize_separators(&mut items);
    }
    mark_bound_heads(&mut items, &mut Vec::new());
    if opts.uppercase_functions {
        uppercase_function_heads(&mut items);
    }

    let mut out = String::new();
    if eq {
        out.push('=');
    }
    out.push_str(&render_inline(&items, true).0);
    out.push('\n');
    Ok(out)
}
