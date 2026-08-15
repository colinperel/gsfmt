//! Sequence and group layout.

use crate::layout::pairs::{layout_pairs, pair_layout_fits, pair_lead};
use crate::layout::width::{cols, emitted_last, emitted_widest, ind};
use crate::parse::{first_token, Group, Node};
use crate::render::{is_tight, render_group_inline, render_inline};
use crate::token::Kind;
use crate::INDENT;

/// Columns the separator after argument `i` occupies, mirroring the
/// `sep_after` closures in the layout functions.
pub(crate) fn sep_len(g: &Group, i: usize) -> usize {
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
pub(crate) fn trailing_cols(g: &Group, i: usize) -> usize {
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

/// `LET` always breaks, however short it is. It exists to name intermediate
/// values for readability, so collapsing one onto a single line defeats the
/// reason it was written. Ordinary short calls (`SUM(a, b)`) still stay
/// inline — including a bound user function that happens to be named `let`.
pub(crate) fn always_breaks(g: &Group) -> bool {
    !g.bound_head && g.name_upper() == "LET" && g.args.len() >= 3
}

/// A blank line between arguments is an explicit grouping the author wrote,
/// and it can only exist in a broken layout. So a group carrying one stays
/// broken even when it would fit on a single line — collapsing it would
/// silently discard that grouping.
pub(crate) fn has_authored_grouping(g: &Group) -> bool {
    g.args
        .iter()
        .skip(1)
        .any(|a| first_token(a).is_some_and(|t| t.blank_before))
}

/// True if this group, or anything nested inside it, carries authored
/// grouping — an enclosing call cannot collapse to one line either.
pub(crate) fn group_forces_break(g: &Group) -> bool {
    always_breaks(g)
        || has_authored_grouping(g)
        || g.args.iter().any(|a| contains_authored_grouping(a))
}

pub(crate) fn contains_authored_grouping(items: &[Node]) -> bool {
    items
        .iter()
        .any(|n| matches!(n, Node::Group(g) if group_forces_break(g)))
}

/// A broken `LAMBDA` always gives its body its own block, even when the body
/// would fit on one line. Keeps `LAMBDA(x, y, IF(...))` readable rather than
/// trailing a dangling close paren. A bound user function named `lambda`
/// has no body argument, so the rule skips it.
pub(crate) fn force_arg_break(g: &Group, arg_index: usize) -> bool {
    !g.bound_head && g.name_upper() == "LAMBDA" && g.args.len() > 1 && arg_index + 1 == g.args.len()
}

/// Positions of the *spaced* binary operators in a flat sequence — the only
/// legal places to break a line.
///
/// Tight operators are excluded on purpose: `:` and `!` are part of a
/// reference (`stock_data!$A$3:INDEX(...)`), `^` follows spreadsheet idiom,
/// and a `%` or a unary sign binds to the operand it modifies. Breaking at
/// any of those would split a single value across lines.
pub(crate) fn binary_op_positions(items: &[Node]) -> Vec<usize> {
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
pub(crate) fn layout_items(
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
pub(crate) fn layout_group(
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
