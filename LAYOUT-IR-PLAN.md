# Replacing the layout engine with a document IR

Scoping document, not executed. Supersedes `BOUND-REFACTOR-PLAN.md`, which
proposed a different fix for the same problem and was tried and abandoned —
read its outcome section before reviving anything from it.

Self-contained with one exception, called out because it matters for the
gates below: the 760-line production formula referenced throughout is the
one that started this work and is **not in the repository**. It is a user's
own spreadsheet logic, so committing it is their call, not the plan's. Until
it is either committed as an anonymised fixture or replaced, treat every gate
that names it as a local check an implementer cannot reproduce from the repo
alone, and rely on `tests/data/` for the reproducible ones.

Otherwise self-contained: an implementing agent needs this file, the repo, and
the reference implementation named below.

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
| `Text(String, TextWidth)` | every leaf, verbatim or rewritten — see decision 2 |
| `Space` | the one between an operator and its operand |
| `Line(Soft)` | break only if the enclosing group breaks |
| `Line(SoftOrSpace)` | a space when flat, a break when the group breaks |
| `Line(Hard)` | `LET` always breaks, however short |
| `Line(Empty)` | a blank line the author wrote between bindings |
| `ExpandParent` | propagate a forced break outward |
| `Group` | the unit of fits-or-breaks |
| `Indent` | one nesting step |
| `IfFlat` | the pair gutter — padding that exists only while the pair is flat |
| `BestFitting{variants, mode, fallback}` | pair layout versus the plain per-argument layout — mode *and* fallback are load-bearing, see decision 6 |

## Mapping

| gsfmt today | becomes |
|---|---|
| `min_group_width`, `min_items_width`, `min_chunk_width`, `min_pairs_width`, `MinWidth` — 206 lines | deleted; `fits` walks the print stream |
| `pair_layout_fits` strategy choice | `BestFitting{[pairs, plain], AllLines, fallback: 0}` |
| `pairs_align`, `PairKeys`, the gutter | `IfFlat(spaces(pad))`, the pad computed once before building the IR |
| `pair_value_col` / `stays_inline` | a `Group` around the value; it breaks or it does not |
| `always_breaks` (LET) | `Line(Hard)` inside the group |
| `has_authored_grouping`, `group_forces_break` | `Line(Empty)` + `ExpandParent` |
| `trailing_cols` / `pending` threading | the separator is an element in the stream; measured where it sits |
| `force` flag | flat-mode measurement (`must_be_flat`) |
| `emitted_widest`, `emitted_last`, `emitted_span` | one copy, inside `fits` — see the spike |

## What this removes

**Five of the twelve findings** were one error: measuring a token whose text
carries a newline as though it were a single line. Ruff never measures such a
token — `TextWidth` is `Width(n)` or `Multiline`, `will_break()` is true for
`Multiline`, the group expands, the check is skipped — so under Ruff's rule
the error is unwriteable.

The spike showed that rule changes gsfmt's output, so it is a decision rather
than a free win (see "Decisions this forces", item 5, and the spike section).
If gsfmt keeps its current behaviour, the arithmetic stays but lives in one
`fits` instead of five hand-kept copies. Five sites become one either way;
whether it becomes none is the style question.

**The `pending` threading from #20** disappears. A separator is an element
between two others, so whatever measures the block measures the separator;
there is nothing to thread and nothing to forget to thread.

**The predictor/renderer split from #21** disappears with the predictor.

## Decisions this forces

These are real choices, not implementation detail. Settle them before step 3.

1. **The fallback when nothing fits.** gsfmt's rule is that pairs stay,
   because "falling back would tear the pair apart without fixing the
   overshoot", and it is the case that broke the first attempt at this
   rewrite. Ruff's contract is the opposite — the last variant wins,
   unmeasured. See decision 6 for how gsfmt expresses its rule against that
   contract; ordering alone cannot.

2. **One text element, not Ruff's two.** Ruff splits `Text` (owned) from
   `SourceCodeSlice` (borrowed) as a memory optimisation, and its
   `source_text_slice` builder carries `debug_assert_no_newlines`. Neither
   half of that transfers. gsfmt's `Token` already owns its text
   (`pub text: String`), so there is nothing to borrow; and gsfmt's leaves
   include string literals carrying newlines, which that assertion forbids.

   So: a single `Text(String, TextWidth)` element, where `TextWidth` is
   `Width(n)` or `Multiline`, used for every leaf. That also absorbs the two
   sanctioned rewrites, which mutate leaves today — `normalize_separators`
   sets `sep.text = ","` in the dot locale, `uppercase_function_heads` sets
   `h.text = h.text.to_uppercase()` under `--uppercase`. Both need their own
   migration gate: `comma_locale.gsfx` for the first, the uppercase tests for
   the second.

3. **Blank arguments.** `IF(a,,b)` holds an argument with zero tokens and
   must round-trip. It is an empty entry between two separators; confirm that
   survives the IR without becoming `""`.

4. **Comma-locale separators.** `;` must round-trip untouched under
   `Decimal::Comma`. Separators are `Token`s owned by the builder, so the
   builder has to read the locale rather than hard-code `,`.

6. **`BestFitting` needs three variants, not two, and `AllLines`.**

   Ruff prints the *last* variant unconditionally, without measuring it —
   `// No variant fits, take the last (most expanded) as fallback`. So
   `[pairs, plain]` does not preserve today's policy: when neither fits it
   would select plain, where `pair_layout_fits` deliberately keeps pairs.
   Reversing to `[plain, pairs]` breaks the ordinary prefer-pairs case
   instead. Ordering cannot express it with two.

   `[pairs, plain, pairs]` does express it: pairs if pairs fit, else plain
   if plain fits, else pairs unmeasured. But taken literally it is a trap.
   With owned recursive documents the pairs tree is materialised twice at
   every pair group, and a pair value that is itself a `LET` repeats that
   inside its own variants — construction becomes exponential in nesting
   depth, on the same shape `pair_nesting_at_the_cap_stays_fast` exists to
   protect, and it happens before `fits` ever runs.

   Ruff's answer is `Interned` (`Rc<[FormatElement]>`), which exists
   precisely so repeated `BestFitting` content is shared rather than cloned.
   gsfmt can do better than adopt the idiom *and* the machinery it needs:
   carry the fallback as an index instead.

       BestFitting { variants: Vec<Doc>, fallback: usize }

   with `[pairs, plain]` and `fallback: 0`. No repetition, no interning, no
   `Rc`, and the policy reads as what it is — prefer pairs, take plain if it
   fits, otherwise variant 0. The divergence from Ruff is deliberate and
   confined to one field.

   Either way, gate IR *construction* against the depth-99 shape, not just
   printing: this is a cost that lands before any measurement.

   On the mode: Ruff defaults to `BestFittingMode::FirstLine`,
   where a variant is chosen if the content up to its *first* line break
   fits. That is wrong for gsfmt and would silently disable the fallback:
   the pairs variant opens with `LET(` or `SUMIFS(`, which always fits, so
   `FirstLine` selects pairs before an oversized key or value is ever
   measured — and `pair_layout_yields_to_breakable_oversized_keys` would
   fail. gsfmt needs `AllLines`.

   `AllLines` carries its own catch worth designing around: a variant does
   not fit if it contains a hard line break *outside* a group in expanded
   mode. gsfmt's `LET` always breaks, which is a hard break, so the pairs
   variant must place that break inside an expanded group or `AllLines`
   will reject the variant outright. Prove this with a test before relying
   on it.

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
3. Pair layout as `BestFitting{[pairs, plain], AllLines, fallback: 0}`, plus
   the `IfFlat` gutter. Gate in this order, because each catches a
   different half of decision 6:
   `pair_layout_yields_to_breakable_oversized_keys` first — its `SUMIFS`
   case fails on a wrong *mode* (`FirstLine` never reaches the oversized
   key) and its `veryLongBindingNameHere` case fails on a wrong *fallback*
   (two variants would yield plain where the rule keeps pairs) — then
   `monthly.gsfx` and `payperiods.gsfx` byte-identical. (Locally, also the
   760-line formula at width 82; see the note at the top.)
4. Delete the `min_*` family and the old `layout_*` family. Re-point the
   `the_width_bound_*` tests at emitted geometry; any that only made sense
   against the deleted bound should be said so, not deleted quietly.
5. Width matrix 20–120 over `tests/data/`, and over the 760-line formula if
   it is available locally, diffing
   against today. Every difference explained before it is accepted.

Steps 1–2 are additive and safe. Step 3 is where output can move.

## What not to copy from Ruff

- **Its scale.** Ruff's element set, `fill`, line suffixes and verbatim
  handling exist for Python. Take the subset above.
- **Interning.** `Interned`/`Rc<[FormatElement]>` earns its place in Ruff
  because repeated `BestFitting` content is common there. gsfmt has exactly
  one such use, and decision 6 removes it with a fallback index instead. If
  a second use appears, revisit — sharing is the right answer at that point,
  not a third copy.
- **Its AST orientation.** Ruff formats a parsed AST and normalises literals.
  gsfmt's whole thesis is that a formatter must not — see the module header
  on why both mature formula parsers were rejected. Leaf *text* is carried
  through byte-for-byte (in an owned `Text`, per decision 2, since gsfmt's
  tokens already own their strings); what must not be copied is the
  normalisation.
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
  10s ceiling) and, where available, the 760-line formula (8.0 ms) at every
  step.
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
