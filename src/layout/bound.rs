//! The width bound — a second model of what `layout` will emit.
//!
//! Every disagreement between this and the renderer has been a defect; see
//! `LAYOUT-IR-PLAN.md` for the plan to remove it.

use crate::layout::core::{binary_op_positions, sep_len, trailing_cols};
use crate::layout::pairs::{key_line_width, pair_lead, PairKeys};
use crate::layout::width::{cols, emitted_last, emitted_widest};
use crate::parse::{Group, Node};
use crate::render::{render_group_inline, render_inline};
use crate::INDENT;

/// A block's width bound. `widest` is the widest line the block could need;
/// `last` is where its final line ends — the column at which a separator
/// appended by the caller would land. `last <= widest` always holds.
#[derive(Clone, Copy)]
pub(crate) struct MinWidth {
    pub(crate) widest: usize,
    pub(crate) last: usize,
}

impl MinWidth {
    /// Widest line once `sep` columns are appended to the final line.
    pub(crate) fn with_sep(self, sep: usize) -> usize {
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
/// [`crate::layout::pairs::pair_value_col`] — depends on it, and the bound
/// has to model the
/// layout the formatter will actually emit at *this* width.
pub(crate) fn min_group_width(g: &Group, col: usize, width: usize) -> MinWidth {
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

/// Minimum width of a pair-aligned body, mirroring `layout_pairs`' columns —
/// including the separator each lead argument and non-final pair carries,
/// and the key line a hanging value leaves behind.
pub(crate) fn min_pairs_width(g: &Group, lead: usize, arg_indent: usize, width: usize) -> usize {
    let n = g.args.len();
    let pair_count = (n - lead) / 2;
    let has_tail = (n - lead) % 2 == 1;
    let key_of = |p: usize| lead + p * 2;

    let keys: Vec<String> = (0..pair_count)
        .map(|p| render_inline(&g.args[key_of(p)], false).0)
        .collect();
    let pk = PairKeys::new(g, lead, &keys, arg_indent, width);

    let mut widest = arg_indent;
    for i in 0..lead {
        widest = widest.max(min_items_width(&g.args[i], arg_indent, width).with_sep(sep_len(g, i)));
    }
    for p in 0..pair_count {
        let ki = key_of(p);
        let vi = ki + 1;
        let sep = if p + 1 == pair_count && !has_tail {
            0
        } else {
            sep_len(g, vi)
        };
        // Read the decision rather than repeating it: `PairKeys` made it
        // against the gutter it offered, and asking again here — against the
        // narrowed one — is how the bound and the layout come to disagree.
        let col = pk.value_col(p);
        // A hanging value leaves its key alone on the line above, carrying
        // only the separator that follows it.
        if pk.hangs(p) {
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
pub(crate) fn min_items_width(items: &[Node], col: usize, width: usize) -> MinWidth {
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
pub(crate) fn min_chunk_width(items: &[Node], col: usize, width: usize) -> MinWidth {
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
