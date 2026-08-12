# Issue #12 implementation plan: comma-locale array separators

> **Status:** Ready to implement
> **Issue:** [#12 — recognize backslash array separators in comma-decimal locales](https://github.com/colinperel/gsfmt/issues/12)
> **Baseline:** `cc46a06` (`v0.7.1`)
> **Scope:** gsfmt only; no tree-sitter grammar changes

This plan is self-contained. An implementing agent should be able to execute it
without revisiting the original review. Read the named source and test sections
before editing, preserve the decisions below, and keep the change focused on
issue #12.

## Outcome

In `Decimal::Comma`, gsfmt must recognize `\` as the horizontal/column
separator in a Google Sheets array literal. It must preserve the separator
through normal formatting and minification, keep `;` as the row separator,
and leave comma-decimal numbers intact.

The implementation must not change dot-locale output or turn the fix into a
general formula validator. In particular, dot-locale backslashes retain the
current permissive malformed-input behavior.

Examples after the fix:

```text
input (comma locale)       format
={1\2;3\4}                ={1\ 2; 3\ 4}\n
={1,5\2,5;3,5\4,5}        ={1,5\ 2,5; 3,5\ 4,5}\n
input (comma locale)       minify
={1 \ 2; 3 \ 4}           ={1\2;3\4}\n
=SPARKLINE(                =SPARKLINE(A1:A10;{"charttype"\"column";
  A1:A10;                    "color"\"green"})\n
  {"charttype"\"column";
   "color"\"green"}
)
```

The compact SPARKLINE output above is the minified shape. At the default width,
normal formatting should be:

```text
=SPARKLINE(A1:A10; {"charttype"\ "column"; "color"\ "green"})
```

## Authoritative syntax and confirmed defect

Google's official [Using arrays in Google Sheets](https://support.google.com/docs/answer/6208276?hl=en)
documentation states:

- commas separate columns in ordinary dot-decimal locales;
- semicolons separate rows; and
- in countries that use comma decimal separators, commas are replaced by
  backslashes when creating arrays.

Therefore a comma-locale 2×2 array is `={1\2;3\4}`: `\` separates columns
and `;` separates rows.

Current v0.7.1 behavior, reproduced from the built CLI:

```console
$ printf '%s\n' '={1\2;3\4}' | gsfmt --decimal comma
={1 \2; 3 \4}

$ printf '%s\n' '={1\2;3\4}' | gsfmt --decimal comma --minify
={1 \2;3 \4}
```

The lexer currently returns `Atom("1")`, `Atom("\\2")`, `Sep(";")`,
`Atom("3")`, `Atom("\\4")`. The renderer correctly inserts a guard space
between what it believes are adjacent operands, but those boundaries are
wrong. The source boundaries are `Atom("1")`, `Sep("\\")`, `Atom("2")`,
`Sep(";")`, `Atom("3")`, `Sep("\\")`, `Atom("4")`.

This is a lexer bug, not a layout bug. Once `\` is a `Kind::Sep`, the existing
parser and renderers already retain its source text and apply ordinary
separator spacing.

## Relevant ownership path

Read these locations before editing:

1. `src/lib.rs:119-163` — `Kind`, `Token`, `is_ident_start`, and
   `is_ident_body`.
2. `src/lib.rs:165-380` — `tokenize`, especially identifier scanning and the
   `;`/`,` separator branches.
3. `src/lib.rs:385-530` — `Group`, `parse_items`, and `parse_group`; confirms
   that every `Kind::Sep` is retained in `Group::seps`.
4. `src/lib.rs:532-557` — `normalize_separators`; only dot-locale non-array
   semicolons are rewritten, so comma-locale `\` requires no rewrite pass.
5. `src/lib.rs:821-839` — `render_group_inline`; separator token text is
   emitted verbatim and receives a space only outside minify mode.
6. `src/lib.rs:983-987`, `src/lib.rs:1375-1432`, and
   `src/lib.rs:1463-1511` — separator measurement and broken-layout emission;
   all use `Token.text`, so a one-character backslash works without layout
   special cases.
7. `tests/format.rs:1121-1214` — existing comma-locale exact, golden,
   idempotence, and mixed-locale tests.
8. `tests/property.rs` — deterministic generated invariant sweep, currently
   dot-locale only.
9. `tests/data/comma_locale.gsfx` — comma-locale fixed-point golden.
10. `README.md:50-74` and the `DECIMAL` section in `src/main.rs`'s `USAGE` —
    user-facing locale behavior.

Do not modify the historical root `PLAN.md`; it is marked executed and covers
an older review.

## Required behavior decisions

### 1. Backslash is locale-sensitive

- Under `Decimal::Comma`, `\` is a `Kind::Sep` token.
- Under `Decimal::Dot`, retain current tokenization: backslash remains allowed
  in the permissive identifier character set.

The dot-locale choice is deliberate backward compatibility. Google Sheets
does not permit backslash in names, but gsfmt has historically tolerated it
under its “a formatter preserves garbage rather than validating it” contract.
The sibling `tree-sitter-gsformula` grammar intentionally rejects backslash
because a highlighter has the opposite contract; it is dot-locale only and
must not be changed for this issue.

Do not globally remove backslash from identifiers. Doing so would reject or
retokenize malformed dot-locale input and create an unrelated behavior change.

### 2. Tokenization remains context-light

The tokenizer does not know whether it is inside `{}` or `()`. Continue that
design: in comma locale, emit `Kind::Sep` for a backslash encountered at a
normal token boundary. Do not add lexer nesting state or a special `ArraySep`
kind. Existing opaque token scans still own their contents: a backslash inside
a string, quoted sheet name, table selector, or chip selector remains content
of that token and must survive byte-for-byte.

Consequences are acceptable and consistent with current separator handling:

- inside any parsed group—array, call, or parenthesized group—the token is
  retained in `Group::seps`; only arrays give it valid Sheets semantics;
- at top level, it triggers the existing `unexpected "\\"` parse error;
- gsfmt still does not attempt semantic validation of array dimensions,
  function arity, or separator placement.

### 3. Preserve separator bytes; do not translate locales

This issue recognizes comma-locale syntax. It does not convert array syntax
between locales:

- comma-locale `\` must remain `\`;
- comma-locale `;` must remain `;`;
- dot-locale array `,` and `;` behavior must remain byte-identical;
- the existing dot-locale call/paren normalization from `;` to `,` remains
  unchanged.

Do not alter `normalize_separators`, `Group`, or any layout signature.

### 4. Standard separator spacing applies

Normal format mode inserts one space after array separators; minify mode does
not. Backslash follows exactly the existing comma/semicolon path:

```text
format: ={1\ 2; 3\ 4}
minify: ={1\2;3\4}
```

Do not special-case whitespace around `\` in the renderer.

### 5. Keep the crate dependency-free

No new dependency is needed. Do not introduce a parser framework, regex crate,
or tree-sitter dependency.

## Implementation steps

### Step 1: Make identifier scanning locale-aware

In `src/lib.rs`, change `is_ident_start` and `is_ident_body` to accept the
resolved `Decimal` and allow `\` only for `Decimal::Dot`.

Target logic:

```rust
fn is_ident_start(c: char, decimal: Decimal) -> bool {
    c.is_alphabetic()
        || matches!(c, '_' | '$')
        || (c == '\\' && decimal == Decimal::Dot)
}

fn is_ident_body(c: char, decimal: Decimal) -> bool {
    c.is_alphanumeric()
        || matches!(c, '_' | '$' | '.')
        || (c == '\\' && decimal == Decimal::Dot)
}
```

Update both tokenizer call sites:

- the branch deciding whether an identifier starts; and
- the loop consuming identifier-body characters.

Both changes are required. Adding only a standalone backslash branch is
insufficient because `foo\bar` would otherwise consume the backslash inside
the preceding identifier before dispatch reaches that branch.

Use the surrounding formatting if rustfmt produces a different but equivalent
line shape. Do not alter accepted letters, digits, `_`, `$`, or `.`.

### Step 2: Emit comma-locale backslashes as separators

Extend the existing separator dispatch in `tokenize`. The smallest clear
shape is to handle `;` and comma-locale `\` together before the `,` branch:

```rust
} else if ch == ';' || (ch == '\\' && decimal == Decimal::Comma) {
    i += 1;
    (ch.to_string(), Kind::Sep)
} else if ch == ',' {
```

Update public documentation at the same ownership boundary:

- `Decimal::Comma` should mention `\` array columns and `;` arguments/rows.
- `Kind::Sep` should say `` `,`, `;`, or comma-locale `\` ``.
- `Group::seps` should cover both array row and array column separators.
- `tokenize`'s error/behavior docs should identify `\` as the comma-locale
  array column separator where useful.
- `normalize_separators` should explicitly say comma-locale separators,
  including array backslashes, are untouched.

The existing table-selector comment about permissive dot-locale backslash
tolerance remains true. Do not churn it for this issue.

Do not edit parse or layout code unless a failing regression proves the
existing generic separator path is insufficient.

### Step 3: Add independent exact lexer and rendering regressions

Add focused tests in the locale section of `tests/format.rs`.

#### Exact token boundaries

Call `gsfmt::tokenize("={1\\2;3\\4}", Decimal::Comma)` and compare the
ordered `(kind, text)` pairs against:

```text
Op    "="
Open  "{"
Atom  "1"
Sep   "\\"
Atom  "2"
Sep   ";"
Atom  "3"
Sep   "\\"
Atom  "4"
Close "}"
```

This expected stream must be hand-authored. Do not derive it by tokenizing a
second formatter output: the production lexer cannot serve as an independent
oracle for the defect it caused.

Add a second hand-authored boundary case for the identifier-body path:

```text
input: ={foo\bar}
Op    "="
Open  "{"
Atom  "foo"
Sep   "\\"
Atom  "bar"
Close "}"
```

Also assert exact outputs `={foo\ bar}\n` from format and `={foo\bar}\n`
from minify under comma options. The numeric case proves that `\` is no
longer accepted as an identifier *start*; this named case independently proves
that identifier-body scanning stops before it.

#### Exact format/minify output

Using comma options, assert all of the following:

```rust
format("={1\\2;3\\4}")
    == "={1\\ 2; 3\\ 4}\n"
minify("={1 \\ 2; 3 \\ 4}")
    == "={1\\2;3\\4}\n"
format("={1,5\\2,5;3,5\\4,5}")
    == "={1,5\\ 2,5; 3,5\\ 4,5}\n"
format("=SPARKLINE(A1:A10;{\"charttype\"\\\"column\";\"color\"\\\"green\"})")
    == "=SPARKLINE(A1:A10; {\"charttype\"\\ \"column\"; \"color\"\\ \"green\"})\n"
minify(the same SPARKLINE input)
    == "=SPARKLINE(A1:A10;{\"charttype\"\\\"column\";\"color\"\\\"green\"})\n"
```

Use `gsfmt::format`/`minify` directly with `comma_opts()`; the existing `fmt`
and `min` helpers intentionally use dot locale.

Add one content-boundary assertion showing that a backslash inside a comma-
locale string remains string content, not a separator, for example:

```rust
gsfmt::format("=CONCAT(\"a\\b\";A1)", &comma_opts())
    == "=CONCAT(\"a\\b\"; A1)\n"
```

#### Fixed-point and preservation assertions

For each comma-array case:

- `format(format(src, comma), comma) == format(src, comma)`;
- `minify(minify(src, comma), comma) == minify(src, comma)`;
- `minify(format(src, comma), comma) == minify(src, comma)`; and
- no output line ends in whitespace.

Add the array cases to the existing
`comma_decimal_output_is_idempotent_and_semantics_preserving` loop rather than
creating duplicate fixed-point plumbing.

#### Dot-locale compatibility

Add focused assertions pinning both identifier-start and identifier-body
behavior for malformed dot-locale backslashes:

```rust
assert_eq!(fmt("=foo\\bar"), "=foo\\bar\n");
assert_eq!(fmt("=\\foo"), "=\\foo\n");
```

This is not a claim that Sheets accepts the name. The test should say it
prevents this locale-specific fix from widening into a dot-locale compatibility
break.

### Step 4: Expand the comma-locale golden

Add an array-valued binding to `tests/data/comma_locale.gsfx` so the existing
full-formula fixed-point test covers backslash in realistic LET layout:

```text
  matrix;     {1\ 2; 3\ 4};
```

Place it after `netto` and before `brutto`. Keep `matrix` unused; the fixture
tests syntax/layout rather than spreadsheet evaluation. `steuersatz` remains
the widest key, so existing value alignment should not change.

After editing, run the formatter with the fixture's test options (comma locale,
width 60) or the focused golden test to verify the file is already canonical.
Do not add this fixture to the cross-repository fixture-sync workflow:
`tree-sitter-gsformula` is intentionally dot-locale only, and
`comma_locale.gsfx` is already excluded from that sync.

### Step 5: Add a narrow generated comma-array sweep

Issue #16 owns the later comprehensive independent test oracle. For this issue,
make the smallest extension that ensures generated comma-locale arrays contain
real backslash separators and exercise all existing invariants.

In `tests/property.rs`:

1. Generalize `token_texts` to accept a `Decimal` argument instead of
   hard-coding `Decimal::Dot`; update current callers with `Decimal::Dot`.
2. Add a deterministic `gen_comma_array(&mut Rng) -> String` that emits 1–4
   rows and 1–4 columns per row. Join columns with `\`, rows with `;`, and
   choose leaves from a comma-safe list such as:

   ```text
   1
   1,5
   2,5E+3
   A1
   $B$2
   "text, inside string"
   TRUE
   #N/A
   näme
   Table1[[#ALL],[Col 1]]
   ```

   Keep row widths rectangular. Wrap the result as `={...}`. This generator
   is intentionally array-focused; do not refactor the full expression
   generator or implement all of issue #16 here.
3. Add a fixed-seed test generating approximately 100 arrays. For each source
   and width in `[1, 10, 30, 82]`, use `Options { decimal: Decimal::Comma,
   width, ..Default::default() }` and assert the same invariants as the current
   dot-locale sweep:
   - formatting succeeds;
   - format is idempotent;
   - minify is idempotent;
   - source, formatted, and minified token-text streams agree when tokenized
     under `Decimal::Comma`;
   - `minify(format(src)) == minify(src)` under the same comma options; and
   - no output line ends in whitespace.
4. Assert each generated source contains `\` when it has at least two columns,
   or configure the generator to always produce at least two columns. Prefer
   always generating 2–4 columns so the test cannot accidentally stop
   exercising the defect.

Do not allow generated comma-array cases to be skipped on parse errors. Unlike
the broad existing expression generator, this narrow generator has a simple
grammar and every output should be valid. A rejection is a test failure.

The token-text comparison here still uses the production lexer and is not an
independent semantic oracle; the hand-authored exact lexer test above closes
that specific gap. Issue #16 will broaden independent coverage later.

### Step 6: Document the complete locale contract

Update user-facing documentation in the same commit:

- In `README.md`'s `decimal` explanation, state:
  - dot locale: decimal `.`, function arguments `,`, array columns `,`, array
    rows `;`;
  - comma locale: decimal `,`, function arguments `;`, array columns `\`,
    array rows `;`.
- In `src/main.rs`'s `DECIMAL` help section, add the same concise distinction.
- In the README preservation paragraph, clarify that array separators remain
  untouched; only dot-locale function/paren `;` separators normalize to `,`.

Do not add a changelog release section or bump `Cargo.toml` in this issue. This
repository records release notes when cutting a version; the implementation
commit should remain a focused bug fix.

## Files expected to change

```text
src/lib.rs                    lexer behavior and local API docs
src/main.rs                   CLI help text only
tests/format.rs               exact lexer/output/compatibility regressions
tests/property.rs             narrow generated comma-array invariant sweep
tests/data/comma_locale.gsfx  realistic fixed-point coverage
README.md                     documented locale separator matrix
```

No other file should need modification. In particular:

- no dependency or lockfile changes;
- no `PLAN.md` changes;
- no tree-sitter repository changes;
- no release workflow or fixture-sync changes;
- no formatter layout refactor.

## Required implementation order

Use test-first order so the bug and intended boundary are visible:

1. Add the exact comma-locale lexer and rendering tests; run them and confirm
   they fail for the reproduced reason.
2. Implement locale-aware identifier scanning and separator dispatch.
3. Run the focused tests until green.
4. Extend the golden and generated comma-array coverage.
5. Update README and CLI help.
6. Run the complete verification matrix.
7. Review the full diff against this plan, then obtain independent cross-agent
   review because the change affects formatter behavior.

Do not commit a deliberately red test separately unless the owner asks for a
multi-commit TDD history. One focused final commit is preferred.

## Verification commands

Run from the repository root:

```sh
# Focused red/green loop; use the actual test names chosen during implementation.
cargo test --locked --test format comma_locale_array
cargo test --locked --test property generated_comma_locale_arrays

# Full required checks.
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --locked

# Confirm dependency-free scope and inspect the complete change.
git diff --check
git diff --stat
git diff -- src/lib.rs src/main.rs tests/format.rs tests/property.rs \
  tests/data/comma_locale.gsfx README.md
```

Also perform these real CLI checks against the newly built binary:

```sh
printf '%s\n' '={1\2;3\4}' | cargo run --quiet --locked -- --decimal comma
printf '%s\n' '={1\2;3\4}' | cargo run --quiet --locked -- --decimal comma --minify
printf '%s\n' '=foo\bar' | cargo run --quiet --locked -- --decimal dot
```

Expected stdout, respectively:

```text
={1\ 2; 3\ 4}
={1\2;3\4}
=foo\bar
```

If any existing ASCII or dot-locale golden changes, stop and diagnose it. That
is outside this issue's intended blast radius.

## Acceptance checklist

- [ ] In comma locale, every `\` at a normal token boundary—including after
      an identifier—terminates the preceding atom and tokenizes as `Kind::Sep`.
- [ ] Backslashes inside strings, quoted sheets, table selectors, and chip
      selectors remain opaque token content.
- [ ] `={1\2;3\4}` formats exactly as `={1\ 2; 3\ 4}\n`.
- [ ] The same array minifies exactly as `={1\2;3\4}\n`.
- [ ] Comma decimals adjacent to separators remain whole tokens.
- [ ] String-valued SPARKLINE option arrays preserve string bytes and use
      backslash columns.
- [ ] Format and minify are fixed points for comma-locale arrays.
- [ ] `minify(format(x)) == minify(x)` under comma options.
- [ ] The comma-locale golden includes and preserves a backslash array.
- [ ] The generated comma-array sweep emits backslashes and passes at widths
      1, 10, 30, and 82.
- [ ] Dot-locale malformed backslash input retains current identifier-start
      and identifier-body behavior.
- [ ] Dot-locale call and array output is byte-identical to the baseline.
- [ ] README and `--help` document decimal, argument, array-column, and
      array-row separators for both locales.
- [ ] No dependencies, release metadata, grammar files, or layout APIs change.
- [ ] Format, clippy, full tests, strict rustdoc, and `git diff --check` pass.
- [ ] Independent review finds no behavior regression.

## Pitfalls to avoid

1. **Only adding a backslash dispatch branch.** Identifier-body scanning would
   still swallow `foo\bar`. Make both identifier helper calls locale-aware.
2. **Globally deleting backslash from identifiers.** That changes dot-locale
   malformed-input behavior and violates the compatibility decision.
3. **Treating backslash as an operator.** It is a separator and must populate
   `Group::seps`; operator rendering would glue or space it incorrectly.
4. **Normalizing `\` to `,`.** Locale selection controls parsing/formatting,
   not translation between locale syntaxes.
5. **Special-casing only arrays in layout.** The generic separator pipeline
   already owns spacing and preservation; lexer context state would add
   complexity without improving this formatter's validation contract.
6. **Using default options in comma tests.** Every format, minify, re-tokenize,
   and round-trip call must use `Decimal::Comma` consistently.
7. **Using the lexer as the only oracle.** Retain the hand-authored expected
   token-boundary test even after generated invariants pass.
8. **Sending the comma golden to tree-sitter fixture sync.** The grammar is
   dot-locale only by design.
9. **Refactoring layout while here.** No layout behavior needs to change once
   the separator token is correct.

## Commit and issue closeout

Use one conventional commit after all checks and independent review pass:

```text
fix: recognize comma-locale array separators
```

In the commit or eventual pull request, reference `Fixes #12`. Do not push,
open a pull request, merge, publish, or close the issue unless the owner
explicitly requests that shared action.
