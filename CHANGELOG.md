# Changelog

All notable changes to gsfmt. Versions follow [SemVer](https://semver.org);
entries mirror the [GitHub release notes](https://github.com/colinperel/gsfmt/releases).

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
