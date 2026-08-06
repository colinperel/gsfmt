# gsfmt / tree-sitter-gsformula — Remediation Plan

Source: full technical review of both repos (2026-08-05). Each item below is
self-contained: an implementing agent needs only this file plus the repo.
Items are independent unless a **Depends on** line says otherwise. Do the
items in the listed order when doing several — P1 changes recursion depth
behavior that P11's generator exercises, and P2/P3 change output that P11/P12
pin down.

**Repos:**

- gsfmt: `/Users/colinperel/code/personal/gsfmt` (Rust, zero dependencies — keep it that way)
- grammar: `/Users/colinperel/code/personal/tree-sitter-gsformula` (Tree-sitter, highlighting-only scope)

**Ground rules for every item:**

- gsfmt's core contract: token bytes are copied verbatim; only whitespace
  between tokens changes (P10 is the one sanctioned exception, see its spec).
  Never normalize case, literal spelling, string bytes, or parentheses.
- A blank argument (`IF(x,,y)`) is semantically distinct from `""` — never
  collapse or synthesize one.
- No new crate dependencies in gsfmt. `cargo fmt --check`,
  `cargo clippy --all-targets --locked -- -D warnings` (pedantic is on), and
  `cargo test --locked` must pass after every item.
- Grammar changes require `tree-sitter generate` (regenerated `src/` must be
  committed) and `tree-sitter test` green.
- LF line endings everywhere (`.gitattributes` enforces `eol=lf`).
- Conventional commits (`fix:`, `feat:`, `test:`, `docs:`, `ci:`).

**Sheets engine facts these specs rely on (confirmed by the repo owner):**

1. Sheets **rejects** Excel's space-intersection operator: `=SUM(A1:B2 C1:D2)`
   is a formula parse error. Adjacent operands are never valid Sheets syntax.
2. `^` is **right-associative** in Sheets (`=2^3^2` = 512). The grammar
   already matches; do not change it.
3. Named-range identifiers: Sheets accepts Unicode letters broadly
   (effectively `\p{L}`), plus digits and `_`, not starting with a digit.
4. Multi-line string literals are valid: `="Line 1\nLine 2"` (Alt+Enter)
   resolves with an embedded line break. The newline is *string content* and
   must survive both `format` and `--minify` byte-for-byte.
5. In dot-decimal locales Sheets accepts `;` as an argument separator and
   normalizes it to `,` on entry. `;` remains the array row separator
   (`={1;2}`) in all locales.

---

## P1 — Cap nesting depth in the parser (crash + hang fix)

- **Repo**: gsfmt · **Severity**: Medium · **Effort**: XS · **Risk**: none
- **Problem**: `parse_items`/`parse_group` (src/lib.rs:412–488) recurse per
  nesting level. ~10k nested parens overflow the stack and abort (exit 134,
  even under `--minify`). Separately, layout cost is ~cubic in depth:
  500 nested parens ≈ 7s, 700 ≈ 20s, 1000+ hangs. Repro:
  `python3 -c "print('='+'('*50000+'1'+')'*50000)" | gsfmt --minify -`
- **Spec**:
  - Add `const MAX_DEPTH: usize = 200;` with a comment: real formulas in the
    fixtures nest ≤ ~10; the deepest regression test uses 40; 200 keeps the
    cubic layout worst case well under half a second while making the
    stack-overflow and hang regions unreachable.
  - Thread a `depth: usize` parameter through `parse_items` and `parse_group`
    (or track it in a small parser struct). When a `parse_group` call would
    exceed `MAX_DEPTH`, return
    `err(format!("nesting exceeds {MAX_DEPTH} levels"), open.pos)`.
  - No layout-side change needed: the parser cap bounds layout depth too.
- **Tests** (tests/format.rs):
  - Depth 201 nested parens → `Err`, message contains `"nesting"`.
  - Depth 199 → `Ok` (and completes; wrap in the existing perf-test style
    with a generous `Duration` bound, e.g. < 5s, to also pin the cubic cost).
  - CLI test (tests/cli.rs): deep input exits 2 with empty stdout, mirroring
    `an_unparseable_formula_exits_two_without_writing_output`.
- **Docs**: mention the cap in `--help` is not necessary; add one line to
  README's Development section only if you touch README anyway.
- **Acceptance**: the repro above exits 2 in <100ms; full suite green.

## P2 — Stop fusing adjacent operand tokens

- **Repo**: gsfmt · **Severity**: Medium · **Effort**: S · **Risk**: low
- **Problem**: `render_inline` (src/lib.rs:558–573) emits a joining space only
  after a spaced binary operator. Two consecutive operand *leaves* therefore
  fuse: `=A1 B1` → `=A1B1` (two tokens re-lex as one), `="a" "b"` → `"a""b"`
  (re-lexes as a *single* string with an escaped quote — a semantic merge).
  This violates "only whitespace changes": deleting the whitespace merges
  tokens. Input is never valid Sheets syntax (fact 1), so the fix is
  preservation, not rejection — same doctrine as the selector-shape test
  (tests/format.rs:826: "never crash on garbage, only preserve it").
- **Spec**:
  - In `render_inline`, when the node being emitted is a `Node::Leaf` of kind
    `Atom` or `Str`, and the previous emission was an operand
    (`prev_operand == true`) with no `pending_space`, push one `' '` before
    it — in **both** normal and minify modes.
  - Do **not** insert a space before a `Node::Group`. Group adjacency is
    load-bearing and valid: immediately-invoked callables
    (`LAMBDA(x, x)(1000)` — present in tests/data/monthly.gsfx) must stay
    glued. Note the tokenizer discards whitespace before `(`, so
    `SUM (A1)` already parses as a call and is unaffected.
  - `Kind::Op` handling is unchanged (tight ops, unary signs, `%` keep their
    current spacing rules).
- **Non-goals**: do not try to detect *which* adjacent pairs would lexically
  merge (`A1`+`B1` merges, `A1`+`"x"` doesn't). Uniform space between
  adjacent leaf operands is simpler and always token-preserving.
- **Tests** (tests/format.rs):
  - `fmt("=A1 B1") == "=A1 B1\n"` and `min("=A1 B1") == "=A1 B1\n"`.
  - `min("=\"a\" \"b\"")` keeps two string tokens (assert output contains
    `"a" "b"`); re-formatting is idempotent.
  - `fmt("=LAMBDA(x, x)(1000)")` keeps the invocation glued (already covered
    by the monthly golden — do not break it).
  - Extend the corpus consumed by property tests with one adjacent-atom case.
- **Acceptance**: goldens unchanged; `min(fmt(x)) == min(x)` still holds for
  the whole corpus; new cases pass.

## P3 — Measure width in characters, not bytes

- **Repo**: gsfmt · **Severity**: Medium · **Effort**: M · **Risk**: medium (mechanical, wide blast radius)
- **Problem**: every layout measurement uses `String::len()`/`str::len()`
  (bytes). A 62-char formula containing `é`-heavy strings breaks at width 82
  because it is >82 bytes. Pair alignment pads (src/lib.rs:1095, 1135) also
  count bytes, so multibyte keys mis-align value columns.
- **Spec**:
  - Add `fn width(s: &str) -> usize { s.chars().count() }` (chars, not
    graphemes — no new deps; document the limitation in its doc comment:
    combining marks and wide CJK count as 1).
  - Replace **measurement** uses of `.len()` with `width(...)` throughout the
    layout paths: `render_inline` return handling, `min_group_width`,
    `sep_len` (token text), `min_pairs_width`, `min_items_width`,
    `min_chunk_width`, `layout_items` fit tests, `layout_group` (head/open
    lengths, packed-head fit), `layout_pairs` (keys, pads), `MinWidth`
    arithmetic inputs, and the `format()` top-level inline check.
  - **Trap — do not convert blindly**: `render_inline` returns byte offsets
    (`offs`) that are used for *slicing* (`inline[..offs[gi]]`,
    `inline[offs[gi] + glen..]` in `layout_items`, src/lib.rs:932–934, and in
    `min_chunk_width`). Slicing must stay byte-based. Where an offset is used
    as a *column* (e.g. `col + offs[gi]` in `min_chunk_width`,
    src/lib.rs:794), convert via `width(&inline[..offs[gi]])` instead. Audit
    each `offs` use individually.
  - `Error.pos` is already char-based; leave it.
- **Tests**:
  - `=IF(x, "<50×é>")` (62 chars, 112 bytes) stays inline at width 82.
  - A LET whose keys contain multibyte chars aligns values to the same
    *visual* column (assert the padded prefix char-count, not byte-count).
  - Add one multibyte formula to `CORPUS` so all property tests sweep it.
- **Acceptance**: existing ASCII goldens byte-identical; new tests pass.

## P4 — No trailing whitespace on emitted lines

- **Repo**: gsfmt · **Severity**: Low · **Effort**: XS · **Risk**: none
- **Problem**: in broken layout, a *final* blank argument hangs off the
  previous line as `' '` with no separator (src/lib.rs:1052–1061), leaving a
  line that ends in a space (`  someValue, ␠`). Editors that strip trailing
  whitespace then fight the formatter. (The *inline* form `IF(a, b, )` with a
  space before `)` is tested, documented behavior — keep it.)
- **Spec**: in `layout_group`'s blank-argument hang, only push the joining
  `' '` when a separator or further text will follow on that line; for a
  final blank argument in broken layout, append nothing (the line already
  ends with the previous argument's `,`, which round-trips to the same blank
  argument on re-parse). The measurement mirror in `min_group_width`
  (src/lib.rs:665–690) counts that joining space (`end_cols += 1`); leaving
  it uncorrected makes the bound overestimate by one column for this shape,
  which is the safe direction — either align it or leave a comment saying
  the overestimate is deliberate.
- **Tests**:
  - Reformat a broken call with trailing blank arg; assert no line matches
    `/ $/`.
  - Add a **global invariant** to the property section: for every corpus
    entry and every width in {10, 30, 82}, no output line ends with a space
    or tab. (Exempt nothing; if the inline `, )` form trips it, note that
    `, )` ends with `)`, not a space — it does not trip it.)
  - Assert idempotence of the new shape.
- **Acceptance**: goldens unchanged (fixtures have no trailing-blank-arg
  broken calls); invariant test green.

## P5 — Count the leading `=` in broken-layout width

- **Repo**: gsfmt · **Severity**: Low · **Effort**: S · **Risk**: low
- **Problem**: `format()` (src/lib.rs:1188–1196) includes the `=` in the
  inline fit test but calls `layout_items(&items, 0, …)` and prepends `=`
  afterwards, so line 0 of a broken layout can overshoot by one column:
  `=SUM(abcde) + e` at `--width 10` emits an 11-column first line.
- **Spec**: `layout_items` already distinguishes nothing between "where line
  0 starts" and "continuation indent"; give it that distinction the same way
  `layout_group` has `start_col` vs `indent`: add a `start_col: usize`
  parameter used for line-0 fit decisions (the inline shortcut at
  src/lib.rs:891 and the op-chain trigger at src/lib.rs:906, plus the
  first-chunk recursion at src/lib.rs:908); continuation lines keep using
  `indent`. All existing callers pass `start_col == indent`; `format()`
  passes `start_col = usize::from(eq)`, `indent = 0`.
- **Alternative** (if the parameter threading turns ugly): extend the
  documented "known one-column gap" comment (src/lib.rs:885–888) to cover the
  leading `=` and skip the code change. Prefer the fix; take the
  documentation route only if the diff exceeds ~30 lines.
- **Tests**: `fmt_w("=SUM(abcde) + e", 10)` — every line ≤ 10 columns.
  Re-run the width-bound regression tests (they must stay green).
- **Acceptance**: no golden churn at width 82 (fixtures never sit at the
  boundary); suite green.

## P6 — Multi-line string carve-out in the minify contract

- **Repo**: gsfmt · **Severity**: Low · **Effort**: XS · **Risk**: none
- **Problem**: string literals may contain real newlines (fact 4). gsfmt
  correctly preserves the bytes, but `--minify` output is then not one line,
  contradicting README ("collapses back to a single line") and the
  `minify_output_is_a_single_line` test invariant — which only holds because
  no corpus entry has one. **Do not strip or escape the newline; it is user
  data.**
- **Spec**:
  - README: amend the minify sentence — output is one line *except that
    newlines inside string literals are content and are preserved*.
  - `USAGE` text in src/main.rs: same one-line caveat under `-m, --minify`.
  - Test: change `minify_output_is_a_single_line` to assert "no newlines
    outside string tokens": expected `\n` count = 1 (trailing) + newlines
    inside `Str` tokens of the input (compute via `gsfmt::tokenize`).
  - Add `"=IF(A1,\"line1\nline2\",\"x\")"` to `CORPUS`.
- **Acceptance**: suite green; README/help updated in the same commit.

## P7 — Grammar: `TRUE()` / `FALSE()` must parse as calls

- **Repo**: grammar · **Severity**: Low · **Effort**: S · **Risk**: low
- **Problem**: `boolean` is `token(prec(2, …))` (grammar.js:264) and beats
  `identifier`; `call_expression.function` accepts only
  `choice($.identifier, $.reference)` (grammar.js:108–114). `=TRUE()` — a
  valid Sheets formula — yields ERROR nodes.
- **Spec**: add `$.boolean` to the `function` choice in `call_expression`
  (mirroring the existing `reference` accommodation and its comment). If
  `tree-sitter generate` reports a conflict, resolve with the same
  LR(1)-deferral reasoning documented at grammar.js:71–84 before reaching for
  `conflicts` — a genuine conflict here is unexpected.
  Check `queries/highlights.scm`: whatever fills the `function` field should
  be colored as a function; add a capture for the boolean-as-function case if
  the existing query matches on node type rather than field.
- **Tests** (test/corpus/calls.txt): `=TRUE()`, `=false()`, and
  `=IF(TRUE(), 1, 2)` parse without ERROR; `=TRUE` alone stays a `boolean`.
- **Acceptance**: `tree-sitter generate` clean, regenerated `src/`
  committed, `tree-sitter test` 100%, examples still parse with zero
  ERROR/MISSING.

## P8 — Grammar: Unicode identifiers

- **Repo**: grammar · **Severity**: Low · **Effort**: S · **Risk**: medium (lexer-wide)
- **Problem**: `identifier: /[A-Za-z_][A-Za-z0-9_.]*/` (grammar.js:291) and
  the unquoted sheet-name half of `SHEET` (grammar.js:38) are ASCII-only.
  Sheets binds named ranges with Unicode letters (fact 3), and gsfmt's lexer
  already accepts them (`is_alphabetic`, gsfmt src/lib.rs:131) — so the
  editor shows ERROR on formulas the formatter happily formats.
- **Spec**:
  - `identifier` → `/[\p{L}_][\p{L}\p{N}_.]*/` (tree-sitter's regex supports
    Unicode property escapes; verify with `tree-sitter generate` — if it
    rejects `\p{N}`, fall back to `\p{L}` + explicit `0-9`).
  - Update the unquoted alternative inside `SHEET` the same way (quoted
    sheet names already cover arbitrary content).
  - Leave `CELL`, `ABS_COL`, `boolean`, `error`, and the LET/LAMBDA keyword
    tokens ASCII — column letters and keywords are ASCII in Sheets.
  - Confirm token-precedence still resolves: `boolean`/`error`/keyword
    tokens carry explicit `prec` and must keep beating the widened
    identifier. The corpus run is the check.
- **Tests** (test/corpus/references.txt or a new literals section):
  `=näme + 1`, `=кирилица`, `=日本語.total`, and a sheet-qualified
  `=Umsätze!A1` parse without ERROR.
- **Acceptance**: generate clean, corpus 100%, examples zero-ERROR,
  regenerated `src/` committed.

## P9 — Documentation corrections (both repos)

- **Severity**: Low · **Effort**: XS · **Risk**: none
- **Spec**:
  1. grammar README.md:23–24 — remove "range intersection/union" from the
     covered-operator list (the grammar has neither, correctly: Sheets
     rejects space-intersection, fact 1).
  2. gsfmt: replace the three dangling `dot_config/gsfmt/config` references
     (src/lib.rs:24, tests/format.rs:10, tests/format.rs:328) — the path is a
     dotfiles remnant that does not exist in this repo. Point at the README
     Configuration section instead; the "why 82" rationale should move into
     the `DEFAULT_WIDTH` doc comment itself if it isn't recorded anywhere
     reachable.
  3. gsfmt README: one sentence noting the editor grammar is dot-locale-only
     and stricter than the formatter (`;` separators, Unicode names may
     highlight as errors until P8 ships) so the disagreement is documented.
- **Acceptance**: `grep -rn dot_config src tests README.md` empty; both
  suites green (comment-only changes).

## P10 — OPTIONAL (approved direction, behavior change): normalize `;` → `,` in dot-locale call arguments

- **Repo**: gsfmt · **Severity**: enhancement · **Effort**: M · **Risk**: medium (contract change)
- **Rationale**: Sheets itself accepts `;` in dot locales and rewrites it to
  `,` on entry (fact 5). Mirroring the native editor makes gsfmt output match
  what Sheets will store. This is the **one sanctioned exception** to
  "token bytes verbatim" and must be documented as such.
- **Spec**:
  - Scope: `Decimal::Dot` only. Inside **call/paren groups**
    (`open.text == "("`), emit `,` for every separator regardless of source
    text. Inside **arrays** (`is_array()`), preserve separators verbatim —
    `;` is the row separator and semantic.
  - Under `Decimal::Comma`, nothing changes anywhere.
  - Implementation: the emit sites are the `sep_after`
    closures/`seps.get(..)` lookups in `render_group_inline`
    (src/lib.rs:589), `layout_group` (src/lib.rs:1019), `layout_pairs`
    (src/lib.rs:1102), and the measurement mirror `sep_len`
    (src/lib.rs:696) — centralize into one
    `fn sep_text(g: &Group, i: usize, decimal: Decimal) -> &str` so layout
    and measurement cannot disagree. Note `Options`/`Decimal` must reach
    these functions; today layout is decimal-agnostic, so thread it through
    (or store the resolved separator on `Group` at parse time — simpler and
    keeps layout signatures unchanged; parse already can't see decimal, so
    prefer threading `opts` from `format`/`minify`).
  - Top-level items are argument-free; no change outside groups.
- **Contract/doc updates in the same commit** (mandatory):
  - README "byte-preserving for content: only whitespace changes, never
    tokens" → add "…with one exception: in the dot locale, `;` argument
    separators are normalized to `,`, mirroring the Sheets editor; array row
    separators are untouched."
  - lib.rs module doc: same caveat.
- **Tests to update** (these currently assert preservation):
  - `argument_separators_are_preserved` (tests/format.rs:304) — becomes
    dot-locale normalization cases: `=LET(x;1;x+1)` →
    `=LET(\n  x, 1,\n  x + 1\n)`; keep the comma-locale halves asserting `;`
    is preserved.
  - `separators_survive_a_minify_round_trip` — `{1,2;3,4}` array case stays;
    call-arg cases now assert normalized output on both paths (the invariant
    `min(fmt(x)) == min(x)` must still hold — both normalize identically).
  - `formatting_preserves_semantics` and idempotence sweeps must stay green
    unchanged.
- **Acceptance**: `=SUM(1; 2)` (dot) → `=SUM(1, 2)`; `={1;2}` (dot) →
  `={1;2}`; comma-locale goldens (tests/data/comma_locale.gsfx) byte-identical.
- **Do not start this item without re-confirming with the repo owner** — it
  reverses a currently-tested guarantee; the review recorded it as
  recommended-but-optional.

## P11 — In-repo seeded property test (fuzz institutionalization)

- **Repo**: gsfmt · **Severity**: test gap · **Effort**: M · **Risk**: none
- **Depends on**: P1–P4 (otherwise the generator finds the known bugs).
- **Problem**: a 4000-case external fuzz (idempotence, `min∘fmt = min`,
  re-parse, widths {10, 30, 82}) found zero violations, but that protection
  isn't in the repo.
- **Spec**: new `tests/property.rs` with a small hand-rolled deterministic
  PRNG (e.g. xorshift64, fixed seed constant — no new deps, no
  `Date::now`-style nondeterminism) and a recursive generator over: the
  existing atom kinds (cells, absolute/sheet-qualified refs, quoted sheets,
  numbers incl. `.5`/`%`/exponents, strings with `""` escapes and embedded
  `,;(){}` and newlines, booleans, `#N/A`, table refs with selectors, chip
  postfixes), operators (spaced binary, tight `:`/`!`/`^`, unary chains,
  `%`), calls (0–4 args, blank args, LET/IFS/SWITCH/LAMBDA heads, nested
  arrays), depth ≤ 6, plus multibyte identifiers/strings (P3) and adjacent
  leaf operands (P2). ~500 formulas per run (keep suite <5s).
  For each formula f and each width w in {1, 10, 30, 82}:
  1. `format(f)` errors ⇒ skip (generator may build invalid locale mixes);
     otherwise:
  2. `format(format(f)) == format(f)` (idempotence)
  3. `minify(format(f)) == minify(f)` (semantic preservation)
  4. `format` output re-parses (`tokenize` + `parse` succeed)
  5. no output line ends in whitespace (P4 invariant)
  6. `minify(minify(f)) == minify(f)`
  On failure, print the formula, width, and both outputs (the seed is fixed,
  so every failure is reproducible verbatim).
- **Acceptance**: suite runtime stays reasonable (<10s total); test is
  deterministic across runs and platforms.

## P12 — Width-sweep test

- **Repo**: gsfmt · **Effort**: XS · **Depends on**: P5 (else the `=`
  off-by-one fails the sweep — or scope the assertion to idempotence only).
- **Spec**: for every `CORPUS` entry and every width 1..=100: `format` at
  that width is idempotent and `min(fmt(x)) == min(x)`. Do **not** assert
  line-length ≤ width (unbreakable tokens legitimately overshoot; that
  invariant is only testable the way
  `a_medium_prefix_does_not_run_the_body_off_the_edge` does it — with inputs
  whose every token fits).
- **Acceptance**: runs in a few seconds; green.

## P13 — MSRV job in CI

- **Repo**: gsfmt · **Effort**: XS
- **Spec**: `Cargo.toml` declares `rust-version = "1.74"` but CI only tests
  latest stable. Add a job to `.github/workflows/ci.yml`
  (ubuntu-latest only): install 1.74 via `rustup toolchain install 1.74`
  (or `dtolnay/rust-toolchain@1.74`), run `cargo +1.74 test --locked`. Skip
  clippy on the MSRV job (pedantic lints drift across versions); fmt/clippy
  stay on the stable job.
- **Acceptance**: CI green. If 1.74 fails to compile, either fix the code or
  bump `rust-version` — do not delete the declaration.

## P14 — Fixture drift guard between the two repos

- **Repos**: both · **Effort**: S · **Risk**: none
- **Problem**: `gsfmt/tests/data/{gnarly,monthly,payperiods}{,.min}.gsfx` and
  `tree-sitter-gsformula/examples/*.gsfx` are byte-identical manual copies
  (comma_locale.gsfx is gsfmt-only — grammar is dot-locale). They will drift
  silently.
- **Spec** (pick the lightweight option): add a CI step (and a
  `tests/cli.rs`-style local test is unnecessary — CI is enough) to the
  **grammar** repo that checks out gsfmt at a pinned ref and diffs the six
  shared files, failing on mismatch with a message naming the sync
  direction (gsfmt's goldens are canonical: the formatter generates them).
  Alternatively, a plain `diff -r` step in gsfmt's CI cloning the grammar
  repo read-only. Either repo may host it; do not set up submodules for six
  small files.
- **Acceptance**: CI fails when either copy is edited alone; docs
  (grammar examples/README.md) state which side is canonical.

## P15 — OPTIONAL: layout memoization

- **Repo**: gsfmt · **Effort**: L · **Risk**: medium
- Only if pathological-depth performance matters after P1's cap: the ~cubic
  cost comes from `min_group_width`/`render_inline` re-walking subtrees at
  every level (src/lib.rs:746–797, 957–959). Memoize per (node identity,
  column) or precompute subtree inline widths bottom-up once per `format`
  call. **Skip unless a real formula inside the 200-depth cap is measurably
  slow** — at depth ≤ 200 the worst case observed extrapolates to <0.5s, and
  the complexity is not worth it. Validation if attempted: byte-identical
  output across the full corpus and P11 sweep, plus the existing
  `deeply_nested_prefixed_groups_stay_fast` bound tightened to 1s at depth 100.

---

## Validation matrix (run after any item)

```sh
# gsfmt
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked

# grammar (only for P7/P8/P9.1/P14)
tree-sitter generate && git status --short src/   # regenerated src/ must be committed
tree-sitter test
for f in examples/*.gsfx; do tree-sitter parse "$f" 2>/dev/null | grep -cE 'ERROR|MISSING'; done  # all zeros

# smoke (gsfmt, after building release)
python3 -c "print('='+'('*50000+'1'+')'*50000)" | target/release/gsfmt --minify -; echo "exit=$? (want 2 after P1)"
printf '=A1 B1' | target/release/gsfmt --minify -   # want '=A1 B1' after P2
```

## Suggested sequencing

1. P1 (crash) → P2 (fusion) → P4 (trailing ws) → P3 (char width) — each with its tests
2. P11 + P12 (lock everything in)
3. P5, P6, P9, P13 in any order
4. P7, P8 (grammar), then P14
5. P10 only after explicit go-ahead; P15 almost certainly never
