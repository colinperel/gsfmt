# Replacing the layout engine with a document IR

Scoping document, not executed. Supersedes `BOUND-REFACTOR-PLAN.md`, which
proposed a different fix for the same problem and was tried and abandoned —
read its outcome section before reviving anything from it.

Self-contained: an implementing agent needs this file, the repo, and the
reference implementation named below.

## The problem, stated properly

`src/lib.rs` decides layout twice. The `min_*` family predicts what the
`layout_*` family will emit, and the two are kept in step by hand. Across
PRs #19, #20 and #21 that failed twelve times — every finding was the
prediction disagreeing with the emission.

The first attempt at a fix assumed the answer was to delete the predictor and
lay both candidates out instead. That does not work: laying a candidate out
to completion costs a full traversal, so the fallback path doubles per
nesting level, and the tie-break when neither candidate fits needs a fact
(is this key breakable?) that no width comparison can recover.

Both of those are solved problems. gsfmt is the outlier here, not the hard
case.

## The reference

[`ruff_formatter`](https://github.com/astral-sh/ruff/tree/main/crates/ruff_formatter),
Astral's formatting engine for Ruff — a fork of Rome's `rome_formatter`, now
also Biome's. It descends from
[Wadler's *A prettier printer*](https://homepages.inf.ed.ac.uk/wadler/papers/prettier/prettier.pdf)
via [Lindig's *Strictly Pretty*](https://lindig.github.io/papers/strictly-pretty-2000.pdf),
which is the version that works in a strict language.

It is the right reference rather than Prettier because it is Rust, and
because it has the two things Prettier deliberately refuses and gsfmt needs.

**It has no second width model.** The printer's `FitsMeasurer` walks the same
`FormatElement` stream the printer is about to emit, and stops early:
`Fits::No` when `line_width` is exceeded, `Fits::Yes` at the first hard line
break. Each group decision is bounded lookahead over the real output, not a
re-derivation of it. That single property is what gsfmt is missing.

## The IR subset gsfmt needs

Ruff's element set is larger than this; these are the pieces that earn their
place here.

| element | purpose in gsfmt |
|---|---|
| `Token(&'static str)` | punctuation the formatter owns: `(`, `)`, `,` |
| `SourceSlice(range)` | every leaf, byte-for-byte from the source |
| `Space` | the one between an operator and its operand |
| `Line(Soft)` | break only if the enclosing group breaks |
| `Line(SoftOrSpace)` | a space when flat, a break when the group breaks |
| `Line(Hard)` | `LET` always breaks, however short |
| `Line(Empty)` | a blank line the author wrote between bindings |
| `ExpandParent` | propagate a forced break outward |
| `Group` | the unit of fits-or-breaks |
| `Indent` | one nesting step |
| `IfFlat` | the pair gutter — padding that exists only while the pair is flat |
| `BestFitting{variants}` | pair layout versus the plain per-argument layout |

## Mapping

| gsfmt today | becomes |
|---|---|
| `min_group_width`, `min_items_width`, `min_chunk_width`, `min_pairs_width`, `MinWidth` — 206 lines | deleted; `fits` walks the print stream |
| `pair_layout_fits` strategy choice | `BestFitting{[pairs, plain]}` |
| `pairs_align`, `PairKeys`, the gutter | `IfFlat(spaces(pad))`, the pad computed once before building the IR |
| `pair_value_col` / `stays_inline` | a `Group` around the value; it breaks or it does not |
| `always_breaks` (LET) | `Line(Hard)` inside the group |
| `has_authored_grouping`, `group_forces_break` | `Line(Empty)` + `ExpandParent` |
| `trailing_cols` / `pending` threading | the separator is an element in the stream; measured where it sits |
| `force` flag | flat-mode measurement (`must_be_flat`) |
| `emitted_widest`, `emitted_last`, `emitted_span` | gone — see below |

## What this makes unwriteable

Not "fixes" — unwriteable, which is the point.

**Five of the twelve findings** were the same error: measuring a token whose
text carries a newline as though it were one continuous line. Ruff does not
measure such tokens at all. `TextWidth` is computed once per text element as
either `Width(n)` or `Multiline`; `will_break()` returns true for
`Multiline`, the enclosing group expands, and the width check is skipped
entirely. There is no span-versus-physical question because nothing ever
measures a multiline token.

This also vindicates the one finding rejected during #21's review — keeping
the span check so a formula built around a multi-line `QUERY` string stays
structured. Ruff's rule is the same and stronger: a token containing a
newline forces its group to break, full stop.

**The `pending` threading from #20** disappears. A separator is an element
between two others, so whatever measures the block measures the separator;
there is nothing to thread and nothing to forget to thread.

**The predictor/renderer split from #21** disappears with the predictor.

## Decisions this forces

These are real choices, not implementation detail. Settle them before step 3.

1. **`BestFitting` falls back to the *most expanded* variant when none fit.**
   gsfmt's current rule is the opposite: when neither pairs nor plain fit,
   pairs stay, because "falling back would tear the pair apart without fixing
   the overshoot". Adopting Ruff's convention changes that, and it is exactly
   the case that broke the first attempt. Either order the variants to suit,
   or keep gsfmt's rule and document the divergence.

2. **Byte preservation.** gsfmt's contract is that only whitespace between
   tokens may change. Ruff formats an AST and does not make that promise, so
   every leaf must be a source slice in the IR and no builder may normalise
   it. Ruff's `text()` asserts its input has no newlines — gsfmt's string
   literals do, so gsfmt needs the `SourceSlice`/`Multiline` path, not
   `text()`.

3. **Blank arguments.** `IF(a,,b)` holds an argument with zero tokens and
   must round-trip. It is an empty entry between two separators; confirm that
   survives the IR without becoming `""`.

4. **Comma-locale separators.** `;` must round-trip untouched under
   `Decimal::Comma`. Separators are `Token`s owned by the builder, so the
   builder has to read the locale rather than hard-code `,`.

5. **The multiline rule.** Ruff never measures a token carrying a newline;
   `will_break` is set and the enclosing group expands. That does *not*
   reproduce gsfmt's output — see the spike below. Decide deliberately:
   adopt Ruff's rule and accept the style change, or keep physical-line
   measurement, which is implementable in `fits` alone.

## Migration

Each step keeps `tests/data/` byte-identical. That is the gate: this is a
rewrite of *how* the layout is decided, not of what it produces.

1. `FormatElement`, `TextWidth` (`Width`/`Multiline`), and a `Printer` with
   flat/expanded modes and a bounded-lookahead `fits`. No gsfmt integration.
   Unit-test the printer against hand-built documents.
2. Build the IR for the shapes with no pair layout — calls, arrays, operator
   chains, blank arguments. Print through the new printer behind a flag.
   Gate: `gnarly.gsfx` byte-identical.
3. Pair layout as `BestFitting{[pairs, plain]}` plus `Align`. Gate:
   `monthly.gsfx` and `payperiods.gsfx` byte-identical, and the reporter's
   760-line formula unchanged at width 82.
4. Delete the `min_*` family and the old `layout_*` family. Re-point the
   `the_width_bound_*` tests at emitted geometry; any that only made sense
   against the deleted bound should be said so, not deleted quietly.
5. Width matrix 20–120 over the fixtures and the reporter's formula, diffing
   against today. Every difference explained before it is accepted.

Steps 1–2 are additive and safe. Step 3 is where output can move.

## What not to copy from Ruff

- **Its scale.** Ruff's element set, interning, `fill`, line suffixes and
  verbatim handling exist for Python. Take the subset above.
- **Its AST orientation.** Ruff formats a parsed AST and normalises literals.
  gsfmt's whole thesis is that a formatter must not — see the module header
  on why both mature formula parsers were rejected. Leaves stay raw slices.
- **`unicode_width`.** Ruff depends on it. gsfmt is deliberately
  zero-dependency and approximates with `cols`; keep that, and keep the
  comment explaining the trade.
- **The crate itself.** `ruff_formatter` is not a general-purpose dependency
  and taking it would end gsfmt's zero-dependency property. Implement the
  design; read their printer as the reference.

## Risks

- **Scope.** This replaces roughly 800 lines of layout. It is the largest
  change the repo has had, and it wants its own review loop per step.
- **Output churn at step 3.** The goal is byte-identical goldens, but pair
  layout is where gsfmt has the most bespoke behaviour (hanging, the gutter,
  the fit-based hang policy). Expect to discover that some current output is
  an artifact of the old model rather than a decision worth keeping — and
  decide those one at a time, not in bulk.
- **Performance.** Should improve: bounded lookahead replaces a full bound
  traversal per group. Confirm against the depth-99 chain (896 ms today,
  10s ceiling) and the 760-line formula (8.0 ms) at every step.
- **The temptation to half-migrate.** Two engines behind a flag is the same
  two-models problem in a new costume. Steps 1–3 may land separately; step 4
  must not be deferred indefinitely.

## Spike result

A standalone printer — elements, a `fits` with bounded lookahead, and the
document built by hand — was written and run against real gsfmt output before
touching `src/lib.rs`. Roughly 130 lines for the printer.

**Reproduced byte-for-byte:**

- lines 3-18 of `tests/data/monthly.gsfx`: the aligned gutter, both authored
  blank lines, the hanging `shipments` value, and the nested `FILTER`/`HSTACK`
  groups
- an operator chain breaking with its operators leading the continuation
  lines

**Did not reproduce**, and this is the important one:

    =LET(longkey, "aaaa\nb" & c, z, 1, z)      at width 23

    gsfmt today          spike (Ruff's rule)
      longkey, "aaaa       longkey,
    b" & c,                  "aaaa
                           b"
                             &
                             c

gsfmt keeps a value carrying a newline *beside* its key, because it measures
that value's physical lines (16 and 7, both inside 23). Ruff sets
`will_break` on any multiline token, so the enclosing group expands
unconditionally.

### Two claims above were wrong

**`Align` is not needed, and would be harmful.** The gutter is padding on the
pair's *first* line only — `IfFlat(spaces(pad))`. When the pair breaks there
is no gutter, because the value hangs one indent step in. `Align` would
anchor a broken value's continuation lines at the gutter column, which is
exactly the skinny-tower shape PR #19 removed. Having the primitive would
make the tower expressible again, so gsfmt should not have it. Corrected in
the tables above.

**"Five findings become unwriteable" is too strong.** That holds only under
Ruff's multiline rule, which changes output. Keeping today's behaviour means
`fits` measures multiline text by physical lines — the same arithmetic that
produced those five findings. The difference, and it is still the whole
point, is that there is exactly *one* `fits` rather than five hand-kept
copies. The class collapses from five sites to one, not to zero.

### Still untested

`BestFitting` and the pairs-versus-plain fallback; blank arguments
(`IF(a,,b)`); comma-locale separators; the packed opening line
(`SUMIFS(qC,` with leading simple arguments); and performance against the
depth-99 chain. The fallback case is the one most likely to hold another
surprise, since it is what broke the previous plan.
