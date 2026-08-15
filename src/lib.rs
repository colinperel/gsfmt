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
    pub(crate) fn mark(self) -> char {
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
pub(crate) const INDENT: usize = 2;

/// Hard ceiling on how far right the widest key may push the shared value
/// column of a `LET`/`IFS`/`SWITCH`, so one long name cannot shove every
/// value off the screen. See [`pairs_align`], which also scales the cap to
/// the window.
pub(crate) const ALIGN_MAX: usize = 40;

/// Groups nested deeper than this are rejected at parse time. Real formulas
/// nest shallowly — the fixtures peak around ten levels, the deepest
/// regression test uses forty. The cap exists because parsing recurses per
/// level (unbounded depth overflows the stack around ten thousand) and
/// layout cost grows super-linearly with depth, so a hostile paren tower
/// could hang an editor on save. The worst measured shape is pair layout
/// (nested `LET`), whose cost is roughly cubic in depth: ~0.45s at depth
/// 100 in a release build, ~4s at 199. One hundred keeps that worst case
/// under half a second.
pub(crate) const MAX_DEPTH: usize = 100;

use crate::layout::core::{contains_authored_grouping, layout_items};
use crate::layout::width::cols;
use crate::parse::{mark_bound_heads, normalize_separators, uppercase_function_heads};
use crate::render::{render_inline, strip_bom};

mod error;
mod layout;
mod parse;
mod render;
mod token;

pub use error::Error;
pub use parse::{parse, Group, Node};
pub use token::{tokenize, Kind, Token};

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
