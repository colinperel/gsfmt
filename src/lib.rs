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

use std::fmt;

/// Default target line width. See `dot_config/gsfmt/config` for why 82.
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
    /// Number, name, cell/range reference, `'quoted sheet'`, or `#REF!`.
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
/// Returns an error for unbalanced or mismatched brackets, or a stray
/// closing bracket.
pub fn parse(toks: &[Token]) -> Result<Vec<Node>, Error> {
    let (items, next) = parse_items(toks, 0)?;
    if next < toks.len() {
        let t = &toks[next];
        return err(format!("unexpected {:?}", t.text), t.pos);
    }
    Ok(items)
}

fn parse_items(toks: &[Token], mut i: usize) -> Result<(Vec<Node>, usize), Error> {
    let mut out = Vec::new();
    while i < toks.len() {
        match toks[i].kind {
            Kind::Sep | Kind::Close => break,
            Kind::Open => {
                let (g, ni) = parse_group(toks, i, None)?;
                out.push(Node::Group(g));
                i = ni;
            }
            Kind::Atom
                if i + 1 < toks.len()
                    && toks[i + 1].kind == Kind::Open
                    && toks[i + 1].text == "(" =>
            {
                let (g, ni) = parse_group(toks, i + 1, Some(toks[i].clone()))?;
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
) -> Result<(Group, usize), Error> {
    let open = toks[open_i].clone();
    let closer = if open.text == "(" { ")" } else { "}" };
    let mut args = Vec::new();
    let mut seps = Vec::new();
    let mut i = open_i + 1;

    loop {
        let (items, ni) = parse_items(toks, i)?;
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
                if pending_space {
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

/// Lay out a sequence. Line 0 carries no indent (the caller places it);
/// every later line carries its full absolute indent.
///
/// `force` suppresses only the inline shortcut at *this* level.
fn layout_items(items: &[Node], indent: usize, width: usize, force: bool) -> Vec<String> {
    let (inline, offs) = render_inline(items, false);
    if !force && !contains_authored_grouping(items) && indent + inline.len() <= width {
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
    if indent + inline.len() > width && !ops.is_empty() {
        let cont = indent + INDENT;
        let mut lines = layout_items(&items[..ops[0]], indent, width, false);
        for (n, &start) in ops.iter().enumerate() {
            let end = ops.get(n + 1).copied().unwrap_or(items.len());
            let Node::Leaf(op) = &items[start] else {
                unreachable!("op position is a leaf")
            };
            let operand = &items[start + 1..end];
            let head = format!("{}{} ", ind(cont), op.text);
            let mut block = layout_items(operand, head.len(), width, false);
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

    let prefix = inline[..offs[gi]].to_string();
    let glen = render_group_inline(g, false).len();
    let suffix = inline[offs[gi] + glen..].to_string();

    // Two different columns, which must not be conflated:
    //
    //   start_col  where the group's opening line actually begins, after the
    //              prefix. Fit decisions must use this or they lie.
    //   body       where its continuation lines and closing bracket sit.
    //              Clamped, because a long prefix (a sheet-qualified range,
    //              say) would otherwise march the body off the right edge.
    //
    // Passing the clamped value as both made an overlong group look like it
    // fitted inline, emitting a line far past the width.
    let start_col = indent + prefix.len();
    let body = if start_col + INDENT <= width {
        start_col
    } else {
        indent + INDENT
    };
    let mut lines = layout_group(g, start_col, body, width, force);
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
    if !force && !group_forces_break(g) && start_col + inline.len() <= width {
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
                layout_items(a, arg_indent, width, force_arg_break(g, i))
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
            (start_col + head_line.len() <= width).then_some((k, head_line))
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
        // `IF(\n  d = \"\", ,\n  SUMPRODUCT(...)\n)`.
        if g.args[i].is_empty() {
            if let Some(prev) = lines.last_mut() {
                if !prev.ends_with(&g.open.text) {
                    prev.push(' ');
                }
                if i + 1 < laid.len() {
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

    let widest = keys.iter().map(String::len).max().unwrap_or(0);
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
        let mut block = layout_items(&g.args[i], arg_indent, width, false);
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
            arg_indent + key.len() + 2
        };
        let pad = col
            .saturating_sub(arg_indent + key.len() + key_sep.len())
            .max(1);

        let mut block = layout_items(&g.args[vi], col, width, false);
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
        let mut block = layout_items(&g.args[i], arg_indent, width, false);
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
    let items = parse(rest)?;

    let lead = usize::from(eq);
    let inline = render_inline(&items, false).0;
    let lines = if !contains_authored_grouping(&items) && lead + inline.len() <= width {
        vec![inline]
    } else {
        layout_items(&items, 0, width, true)
    };

    let mut out = String::new();
    if eq {
        out.push('=');
    }
    out.push_str(&lines.join("\n"));
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
    let items = parse(rest)?;

    let mut out = String::new();
    if eq {
        out.push('=');
    }
    out.push_str(&render_inline(&items, true).0);
    out.push('\n');
    Ok(out)
}
