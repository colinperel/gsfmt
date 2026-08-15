//! `LET`/`IFS`/`SWITCH` pair layout: the aligned gutter, the hanging value,
//! and the choice between pairs and the plain per-argument layout.

use crate::layout::bound::{min_items_width, min_pairs_width};
use crate::layout::core::{
    binary_op_positions, contains_authored_grouping, group_stays_whole, layout_items, sep_len,
};
use crate::layout::width::{cols, emitted_last, emitted_span, emitted_widest, ind};
use crate::parse::{first_token, Group, Node};
use crate::render::{render_group_inline, render_inline};
use crate::{ALIGN_MAX, INDENT};

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
pub(crate) struct PairKeys {
    ends: Vec<usize>,
    aligned: bool,
    shared_col: usize,
}

impl PairKeys {
    pub(crate) fn new(keys: &[String], arg_indent: usize, width: usize) -> Self {
        let ends: Vec<usize> = keys.iter().map(|k| emitted_last(arg_indent, k)).collect();
        let widest = ends.iter().copied().max().unwrap_or(arg_indent);
        let aligned = pairs_align(widest.saturating_sub(arg_indent), arg_indent, width);
        Self {
            ends,
            aligned,
            shared_col: widest + 2,
        }
    }

    pub(crate) fn value_col(&self, p: usize) -> usize {
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
pub(crate) fn key_line_width(key: &str, arg_indent: usize, sep: usize) -> usize {
    emitted_span(arg_indent, key, sep)
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
pub(crate) fn pairs_align(widest_key: usize, arg_indent: usize, width: usize) -> bool {
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
pub(crate) fn stays_inline(
    value: &[Node],
    inline: &str,
    col: usize,
    width: usize,
    pending: usize,
) -> bool {
    if contains_authored_grouping(value) {
        return false;
    }
    let renderer_keeps_it =
        col + cols(inline) <= width && emitted_last(col, inline) + pending <= width;
    renderer_keeps_it
        || (!will_expand(value, col, width, pending) && emitted_span(col, inline, pending) <= width)
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
pub(crate) fn pair_value_col(
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
pub(crate) fn is_breakable(items: &[Node]) -> bool {
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
pub(crate) fn has_openable_group(items: &[Node]) -> bool {
    items
        .iter()
        .any(|n| matches!(n, Node::Group(g) if !g.is_empty_call()))
}

/// Whether `layout_items` will expand this run once its inline shortcut has
/// declined — the question [`stays_inline`] needs, and one the shape of the
/// run cannot answer.
///
/// Two things had to stop being guessed here. Operators do not imply
/// expansion: a chain breaks only under real width pressure measured on
/// physical lines, so one whose lines already fit is emitted as rendered.
/// Nor does holding a *group* — `layout_group` returns a group whole
/// whenever it fits at the column its prefix leaves it at, and always for
/// an empty call. Reading "holds a group" as "will expand" hung values the
/// renderer would have kept beside their key.
///
/// So ask the renderer's own question, through the function it uses:
/// [`group_stays_whole`]. There is no second copy of the condition to
/// drift, which is how every other predicate in this module went wrong.
pub(crate) fn will_expand(items: &[Node], col: usize, width: usize, pending: usize) -> bool {
    let Some(gi) = items.iter().rposition(|n| matches!(n, Node::Group(_))) else {
        return false;
    };
    let Node::Group(g) = &items[gi] else {
        unreachable!("rposition matched a group")
    };
    let (inline, offs) = render_inline(items, false);
    let glen = render_group_inline(g, false).len();
    // The group opens where the prefix's final line ends, and whatever
    // follows rides its closing line — the columns `layout_items` uses.
    let group_col = emitted_last(col, &inline[..offs[gi]]);
    let suffix = cols(&inline[offs[gi] + glen..]);
    !group_stays_whole(g, group_col, width, false, suffix + pending)
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
pub(crate) fn pair_lead(name_upper: &str) -> Option<usize> {
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
pub(crate) fn pair_layout_fits(g: &Group, lead: usize, arg_indent: usize, width: usize) -> bool {
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

/// One (key, value) pair per line, values column-aligned, with any blank
/// lines the author wrote between logical groups preserved. A value too big
/// to sit beside its key takes the line below instead, one step in — see
/// [`pair_value_col`].
pub(crate) fn layout_pairs(
    g: &Group,
    open: &str,
    lead: usize,
    indent: usize,
    width: usize,
) -> Vec<String> {
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
