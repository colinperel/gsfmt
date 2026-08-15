# Changelog

All notable changes to gsfmt. Versions follow [SemVer](https://semver.org);
entries mirror the [GitHub release notes](https://github.com/colinperel/gsfmt/releases).

## v0.8.0 — 2026-08-15

Output-affecting release, and the largest so far. What changes falls into two
groups, and it is worth knowing which applies to you.

Most of it is `LET`-style pair layout — `LET`, `IFS`, `SWITCH`, the `*IFS`
aggregations, `SORT`, `SORTN`, `GETPIVOTDATA`, `AVERAGE.WEIGHTED` — and only
once such a group breaks across lines. Bindings keep their alignment where
they used to lose it, oversized values drop to the line below instead of
marching right, and the alignment gutter stops being set by keys that do not
use it. On a 760-line production formula the result went from 442 lines and
29 columns of indent to 415 and 16.

One change is general: a block that ended exactly at the width now counts the
separator its caller appends, so it breaks where it previously overflowed by
a column. That applies to any argument of any call, not just pair layout —
`IFERROR(SUM(…), fallback)` reformats at the width where its `SUM` landed on
the boundary.

So a formula that fits on one line is untouched, and plenty of multi-line
ones are too — `=SUM(A1:A9)` and an ordinary `LET` whose values all fit
beside their keys are byte-identical to v0.7.1. But you cannot tell by
inspection which side of the second group a given formula falls on. Reformat
in one commit and read the diff: it will be large but mechanical, and
`--minify` output is unchanged, so no formula's meaning moved.

### Behavior change

- **A `LET` value too big to sit beside its key hangs below it** — one indent
  step in, instead of anchoring its whole subtree to the aligned key column.
  Anchoring there indented by horizontal position rather than nesting depth,
  so the wider the widest key the narrower the tower: a 26-column key pushed
  nested values out to column 61. This is the break-after-assignment shape
  every modern formatter uses, and what the reporting formula's author had
  been writing by hand.

- **A pair that does not fit side by side keeps pair layout** rather than
  falling back to the plain per-argument list. That fallback existed because
  pair layout used to pin a key and its value to one line, so falling back
  was the only way to fit; hanging retired the premise. It still fires where
  hanging cannot rescue the pair.

### Fixes

- **The alignment gutter is sized by the keys that use it.** A key whose
  value hangs on the line below never occupies the gutter, but it was still
  setting the width of it, so every other binding in the group paid for
  padding nobody used — a single long name pushed its neighbours eighteen
  columns right. Values only ever move left as a result; no line got wider
  and no overflow count changed.

- **The width bound no longer inflates by one indent step per nesting
  level.** It measured a broken group's body one step deeper than the layout
  emits it, compounding with depth. Three values in a 68-binding `LET`
  measured 83 columns against a width of 82, all of it phantom, and the whole
  group lost its alignment — every key stranded on a line of its own. This is
  the defect that prompted the release.

- **The alignment gutter must fit the window.** The cap on how far right the
  widest key may push the value column was a fixed 40 regardless of
  `--width`, so at narrow widths a long key opened the value column on the
  right margin with nothing beyond it.

- **A block's fit test counts the separator its caller appends.** A block
  ending exactly at the width passed the test and the comma landed one column
  past it — wherever a separator follows a block, which is nearly everywhere.

- **An unbreakable value hangs when that is what makes it fit.** Hanging was
  gated on whether the value could break internally, so a bare `0.55` under a
  long key stayed put and overflowed although the line below fit it easily.

- **A value holding a group is no longer assumed to expand.** A group is
  returned whole whenever it fits at the column its prefix leaves it at, and
  always for an empty call like `NOW()`, so values the layout would have kept
  beside their key were hung for nothing.

- **Tokens carrying a newline are measured by their physical lines** in every
  fit decision, not by the total span of the literal. A `QUERY` string
  spanning four lines was measured as one very long one.

### Internal

- `src/lib.rs` split into modules — `token`, `parse`, `render`, `error`, and
  a `layout` directory. CI's rustdoc job now documents private items, where
  nearly all of this crate's explanation lives.

- A production-scale golden, `tests/data/segments.gsfx`: 68 bindings, a
  26-column widest key, 39 values that hang. Anonymised from the formula that
  reported these defects, with a test asserting the shape it exists to stress.

- `BOUND-REFACTOR-PLAN.md` and `LAYOUT-IR-PLAN.md` record an attempt to remove
  the duplicate width model that failed under measurement, and the design that
  replaces it. Neither ships in the published crate.

## v0.7.1 — 2026-08-11

Distribution release: gsfmt is now on [crates.io](https://crates.io/crates/gsfmt)
(`cargo install gsfmt --locked`) with prebuilt binaries for Linux
(x86_64/aarch64), macOS (Intel/Apple silicon), and Windows attached to
each GitHub release alongside a `SHA256SUMS`. No behavior changes —
formatting output is byte-identical to v0.7.0.

### Infrastructure

- **Tag-triggered release workflow** — a `v*` tag builds all five
  targets, publishes to crates.io only after every binary is green, and
  creates the GitHub release with this file's matching section as the
  body. A guard job refuses a tag that disagrees with Cargo.toml's
  version, and the release step recovers cleanly from a partially
  failed previous run.
- **Registry metadata** — repository/readme/keywords/categories in
  Cargo.toml; CI plumbing and the historical remediation plan are
  excluded from the packaged crate.
- **README** — new Installation section.

## v0.7.0 — 2026-08-11

Feature release: directory inputs and per-project configuration. Both
additive — existing invocations format byte-identically, with one
caveat: a `.gsfmt` file already sitting in an ancestor directory now
silently applies (project-config discovery is new behavior on every
run).

### Features

- **Directory arguments under `--write`** — a FILE that is a directory
  expands to every `.gsfx` file beneath it, recursively and in sorted
  order. Hidden entries are skipped; the walk never follows a symlinked
  directory (a link cycle cannot hang it), though naming one on the
  command line expands it, like `find -H`. Traversal failures are
  reported per path while the rest of the tree still formats, and
  discovered paths stay `PathBuf` end to end so a non-UTF-8 filename
  formats in place rather than through a lossy copy. Without `--write`,
  a directory is rejected up front with a pointer to the flag.

- **Per-project config via `.gsfmt`** — the nearest `.gsfmt` walking up
  from the first FILE (from the current directory when reading stdin)
  supplies settings in the same `key = value` format as the user
  config. Resolution is per key: flag, then `$GSFMT_<KEY>`, then
  project `.gsfmt`, then user config, then the default — so a
  comma-locale spreadsheet's checkout can pin `decimal` in-repo. The
  anchor is canonicalized first: a relative `../sibling` path cannot
  pick up the invocation directory's config.

## v0.6.0 — 2026-08-10

Output-affecting release: broken chain tails now format differently.

### Behavior change

- **Chain-tail bodies are capped at the block indent** — when a prefix
  (an operator chain like `INDEX(…):INDEX(…)`, a unary sign) pushes a
  trailing group right and the group breaks, its body no longer hangs at
  the open bracket's column: it takes at most `indent + INDENT`. Removes
  the skinny right-margin towers deep LET values produced, and gives a
  formula one shape at every width. Corpus goldens were unaffected
  (their chain tails already hit the old overflow clamp).

## v0.5.0 — 2026-08-10

Feature release: in-place formatting and the completed pair-shaped
builtin set. Output-affecting only for broken input of the five newly
pair-shaped functions.

### Feature

- **`--write` / `-i`** — format any number of FILEs in place:
  already-clean files untouched; changed files replaced crash-safely
  via a unique exclusively-created sibling temp carrying the original's
  permission bits (created with the source mode on Unix — no
  world-readable window); per-file error reporting; exit 2 on any parse
  failure, 1 on any I/O failure. Replacement is `sed -i`-like (new
  inode): ownership, ACLs, and extended attributes do not carry.
- **Pair layout for the remaining pair-shaped builtins** —
  `COUNTUNIQUEIFS` and `SORT` (lead 1), `GETPIVOTDATA` (lead 2),
  `SORTN` (lead 3), `AVERAGE.WEIGHTED` (lead 0), completing the
  documented Sheets set.

## v0.4.0 — 2026-08-10

Output-affecting release: broken `*IFS` aggregation calls now format
differently.

### Behavior change

- **Pair layout for `*IFS` aggregation criteria** — a broken
  `SUMIFS`/`AVERAGEIFS`/`MAXIFS`/`MINIFS` puts the aggregated range alone,
  then one (criteria range, criterion) pair per line, values
  column-aligned; `COUNTIFS` is pairs from the first argument. Calls that
  fit stay inline.
- Pair layout yields to the plain per-argument layout when it cannot fit
  but the plain layout can (breakable oversized key, or a pair line that
  overflows while its parts fit alone). A lone unbreakable overshoot
  keeps pairs — width remains a target, not a ceiling.
- All name-driven layout rules follow LET/LAMBDA evaluation-order scope,
  matching the uppercase rewrite: a bound user function named
  `sumifs`/`let`/`lambda` lays out as an ordinary call.

### Library

- `Group` gains a private `bound_head` field — source-breaking for
  exhaustive external `Group` literals.

## v0.3.0 — 2026-08-10

Feature release: opt-in uppercase normalization of function names. Off by
default — existing output is unchanged unless enabled. Source-breaking for
exhaustive `Options` struct literals (new field), hence 0.3.0.

### Feature

- **`--uppercase` / `-u`: rewrite call heads to uppercase** (`sum(` →
  `SUM(`), mirroring what the Sheets editor does to builtin names on
  entry. Also `$GSFMT_UPPERCASE` and config key `uppercase = true|false`,
  resolved flag → env → config → default. New public field
  `Options.uppercase_functions` (default `false`).
- Only call heads are rewritten; arguments, references, and string content
  are untouched. Applied identically in `format` and `minify`.
- Names bound by `LET`/`LAMBDA` keep their authored case, following
  Sheets' evaluation-order scope: a `LET` name is visible only to
  subsequent value expressions and the final expression, a `LAMBDA`
  parameter only to its body, and a bound name shadowing `LET`/`LAMBDA`
  is an ordinary user-function call — it neither uppercases nor binds.
- Bound-name matching uses a dependency-free Unicode case-folding
  approximation (keys ẞ/ß, Kelvin sign, final sigma correctly).

### Fixes

- **Measure multi-line strings by physical line in chain layout** — an
  operator chain interleaved with a multi-line string literal no longer
  breaks when every physical line already fits the width.

## v0.2.0 — 2026-08-05

Output-affecting release: dot-locale input now formats differently.

### Breaking / behavior change

- **`;` argument separators normalize to `,`** — in the dot locale,
  `=SUM(1;2)` now formats to `=SUM(1, 2)`. The engine accepts both, but
  output is no longer byte-identical to a `;`-separated input.

### Fixes

- **Cap nesting depth at 200** — deeply nested formulas previously
  overflowed the stack or hung the layout search.
- **Keep adjacent operand tokens separated** — no more silent fusion of
  neighbouring tokens into one invalid token.
- **Never emit trailing whitespace.**
- **Measure layout width in characters, not bytes** — multi-byte
  identifiers (`Umsätze`) no longer break lines early.
- **Count the leading `=` against the width on line 0** — first-line fit
  now matches the rendered result.
- **Keep operators from fusing into an open error literal**
  (e.g. `#REF!`-adjacent operators).

## v0.1.0 — 2026-08-05

Initial standalone release, extracted from dotfiles: recursive-descent
tokenizer/parser, width-aware layout with LET/IFS/SWITCH pair alignment,
`--minify`, CLI/env/config width resolution, nvim format-on-save
integration.
