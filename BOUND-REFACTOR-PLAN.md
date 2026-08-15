# Making the width bound a dry run of the renderer

> **Superseded by `LAYOUT-IR-PLAN.md`.** Tried 2026-08-15 and abandoned. The approach below was implemented far
> enough to measure and does not work — for two independent reasons, both
> recorded in "What the attempt found". The problem it addresses is real and
> still open; this design is not the answer. Read the outcome section before
> reviving any of it.

Self-contained: an implementing agent needs only this file plus the repo.

## The problem

`src/lib.rs` carries two implementations of the same geometry:

| bound | renderer it models |
|---|---|
| `min_group_width` | `layout_group` |
| `min_items_width` | `layout_items` |
| `min_chunk_width` | `layout_items`' trailing-group branch |
| `min_pairs_width` | `layout_pairs` |
| `stays_inline` | `layout_items`' inline shortcut |

Every one is hand-kept in step with its counterpart. Across PRs #19, #20 and
#21 that has failed twelve times, and each failure looks the same: the bound
predicts a layout the renderer does not produce, and the caller then makes a
decision on a width nothing will ever emit.

The twelve, by root cause:

- **#19** — `min_chunk_width` measured a group's body one indent step deeper
  than `layout_items` emits it, compounding per nesting level. A 68-binding
  `LET` measured 83 against a width of 82, all of it phantom, and lost pair
  layout entirely.
- **#20** — `ALIGN_MAX` gated alignment without reference to the window;
  fit tests could not see the separator their caller appends, while the
  bound already counted it via `with_sep`.
- **#21** — five instances of one measurement error (reading a fragment
  carrying a newline inside a string literal as one continuous line) in the
  hung test, `key_line`, `aligned_col`, `pair_layout_fits` and
  `min_chunk_width`; then two more introduced by the fixes for those, one of
  which reintroduced the tower at the aligned column that #19 existed to
  remove.

Shared helpers (`emitted_span`, `PairKeys`, `trailing_cols`, `stays_inline`)
made each disagreement harder to write. They did not make it impossible,
because the two models remain independent.

## The finding that makes this tractable

The bound looks like infrastructure. It is not. Grep every call:

```
min_items_width  → src/lib.rs:1413, 1431   (both inside pair_layout_fits)
min_pairs_width  → src/lib.rs:1419         (inside pair_layout_fits)
```

`layout_items` never consults it. `layout_group` reaches it through exactly
one call, `pair_layout_fits`, and `layout_pairs` not at all. The body clamp
that `min_chunk_width`'s doc describes is applied unconditionally by
`layout_items`; the bound only *mirrors* it.

So **206 lines of bound exist to answer one question**: should this
pair-shaped group take pair layout or the plain per-argument layout?

| item | lines |
|---|---|
| `MinWidth` + `with_sep` | 4 |
| `min_group_width` | 70 |
| `min_pairs_width` | 47 |
| `min_items_width` | 35 |
| `min_chunk_width` | 50 |
| **total** | **206** |

That question does not need a parallel model of the whole layout algorithm.
It needs the answer to "what would this look like laid out each way?"

## Proposed shape

Replace the bound with a trial layout, memoized.

`pair_layout_fits(g, lead, arg_indent, width)` becomes:

```rust
let pairs = layout_pairs(g, open, lead, indent, width, cache);
if widest(&pairs) <= width {
    return pairs;                       // pairs fit; done
}
let plain = layout_plain(g, indent, width, cache);
if widest(&plain) <= width { plain } else { pairs }
```

This is the policy the current code already documents — *"falling back would
tear the pair apart without fixing the overshoot, and width is a target, not
a ceiling"* — expressed by looking at both layouts instead of predicting
them. The `keys_inline` pre-check disappears: it approximates "a breakable
oversized key would get narrower broken", which the trial answers directly.

`stays_inline` and `pair_value_col` collapse the same way. `pair_value_col`
currently predicts whether the renderer keeps a value inline at a column;
with trials available it can lay the value out at both candidate columns and
compare. The deferred #21 finding (`has_openable_group` overpredicting
expansion, so a value hangs where the renderer would have kept it inline)
disappears with it — not as a fix, but as a question that stops being asked.

### Memoisation is load-bearing, not an optimisation

Naively, `layout_group` would lay its subtree out twice — once on trial, once
for real — making cost `2^depth`. `MAX_DEPTH` is 100, and
`pair_nesting_at_the_cap_stays_fast` already builds a 99-deep `LET` chain
where the indent passes the width around depth 41, so *every* level falls
back and the pathological case is reachable by the existing test suite.

The cache makes the real call reuse the trial's result:

- key: `(node as *const _ as usize, start_col, indent, force, pending)` —
  the tree is immutable during layout, so pointer identity is stable, and
  `width` is constant per `format()` call
- value: `Rc<Vec<String>>`
- threaded as `&mut Cache` through `layout_items` / `layout_group` /
  `layout_pairs`, or held in a struct those become methods on

Total work then falls to one layout per distinct `(node, params)`, which is
what the bound traversal already costs today. Expect this to be *faster* than
the status quo, which pays for a full bound traversal *and* a full layout.

## Steps

1. Introduce the cache and thread it through the three `layout_*` functions.
   No behaviour change; verify goldens byte-identical and the perf test flat.
2. Add `widest(&[String]) -> usize`, measuring physical lines (`cols` per
   line, not the span — the distinction behind five of the twelve findings).
3. Rewrite `pair_layout_fits` as the trial above. Delete `min_pairs_width`.
4. Collapse `pair_value_col` / `stays_inline` onto trials. Delete
   `min_items_width`, `min_chunk_width`, `min_group_width`, `MinWidth`.
5. Re-point the `the_width_bound_*` tests at the new decision path. They
   assert emitted geometry, so they should survive as-is; if one only makes
   sense against the deleted bound, say so rather than deleting it quietly.
6. Delete the deferred `has_openable_group` case and its regression test only
   if the trial genuinely subsumes it — confirm by reverting the test first
   and watching it pass.

Steps 1–2 are safe and independently shippable. Step 3 is the behavioural
commit and wants its own review round.

## Risks

- **Perf.** The one hard constraint. `pair_nesting_at_the_cap_stays_fast`
  bounds the depth-100 case at 10s (currently ~0.5s release, ~3.4s debug).
  Measure after step 1 and again after step 3; if the cache does not hold the
  cost down, stop — a slower formatter is a worse trade than a hand-kept
  bound.
- **Cache key correctness.** A wrong key silently returns another node's
  layout. Pointer identity is only valid while the tree is not mutated;
  `normalize_separators`, `mark_bound_heads` and `uppercase_function_heads`
  all mutate, and all run before layout. Assert that ordering.
- **Output churn.** The trial should reproduce today's decisions in almost
  every case, but "almost" needs measuring: run the width matrix (20–120) over
  `tests/data/` and the reporter's formula, and treat any diff as something to
  explain rather than accept.
- **Scope.** This touches the file's core. It wants to land as its own PR
  with a full review loop, not folded into feature work.

## Not doing

Rewriting the layout algorithm itself. The renderer's decisions are sound —
every defect in this series was the *bound* disagreeing with them, never the
renderer being wrong. This removes the second opinion, it does not rethink
the first.

## What the attempt found

Implemented through step 3 on a branch, measured, and reverted. Two findings,
either of which sinks it.

### 1. The trial does not subsume `keys_inline`

The plan claims the `keys_inline` pre-check "disappears: it approximates 'a
breakable oversized key would get narrower broken', which the trial answers
directly". It does not.

When *neither* layout fits, the trial has to pick one, and the two documented
cases want opposite answers:

- a lone `LET` binding name longer than the width — pairs must stay, because
  "falling back would tear the pair apart without fixing the overshoot"
  (`pair_layout_fits`); plain is *narrower* here, so "pick the narrower" is
  wrong;
- a breakable oversized key such as `INDIRECT("A" & someLongNamedCell)` —
  plain must win, because the key would get genuinely narrower broken; "keep
  pairs" is wrong, and produced a 44-column line against a 24-column window,
  failing `pair_layout_yields_to_breakable_oversized_keys`.

Widths alone cannot separate those. The distinguishing fact — *is the key
itself breakable* — is what `keys_inline` encodes, and it needs
`min_items_width` to answer. Keeping the pre-check keeps `min_items_width`,
which reaches `min_chunk_width`, `min_group_width` and `min_pairs_width`.
Nothing gets deleted.

### 2. The fallback is a second full layout, which the bound never was

Measured, release, 20 runs each:

| variant | depth-99 `LET` chain | 760-line formula | tests | bound deleted |
|---|---|---|---|---|
| baseline (`ebdee17`) | 896 ms | 8.0 ms | pass | — |
| step 1, cache only | 909 ms | 8.2 ms | pass | none |
| step 3, no pre-check | 1813 ms | 7.3 ms | 1 fail | all 206 lines |
| step 3 + pre-check | 1769 ms | 9.3 ms | pass | none |

The interesting row is the third: when pairs fit, the trial *is* the result
and the bound traversal disappears, so real-world input got **9% faster**.
That is the shape of the win the plan predicted.

It is paid for by the fallback. When pairs overflow, the group lays its
subtree out a second time at different columns, so the memo cannot help —
the plan's central assumption, that memoisation makes the trial affordable,
is wrong for exactly the case the trial exists to handle. Nested fallbacks
multiply, and the depth-99 chain doubles.

The last row is the worst of both: the pre-check keeps the bound alive *and*
the trial pays for the layout, so nothing is deleted and real input is 16%
slower.

### What this rules in and out

Out: replacing the bound with trial layouts of both candidates.

Still open: the original problem. Twelve findings across four PRs, all the
bound disagreeing with the renderer, and nothing here changes that. What the
attempt did establish is where the cost actually lives — the bound is cheap
because it never branches on its own decisions, and any design that makes the
decision by laying out both candidates inherits a branching factor the
integer walk never had.

A next attempt should keep the property that made the bound cheap, and attack
the duplication instead: one traversal that yields both a measurement and the
lines, so the two cannot diverge, rather than two traversals that agree by
inspection. That is a different design, not a refinement of this one.
