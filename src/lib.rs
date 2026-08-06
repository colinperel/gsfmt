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
//! One sanctioned exception: in the dot locale, `;` argument separators
//! are rewritten to `,` — exactly what the Sheets editor itself does on
//! entry (see [`normalize_separators`]). Array row separators are
//! semantic and stay untouched.

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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            decimal: Decimal::default(),
        }
    }
}

/// Indent added per nesting level.
const INDENT: usize = 2;

/// Give up on column-aligning `LET`/`IFS`/`SWITCH` values once the widest
/// key would push the value column this far right; fall back to a single
/// space so one long name cannot shove every value off the screen.
const ALIGN_MAX: usize = 40;

/// Groups nested deeper than this are rejected at parse time. Real formulas
/// nest shallowly — the fixtures peak around ten levels, the deepest
/// regression test uses forty. The cap exists because parsing recurses per
/// level (unbounded depth overflows the stack around ten thousand) and
/// layout cost grows super-linearly with depth, so a hostile paren tower
/// could hang an editor on save. Two hundred keeps the worst case well
/// under half a second.
const MAX_DEPTH: usize = 200;

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
/// bracket, or nesting deeper than [`MAX_DEPTH`] levels.
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

// ────────────────────────────────────────────────────── inline render ──

/// Operators printed with no surrounding space. `:` and `!` are part of a
/// reference (`B6:B`, `stock_data!P3`); `^` follows spreadsheet idiom (`9^99`).
fn is_tight(op: &str) -> bool {
    matches!(op, ":" | "!" | "^")
}

/// In minified output, keep `<` `=` etc. from fusing into a different token.
fn guard_minified(out: &mut String, next: &str) {
    let (Some(l), Some(r)) = (out.chars().last(), next.chars().next()) else {
        return;
    };
    if matches!(l, '<' | '>') && matches!(r, '=' | '>') {
        out.push(' ');
    }
}

/// Render a sequence on one line, also reporting where each item starts.
/// The reported offsets are *byte* offsets into the returned string — meant
/// for slicing; measure the slices with [`cols`] for column arithmetic.
fn render_inline(items: &[Node], minify: bool) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut offs = Vec::with_capacity(items.len());
    let mut prev_operand = false;
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
                    pending_space = false;
                } else if is_tight(op) {
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
                // quote. Sheets rejects such input (it has no space-
                // intersection operator), so this is preservation of
                // garbage, not endorsement. Groups stay glued to whatever
                // precedes them: `LAMBDA(x, x)(1000)` is a real invocation.
                let leaf_operand = matches!(node, Node::Leaf(_));
                if pending_space || (leaf_operand && prev_operand) {
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

/// A block's width bound. `widest` is the widest line the block could need;
/// `last` is where its final line ends — the column at which a separator
/// appended by the caller would land. `last <= widest` always holds.
#[derive(Clone, Copy)]
struct MinWidth {
    widest: usize,
    last: usize,
}

impl MinWidth {
    fn flat(w: usize) -> Self {
        Self { widest: w, last: w }
    }

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
fn min_group_width(g: &Group, col: usize) -> MinWidth {
    let head = g.head.as_ref().map_or(0, |h| cols(&h.text)) + cols(&g.open.text);
    let last = col + cols(&g.close.text);
    let widest = (col + head).max(last);
    let arg_indent = col + INDENT;

    // LET/IFS/SWITCH pin each key and its value to the same line, at a column
    // derived from the widest key. Measuring their arguments independently
    // underestimates: it reports a layout the formatter will never emit, and
    // the caller then declines to clamp a body that genuinely does not fit.
    if !g.is_array() {
        if let Some(lead) = pair_lead(&g.name_upper()) {
            if g.args.len() > lead + 1 {
                return MinWidth {
                    widest: widest.max(min_pairs_width(g, lead, arg_indent)),
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
        let mut end_cols = if idx + 1 < n { sep_len(g, idx) } else { 0 };
        let mut j = idx + 1;
        while j < n && g.args[j].is_empty() {
            if j + 1 < n {
                end_cols += 1 + sep_len(g, j);
            }
            j += 1;
        }
        widest = widest.max(min_items_width(&g.args[idx], arg_indent).with_sep(end_cols));
        idx = j;
    }
    MinWidth { widest, last }
}

/// Columns the separator after argument `i` occupies, mirroring the
/// `sep_after` closures in the layout functions.
fn sep_len(g: &Group, i: usize) -> usize {
    g.seps.get(i).map_or(1, |t| cols(&t.text))
}

/// Minimum width of a pair-aligned body, mirroring `layout_pairs`' columns —
/// including the separator each lead argument and non-final pair carries.
fn min_pairs_width(g: &Group, lead: usize, arg_indent: usize) -> usize {
    let n = g.args.len();
    let pair_count = (n - lead) / 2;
    let has_tail = (n - lead) % 2 == 1;
    let key_of = |p: usize| lead + p * 2;

    let keys: Vec<String> = (0..pair_count)
        .map(|p| render_inline(&g.args[key_of(p)], false).0)
        .collect();
    let widest_key = keys.iter().map(|k| cols(k)).max().unwrap_or(0);
    let aligned = widest_key + 2 <= ALIGN_MAX;

    let mut widest = arg_indent;
    for i in 0..lead {
        widest = widest.max(min_items_width(&g.args[i], arg_indent).with_sep(sep_len(g, i)));
    }
    for p in 0..pair_count {
        let ki = key_of(p);
        let vi = ki + 1;
        let value_col = if aligned {
            arg_indent + widest_key + 2
        } else {
            arg_indent + cols(&keys[p]) + 2
        };
        let sep = if p + 1 == pair_count && !has_tail {
            0
        } else {
            sep_len(g, vi)
        };
        widest = widest.max(min_items_width(&g.args[vi], value_col).with_sep(sep));
    }
    if has_tail {
        widest = widest.max(min_items_width(&g.args[n - 1], arg_indent).widest);
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
fn min_items_width(items: &[Node], col: usize) -> MinWidth {
    let ops = binary_op_positions(items);
    let Some(&first) = ops.first() else {
        return min_chunk_width(items, col);
    };

    // Mirror the chain layout in `layout_items` exactly: the first chunk
    // sits at `col`, and every operator starts a continuation line indented
    // one step, with its operand beginning after the operator and a space.
    // Measuring a chunk *from* its operator at the original column instead
    // renders a leading `+` as a unary sign — no space, no continuation
    // indent — and the bound comes in short, skipping a clamp the emitted
    // line needed.
    let cont = col + INDENT;
    let mut widest = min_chunk_width(&items[..first], col).widest;
    let mut last = 0;
    for (n, &start) in ops.iter().enumerate() {
        let end = ops.get(n + 1).copied().unwrap_or(items.len());
        let Node::Leaf(op) = &items[start] else {
            unreachable!("op position is a leaf")
        };
        let operand_col = cont + cols(&op.text) + 1;
        let m = min_chunk_width(&items[start + 1..end], operand_col);
        widest = widest.max(m.widest);
        last = m.last;
    }
    MinWidth { widest, last }
}

/// Narrowest width for one unbreakable run. Its leading tokens must all sit
/// on one line; only a trailing group can open out, and at best its body is
/// clamped back to `col + INDENT`. Anything after that group rides on its
/// closing line, so it widens both the bound and the final-line column.
fn min_chunk_width(items: &[Node], col: usize) -> MinWidth {
    let (inline, offs) = render_inline(items, false);
    let Some(gi) = items.iter().rposition(|n| matches!(n, Node::Group(_))) else {
        return MinWidth::flat(col + cols(&inline));
    };
    let Node::Group(g) = &items[gi] else {
        unreachable!("rposition matched a group")
    };
    let head = g.head.as_ref().map_or(0, |h| cols(&h.text)) + cols(&g.open.text);
    // `offs` and the rendered group length are byte offsets: slice with
    // them, then measure the slices in columns.
    let glen = render_group_inline(g, false).len();
    let prefix = cols(&inline[..offs[gi]]);
    let suffix = cols(&inline[offs[gi] + glen..]);
    // The opening line is unavoidable; the body can at best be clamped.
    let body = min_group_width(g, col + INDENT);
    let last = body.last + suffix;
    MinWidth {
        widest: (col + prefix + head).max(body.widest).max(last),
        last,
    }
}

/// `LET`/`IFS`/`SWITCH` bind (key, value) pairs that read best one pair per
/// line with the values column-aligned. Returns how many leading arguments
/// sit alone before the pairs begin.
fn pair_lead(name_upper: &str) -> Option<usize> {
    match name_upper {
        "LET" | "IFS" => Some(0),
        "SWITCH" => Some(1),
        _ => None,
    }
}

/// `LET` always breaks, however short it is. It exists to name intermediate
/// values for readability, so collapsing one onto a single line defeats the
/// reason it was written. Ordinary short calls (`SUM(a, b)`) still stay inline.
fn always_breaks(g: &Group) -> bool {
    g.name_upper() == "LET" && g.args.len() >= 3
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
/// trailing a dangling close paren.
fn force_arg_break(g: &Group, arg_index: usize) -> bool {
    g.name_upper() == "LAMBDA" && g.args.len() > 1 && arg_index + 1 == g.args.len()
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
/// Known one-column gap, accepted in PR #64 review: the inline fit test
/// below cannot see a separator the *caller* will append to this block's
/// last line, so an argument landing exactly at `width` stays inline and
/// the comma lands one column past it. Fixing it means threading the
/// pending-separator width into every fit decision; do that if it ever
/// bites in practice.
fn layout_items(
    items: &[Node],
    start_col: usize,
    indent: usize,
    width: usize,
    force: bool,
) -> Vec<String> {
    let (inline, offs) = render_inline(items, false);
    if !force && !contains_authored_grouping(items) && start_col + cols(&inline) <= width {
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
    if start_col + cols(&inline) > width && !ops.is_empty() {
        let cont = indent + INDENT;
        let mut lines = layout_items(&items[..ops[0]], start_col, indent, width, false);
        for (n, &start) in ops.iter().enumerate() {
            let end = ops.get(n + 1).copied().unwrap_or(items.len());
            let Node::Leaf(op) = &items[start] else {
                unreachable!("op position is a leaf")
            };
            let operand = &items[start + 1..end];
            let head = format!("{}{} ", ind(cont), op.text);
            let mut block = layout_items(operand, cols(&head), cols(&head), width, false);
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
    // Whether to clamp is decided from the group's *minimum* achievable
    // width at each candidate column — the width it would occupy if every
    // breakable construct broke. That is a single non-branching walk of the
    // subtree, so the decision costs O(nodes).
    //
    // Laying both candidates out and measuring them is the obvious
    // alternative and is exponential: each trial recurses into nested
    // prefixed groups that each run their own two trials, doubling per
    // level. It reached ~5s on a depth-20 formula of 624 bytes, which is an
    // editor freeze on save.
    // The group's opening line begins at `group_col` (which includes the
    // `=` column at top level); its body indents from `indent` (which does
    // not — `=` is a column, not indentation, and the close bracket must
    // stay on the indent grid).
    let group_col = start_col + cols(&prefix);
    let natural_body = indent + cols(&prefix);
    let clamped = indent + INDENT;
    let natural = min_group_width(g, group_col).widest;
    let body = if clamped < natural_body
        && natural > width
        && min_group_width(g, clamped).widest < natural
    {
        clamped
    } else {
        natural_body
    };
    let mut lines = layout_group(g, group_col, body, width, force);
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
) -> Vec<String> {
    let inline = render_group_inline(g, false);
    if !force && !group_forces_break(g) && start_col + cols(&inline) <= width {
        return vec![inline];
    }
    if g.is_empty_call() {
        return vec![inline];
    }

    let head = g.head.as_ref().map_or(String::new(), |h| h.text.clone());
    let open = format!("{head}{}", g.open.text);
    let arg_indent = indent + INDENT;

    if !g.is_array() {
        if let Some(lead) = pair_lead(&g.name_upper()) {
            if g.args.len() > lead + 1 {
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
                layout_items(a, arg_indent, arg_indent, width, force_arg_break(g, i))
            }
        })
        .collect();

    let sep_after = |i: usize| -> &str { g.seps.get(i).map_or(",", |t| t.text.as_str()) };

    let mut lines: Vec<String> = Vec::new();
    let first_multi = laid.iter().position(|l| l.len() > 1);

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
/// lines the author wrote between logical groups preserved.
fn layout_pairs(g: &Group, open: &str, lead: usize, indent: usize, width: usize) -> Vec<String> {
    let arg_indent = indent + INDENT;
    let n = g.args.len();
    let pair_count = (n - lead) / 2;
    let has_tail = (n - lead) % 2 == 1;

    let key_of = |p: usize| lead + p * 2;
    let keys: Vec<String> = (0..pair_count)
        .map(|p| render_inline(&g.args[key_of(p)], false).0)
        .collect();

    let widest = keys.iter().map(|k| cols(k)).max().unwrap_or(0);
    let aligned = widest + 2 <= ALIGN_MAX;
    let value_col = arg_indent + if aligned { widest + 2 } else { 0 };

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
        let mut block = layout_items(&g.args[i], arg_indent, arg_indent, width, false);
        block[0] = format!("{}{}", ind(arg_indent), block[0]);
        emit(&mut lines, block, &g.args[i], sep_after(i));
    }

    for p in 0..pair_count {
        let ki = key_of(p);
        let vi = ki + 1;
        let key = &keys[p];
        let key_sep = sep_after(ki);
        let col = if aligned {
            value_col
        } else {
            arg_indent + cols(key) + 2
        };
        let pad = col
            .saturating_sub(arg_indent + cols(key) + cols(key_sep))
            .max(1);

        let mut block = layout_items(&g.args[vi], col, col, width, false);
        block[0] = format!("{}{key}{key_sep}{}{}", ind(arg_indent), ind(pad), block[0]);
        let last = !has_tail && p + 1 == pair_count;
        emit(
            &mut lines,
            block,
            &g.args[ki],
            if last { "" } else { sep_after(vi) },
        );
    }

    if has_tail {
        let i = n - 1;
        let mut block = layout_items(&g.args[i], arg_indent, arg_indent, width, false);
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
/// Only whitespace between tokens changes; token text is copied verbatim.
///
/// # Errors
///
/// Returns an error if the formula cannot be tokenized or parsed.
pub fn format(src: &str, opts: &Options) -> Result<String, Error> {
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

    let lead = usize::from(eq);
    let inline = render_inline(&items, false).0;
    let lines = if !contains_authored_grouping(&items) && lead + cols(&inline) <= width {
        vec![inline]
    } else {
        // Line 0 starts after the `=`, which is a column but not indent.
        layout_items(&items, lead, 0, width, true)
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

/// Collapse a formula onto a single line.
///
/// # Errors
///
/// Returns an error if the formula cannot be tokenized or parsed.
pub fn minify(src: &str, opts: &Options) -> Result<String, Error> {
    if src.trim().is_empty() {
        return Ok(String::new());
    }
    let toks = tokenize(src, opts.decimal)?;
    let (eq, rest) = split_leading_eq(&toks);
    let mut items = parse(rest)?;
    if opts.decimal == Decimal::Dot {
        normalize_separators(&mut items);
    }

    let mut out = String::new();
    if eq {
        out.push('=');
    }
    out.push_str(&render_inline(&items, true).0);
    out.push('\n');
    Ok(out)
}
