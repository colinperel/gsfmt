use crate::parse::{Group, Node};
use crate::token::Kind;

// ────────────────────────────────────────────────────── inline render ──

/// Operators printed with no surrounding space. `:` and `!` are part of a
/// reference (`B6:B`, `stock_data!P3`); `^` follows spreadsheet idiom (`9^99`).
pub(crate) fn is_tight(op: &str) -> bool {
    matches!(op, ":" | "!" | "^")
}

/// True when `out` ends inside an unterminated error literal — a `#`
/// followed only by characters its lexer scan consumes (alphanumerics,
/// `_`, `/`, `.`). `#N/A` qualifies; `#REF!` does not (the `!` closed it).
/// Appending another consumable character to one (`#N/A/A1`) would re-lex
/// as a single, different token.
pub(crate) fn ends_in_open_error_literal(out: &str) -> bool {
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
pub(crate) fn guard_minified(out: &mut String, next: &str) {
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
pub(crate) fn strip_bom(src: &str) -> &str {
    src.strip_prefix('\u{feff}').unwrap_or(src)
}

/// Render a sequence on one line, also reporting where each item starts.
/// The reported offsets are *byte* offsets into the returned string — meant
/// for slicing; measure the slices with [`cols`] for column arithmetic.
pub(crate) fn render_inline(items: &[Node], minify: bool) -> (String, Vec<usize>) {
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

pub(crate) fn render_group_inline(g: &Group, minify: bool) -> String {
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
