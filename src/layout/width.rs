//! Column arithmetic: how wide a rendered fragment is once placed.

pub(crate) fn ind(n: usize) -> String {
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
pub(crate) fn cols(s: &str) -> usize {
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
pub(crate) fn emitted_widest(col: usize, s: &str) -> usize {
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
pub(crate) fn emitted_last(col: usize, s: &str) -> usize {
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
pub(crate) fn emitted_span(col: usize, s: &str, pending: usize) -> usize {
    emitted_widest(col, s).max(emitted_last(col, s) + pending)
}
