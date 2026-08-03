//! Behaviour tests for the formatter.
//!
//! The contract under test is narrow and load-bearing: only whitespace
//! *outside* string literals may change. Anything else — a dropped paren, a
//! blank argument turned into `""`, a renormalised number, a case change —
//! is a corrupted formula, so those get dedicated tests rather than relying
//! on the golden files to notice.

/// Matches the binary's built-in default (src/main.rs) and the shipped
/// `dot_config/gsfmt/config`. The library takes width as a parameter, so
/// these tests are unaffected by whatever config the host machine has.
const WIDTH: usize = 82;

fn opts(width: usize) -> gsfmt::Options {
    gsfmt::Options {
        width,
        ..gsfmt::Options::default()
    }
}

fn comma_opts() -> gsfmt::Options {
    gsfmt::Options {
        width: WIDTH,
        decimal: gsfmt::Decimal::Comma,
    }
}

fn fmt(src: &str) -> String {
    gsfmt::format(src, &opts(WIDTH)).expect("formats cleanly")
}

fn fmt_w(src: &str, width: usize) -> Result<String, gsfmt::Error> {
    gsfmt::format(src, &opts(width))
}

fn min(src: &str) -> String {
    gsfmt::minify(src, &gsfmt::Options::default()).expect("minifies cleanly")
}

/// Formulas exercised by every property test below.
const CORPUS: &[&str] = &[
    include_str!("data/payperiods.gsf"),
    include_str!("data/payperiods.min.gsf"),
    include_str!("data/gnarly.gsf"),
    include_str!("data/gnarly.min.gsf"),
    include_str!("data/monthly.gsf"),
    include_str!("data/monthly.min.gsf"),
    "=LET(x,1,x+1)",
    "=IF(x = \"\", , y)",
    "=FILTER(D6:D, B6:B <> \"\")",
    "=CONCAT(\"say \"\"hi\"\" now\", A1)",
    "=IF(A1, \"yes (really), ok\", \"no\")",
    "=SUM({1, 2; 3, 4})",
    "=MATCH(9^99, B:B)",
    "=A1 * 50%",
    "=-MOD(-A1, 7)",
    "='my sheet'!A1:B2",
    "=stock_data!P3",
    "=SWITCH(A1, 1, \"one\", 2, \"two\", \"other\")",
    "=MAP(A1:A9, LAMBDA(v, IF(v <> \"\", v * 2, \"\")))",
    "=NOW()",
];

// ─────────────────────────────────────────────────────────────── golden ──

#[test]
fn payperiods_golden_is_a_fixed_point() {
    let golden = include_str!("data/payperiods.gsf");
    assert_eq!(
        fmt(golden),
        golden,
        "formatting the golden must reproduce it"
    );
}

#[test]
fn payperiods_minifies_to_its_golden() {
    assert_eq!(
        min(include_str!("data/payperiods.gsf")),
        include_str!("data/payperiods.min.gsf")
    );
}

/// Blank lines are the one thing a minified formula cannot carry, so the
/// round trip lands on the golden with its blank lines removed — not on
/// some third layout.
#[test]
fn payperiods_minify_then_format_matches_golden_without_blank_lines() {
    let golden = include_str!("data/payperiods.gsf");
    let mut flattened = String::new();
    for line in golden.lines().filter(|l| !l.trim().is_empty()) {
        flattened.push_str(line);
        flattened.push('\n');
    }
    assert_eq!(fmt(include_str!("data/payperiods.min.gsf")), flattened);
}

#[test]
fn gnarly_golden_is_a_fixed_point() {
    let golden = include_str!("data/gnarly.gsf");
    assert_eq!(fmt(golden), golden);
}

#[test]
fn gnarly_minify_round_trips_to_its_golden() {
    let golden = include_str!("data/gnarly.gsf");
    assert_eq!(min(golden), include_str!("data/gnarly.min.gsf"));
    assert_eq!(fmt(include_str!("data/gnarly.min.gsf")), golden);
}

/// A real 60-line production formula: nested LAMBDAs, MAP/SCAN/MMULT,
/// `B6:INDEX(...)` open ranges, sheet-qualified refs, and two blank
/// arguments. Exercises the layout at a depth the smaller goldens do not.
#[test]
fn monthly_golden_is_a_fixed_point() {
    let golden = include_str!("data/monthly.gsf");
    assert_eq!(fmt(golden), golden);
}

#[test]
fn monthly_minify_round_trips() {
    let golden = include_str!("data/monthly.gsf");
    assert_eq!(min(golden), include_str!("data/monthly.min.gsf"));
    let mut flattened = String::new();
    for line in golden.lines().filter(|l| !l.trim().is_empty()) {
        flattened.push_str(line);
        flattened.push('\n');
    }
    assert_eq!(fmt(include_str!("data/monthly.min.gsf")), flattened);
}

// ───────────────────────────────────────────────────────────── property ──

#[test]
fn formatting_is_idempotent() {
    for src in CORPUS {
        let once = fmt(src);
        assert_eq!(fmt(&once), once, "not idempotent: {src}");
    }
}

#[test]
fn minifying_is_idempotent() {
    for src in CORPUS {
        let once = min(src);
        assert_eq!(min(&once), once, "minify not idempotent: {src}");
    }
}

/// Formatting only moves whitespace, so it cannot change what the formula
/// minifies to. This is the strongest cheap check that semantics survived.
#[test]
fn formatting_preserves_semantics() {
    for src in CORPUS {
        assert_eq!(min(&fmt(src)), min(src), "format changed semantics: {src}");
    }
}

#[test]
fn minify_output_is_a_single_line() {
    for src in CORPUS {
        let out = min(src);
        assert_eq!(out.matches('\n').count(), 1, "not one line: {src}");
        assert!(out.ends_with('\n'));
    }
}

// ───────────────────────────────────────────────────────── preservation ──

/// Each case is a formula that a semantic-AST formatter would silently
/// rewrite. `@borgar/fx` drops the parens; `formualizer-parse` additionally
/// turns the blank argument into `""`, which changes what Sheets computes.
#[test]
fn only_whitespace_changes() {
    let cases = [
        // blank argument is not an empty string
        ("=IF(x,,y)", "=IF(x, , y)"),
        ("=IFS(A1,,A2,3)", "=IFS(A1, , A2, 3)"),
        // redundant parentheses are the author's, not ours to remove
        ("=((A1+1))", "=((A1 + 1))"),
        ("=(A1)*(B1)", "=(A1) * (B1)"),
        // literal spelling survives verbatim
        ("=A1*1.50", "=A1 * 1.50"),
        ("=A1*1.5E+10", "=A1 * 1.5E+10"),
        // identifier case is never touched
        ("=sum(A1:A9)", "=sum(A1:A9)"),
        ("=SumIf(A1:A9,\">2\")", "=SumIf(A1:A9, \">2\")"),
        // string bytes, including "" escapes and inner ( and ,
        (
            "=IF(A1,\"yes (really), ok\",\"no\")",
            "=IF(A1, \"yes (really), ok\", \"no\")",
        ),
        (
            "=CONCAT(\"say \"\"hi\"\" now\",A1)",
            "=CONCAT(\"say \"\"hi\"\" now\", A1)",
        ),
        // references stay glued together
        ("=FILTER(D6:D,B6:B<>\"\")", "=FILTER(D6:D, B6:B <> \"\")"),
        ("='my sheet'!A1:B2", "='my sheet'!A1:B2"),
        ("=stock_data!P3", "=stock_data!P3"),
        // tight operators
        ("=MATCH(9^99,B:B)", "=MATCH(9^99, B:B)"),
        ("=A1*50%", "=A1 * 50%"),
        ("=-MOD(-A1,7)", "=-MOD(-A1, 7)"),
        // arrays and empty calls
        ("=SUM({1,2;3,4})", "=SUM({1, 2; 3, 4})"),
        ("=NOW()", "=NOW()"),
        ("=IFERROR(A1,#N/A)", "=IFERROR(A1, #N/A)"),
        // a formula with no leading = keeps not having one
        ("SUM(A1,A2)", "SUM(A1, A2)"),
    ];
    for (src, want) in cases {
        assert_eq!(fmt(src), format!("{want}\n"), "input: {src}");
    }
}

#[test]
fn minify_collapses_without_fusing_operators() {
    let cases = [
        ("=IF(x = \"\", , y)", "=IF(x=\"\",,y)"),
        ("=FILTER(D6:D, B6:B <> \"\")", "=FILTER(D6:D,B6:B<>\"\")"),
        ("=IF(A1 >= B1, 1, 2)", "=IF(A1>=B1,1,2)"),
        ("=A1 * 50%", "=A1*50%"),
        ("=SUM({1, 2; 3, 4})", "=SUM({1,2;3,4})"),
    ];
    for (src, want) in cases {
        assert_eq!(min(src), format!("{want}\n"), "input: {src}");
    }
}

// ─────────────────────────────────────────────────────────────── layout ──

#[test]
fn short_formulas_stay_inline() {
    assert_eq!(fmt("=SUM(A1:A9)"), "=SUM(A1:A9)\n");
    assert_eq!(fmt("=IF(A1>0,\"y\",\"n\")"), "=IF(A1 > 0, \"y\", \"n\")\n");
    assert_eq!(
        fmt("=MAP(A1:A9,LAMBDA(v,v*2))"),
        "=MAP(A1:A9, LAMBDA(v, v * 2))\n"
    );
}

/// `LET` is the one call that breaks regardless of width — it is written to
/// make a formula readable, so it is never collapsed back onto one line.
#[test]
fn let_always_breaks_however_short() {
    assert_eq!(fmt("=LET(x,1,x+1)"), "=LET(\n  x, 1,\n  x + 1\n)\n");
}

#[test]
fn long_formulas_break_and_respect_width() {
    let src = include_str!("data/payperiods.gsf");
    let out = fmt(src);
    assert!(out.lines().count() > 20, "expected a broken layout");
    for line in out.lines() {
        assert!(line.len() <= WIDTH, "line over {WIDTH}: {line:?}");
    }
}

#[test]
fn narrow_width_breaks_more_aggressively() {
    let src = "=IFS(a,1,b,2)";
    assert_eq!(fmt_w(src, 88).unwrap(), "=IFS(a, 1, b, 2)\n");
    assert_eq!(fmt_w(src, 10).unwrap(), "=IFS(\n  a, 1,\n  b, 2\n)\n");
}

#[test]
fn blank_lines_between_let_groups_are_preserved_never_invented() {
    let grouped = "=LET(\n  alpha, 1,\n\n  beta,  2,\n\n  alpha + beta\n)\n";
    assert_eq!(fmt(grouped), grouped, "author's blank lines must survive");

    let ungrouped = fmt_w("=LET(alpha,1,beta,2,alpha+beta)", 12).unwrap();
    assert!(
        !ungrouped.contains("\n\n"),
        "blank lines must never be invented: {ungrouped:?}"
    );
}

/// A blank argument holds no text, so a broken call must hang it off the
/// previous line rather than stranding a lone `,` on one of its own.
#[test]
fn a_blank_argument_never_owns_a_line() {
    let src = "=LAMBDA(d, IF(d = \"\", , SUMPRODUCT((checkIns <= d) * (checkOuts > d), values)))";
    let out = fmt(src);
    assert!(
        out.contains("d = \"\", ,"),
        "blank arg should ride the previous line:\n{out}"
    );
    for line in out.lines() {
        assert_ne!(line.trim(), ",", "stranded blank argument:\n{out}");
    }
    // trailing blank argument, as in ARRAYFORMULA(IF(cond, values, ))
    assert_eq!(
        fmt("=IF(nightsRaw>0,values,)"),
        "=IF(nightsRaw > 0, values, )\n"
    );
}

/// Sheets uses `;` as the argument separator in many locales. Swapping it
/// for `,` is not a whitespace change — it corrupts the formula. Pair layout
/// (LET/IFS/SWITCH) previously hardcoded commas.
#[test]
fn argument_separators_are_preserved() {
    let cases = [
        ("=LET(x;1;x+1)", "=LET(\n  x; 1;\n  x + 1\n)\n"),
        ("=IFS(a;1;b;2)", "=IFS(\n  a; 1;\n  b; 2\n)\n"),
        (
            "=SWITCH(v;1;\"one\";2;\"two\")",
            "=SWITCH(\n  v;\n  1; \"one\";\n  2; \"two\"\n)\n",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(fmt_w(src, 14).unwrap(), want, "input: {src}");
    }
    // and the comma locale is untouched
    assert_eq!(fmt("=LET(x,1,x+1)"), "=LET(\n  x, 1,\n  x + 1\n)\n");
}

#[test]
fn separators_survive_a_minify_round_trip() {
    for src in ["=LET(x;1;x+1)", "=IFS(a;1;b;2)", "=SUM({1,2;3,4})"] {
        assert_eq!(min(&fmt(src)), min(src), "separator changed: {src}");
    }
}

/// Width is a target, not a hard ceiling: a token that cannot be split has
/// to print in full. Documented in `--help` and `dot_config/gsfmt/config`.
#[test]
fn an_unbreakable_token_may_exceed_the_width() {
    let out = fmt_w("=SUM(someVeryLongIdentifierName)", 10).unwrap();
    assert!(
        out.lines().any(|l| l.len() > 10),
        "expected an overshooting line:\n{out}"
    );
    // still structurally sound and stable
    assert_eq!(fmt_w(&out, 10).unwrap(), out);
}

#[test]
fn empty_input_is_a_no_op() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("   \n"), "");
    assert_eq!(min(""), "");
}

/// A long chain of binary operators has to break at its operators. Before
/// this, the chain stayed on one line and the trailing-group break indented
/// from its full width, producing a staircase that marched off the right
/// edge — each nested call starting further right than the last.
#[test]
fn long_operator_chains_break_at_their_operators() {
    let src = "=ARRAYFORMULA((createCol < cutoffDate) * (arrivalCol <= endDate) \
* (shipmentCol > limitDate) * ISERR(SEARCH(\"Depot\", originCol)))";
    let out = fmt(src);
    assert_eq!(
        out,
        "=ARRAYFORMULA(\n  \
           (createCol < cutoffDate)\n    \
           * (arrivalCol <= endDate)\n    \
           * (shipmentCol > limitDate)\n    \
           * ISERR(SEARCH(\"Depot\", originCol))\n\
         )\n"
    );
    for line in out.lines() {
        assert!(line.len() <= WIDTH, "line over {WIDTH}: {line:?}");
    }
}

/// The staircase showed up as each successive line being indented further
/// than the last. Guard the shape directly, not just the width.
#[test]
fn nested_calls_after_a_long_prefix_do_not_staircase() {
    let src = "=LET(valid, ARRAYFORMULA((a < b) * (c <= d) * (e > f) \
* ISERR(SEARCH(\"Depot\", g))), valid)";
    let out = fmt(src);
    let indents: Vec<usize> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .collect();
    let deepest = indents.iter().copied().max().unwrap_or(0);
    assert!(
        deepest <= 24,
        "indentation ran away (max {deepest}):\n{out}"
    );
}

/// Tight operators are part of a single value and must never be break
/// points, however long the reference is.
#[test]
fn references_never_break_at_their_colons_or_bangs() {
    let src = "=LET(createCol, stock_data!$A$3:INDEX(stock_data!$A:$A, rLastRow), createCol)";
    let out = fmt(src);
    assert!(
        out.contains("stock_data!$A$3:INDEX(stock_data!$A:$A, rLastRow)"),
        "reference was split:\n{out}"
    );
}

/// Operator splitting must answer to width, not to `force`. `force` only
/// means "do not take the inline shortcut here" — a LAMBDA body getting its
/// own block, or an authored blank line holding a group open. Splitting a
/// short expression because of that also destroyed the grouping that caused
/// the break in the first place.
#[test]
fn a_forced_break_does_not_split_a_short_operator_chain() {
    let grouped = "=LAMBDA(\n  v,\n\n  v * 2\n)\n";
    assert_eq!(fmt(grouped), grouped, "authored grouping must survive");

    // the LAMBDA-body block rule likewise must not split a body that fits
    assert_eq!(
        fmt("=MAP(A1:A9, LAMBDA(v, v * 2 + 1))"),
        "=MAP(A1:A9, LAMBDA(v, v * 2 + 1))\n"
    );
}

/// A group's real starting column and its continuation indent are different
/// numbers. Conflating them let an overlong group look like it fitted inline,
/// emitting a line far past the width.
#[test]
fn a_clamped_continuation_does_not_lie_to_the_fit_test() {
    let sheet = "aSheetNameDeliberatelyMadeVeryLongToForceTheContinuationClampToEngage";
    let src = format!("=LET(a,1,b,LET(c,2,d,{sheet}!$AAA$3000:INDEX(qq, rr), d), b)");
    let out = fmt(&src);

    // INDEX(...) starts past the width, so it must break rather than trail off
    assert!(
        out.contains("INDEX(\n"),
        "overlong group stayed inline:\n{out}"
    );
    // and its body is clamped back, not indented from the prefix's full width
    for line in out.lines() {
        let indent = line.len() - line.trim_start().len();
        assert!(
            indent <= 20,
            "continuation indent ran away ({indent}):\n{out}"
        );
    }
    // the only over-width line is the unbreakable reference token itself
    for line in out.lines().filter(|l| l.len() > WIDTH) {
        assert!(
            line.contains(sheet),
            "over-width line that is not an unbreakable token: {line:?}"
        );
    }
}

/// Whether to clamp has to be measured, not guessed from spare columns. A
/// medium-length prefix leaves room by the "two columns free" test and still
/// pushes the body well past the edge, even though every token would fit at a
/// clamped indent.
#[test]
fn a_medium_prefix_does_not_run_the_body_off_the_edge() {
    let sheet = "aMediumLengthSheetNameForTesting";
    let src = format!(
        "=LET(x, {sheet}!$AA$300:INDEX({sheet}!$AA:$AA, aFairlyLongRowCountVariableName), x)"
    );
    let out = fmt(&src);

    let longest_token = src
        .split(|c: char| "(), ".contains(c))
        .map(str::len)
        .max()
        .unwrap_or(0);
    assert!(
        longest_token <= WIDTH,
        "test premise: every token should fit"
    );
    for line in out.lines() {
        assert!(
            line.len() <= WIDTH,
            "avoidable overflow ({} cols, longest token {longest_token}):\n{out}",
            line.len()
        );
    }
}

/// Deciding the continuation clamp by laying out both candidates and
/// measuring them is exponential: every nested prefixed group runs its own
/// two trials, doubling per level. A 624-byte depth-20 formula took ~5s, and
/// depth 22 ~20s — an editor freeze on save.
///
/// The bound is deliberately loose. It is not a benchmark; it only has to be
/// unreachable for anything super-polynomial, so a loaded machine cannot make
/// it flake. Depth 40 formats in well under a tenth of a second; the
/// exponential version would not finish this century.
#[test]
fn deeply_nested_prefixed_groups_stay_fast() {
    let mut inner = String::from("x");
    for i in 0..40 {
        inner = format!("someSheet{i}!$AA$300:INDEX({inner}, rr)");
    }
    let src = format!("=LET(a, {inner}, a)");

    let start = std::time::Instant::now();
    let out = fmt(&src);
    let elapsed = start.elapsed();

    assert!(!out.is_empty());
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "formatting depth-40 input took {elapsed:?} — layout is super-polynomial again"
    );
}

/// The width bound has to model pair layout. LET pins each key and its value
/// to one line at a column derived from the widest key, so measuring its
/// arguments independently reports a layout the formatter will never emit —
/// and the caller then declines to clamp a body that genuinely does not fit.
#[test]
fn the_width_bound_accounts_for_let_pair_alignment() {
    let body = "LET(aLongBindingName, someFunction(alphaValue, betaValue), \
anotherBinding, 12345, aLongBindingName + anotherBinding)";

    // On its own this body is comfortable.
    let bare = fmt(&format!("=LET(q, {body}, q)"));
    let bare_widest = bare.lines().map(str::len).max().unwrap_or(0);
    assert!(bare_widest <= WIDTH, "test premise: bare body should fit");

    // Behind a long prefix it must be clamped back, not shifted right.
    for sheet_len in [8, 20, 40, 60] {
        let sheet = "S".repeat(sheet_len);
        let out = fmt(&format!("=LET(q, {sheet}!$AA$300:INDEX({body}, rr), q)"));
        for line in out.lines() {
            assert!(
                line.len() <= WIDTH,
                "avoidable overflow at sheet_len={sheet_len} ({} cols):\n{out}",
                line.len()
            );
        }
    }
}

/// The width bound has to model the operator-chain layout it will actually
/// emit: a continuation line is two more columns of indent, then the
/// operator and a space, then the operand. Measuring a chunk from its
/// operator at the original column renders a leading `+` as if it were a
/// unary sign — no space, no continuation indent — so the bound came in
/// three columns short, the clamp was skipped, and the emitted line
/// overflowed avoidably.
#[test]
fn the_width_bound_accounts_for_operator_continuations() {
    let operand = "aLongOperandIdentifierMadeExactly43Chars8XY";
    assert_eq!(operand.len(), 43, "test premise: 43-column operand");

    let body = format!("alphaValue + {operand} + betaValue");

    // On its own this chain is comfortable.
    let bare = fmt(&format!("=LET(q, INDEX({body}, rr), q)"));
    let bare_widest = bare.lines().map(str::len).max().unwrap_or(0);
    assert!(bare_widest <= WIDTH, "test premise: bare chain should fit");

    // Behind prefixes of every length — including the ones that land the
    // continuation just past the edge — it must be clamped back, never run
    // over. Every token fits within the width, so no overflow is excusable.
    for sheet_len in [8, 16, 20, 21, 22, 24, 32, 40, 56] {
        let sheet = "S".repeat(sheet_len);
        let out = fmt(&format!("=LET(q, {sheet}!$AA$300:INDEX({body}, rr), q)"));
        for line in out.lines() {
            assert!(
                line.len() <= WIDTH,
                "avoidable overflow at sheet_len={sheet_len} ({} cols):\n{out}",
                line.len()
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────── locale ──

/// In comma-decimal locales (de/fr/es and friends) `1,5` is the number 1.5
/// and arguments separate with `;`. Treating that `,` as a separator splits
/// a number in half — and the minify round-trip cannot catch it, because
/// minifying strips the very spaces that were wrongly inserted. So these
/// assert exact output.
#[test]
fn comma_decimal_numbers_are_never_split() {
    let o = comma_opts();
    assert_eq!(
        gsfmt::format("=SUM(1,5;2,5)", &o).unwrap(),
        "=SUM(1,5; 2,5)\n"
    );
    assert_eq!(
        gsfmt::format("=IF(A1>0;1,5;2,5)", &o).unwrap(),
        "=IF(A1 > 0; 1,5; 2,5)\n"
    );
    // LET previously misread the bindings entirely
    assert_eq!(
        gsfmt::format("=LET(x;1,5;x+1)", &o).unwrap(),
        "=LET(\n  x; 1,5;\n  x + 1\n)\n"
    );
    assert_eq!(
        gsfmt::minify("=SUM(1,5; 2,5)", &o).unwrap(),
        "=SUM(1,5;2,5)\n"
    );
}

/// A full comma-locale formula, frozen. Reading it under the default dot
/// locale shifts every LET binding and splits `0,19` into `0` and `19`,
/// which is why the locale is configuration rather than a guess.
#[test]
fn comma_locale_golden_is_a_fixed_point() {
    let golden = include_str!("data/comma_locale.gsf");
    let o = gsfmt::Options {
        width: 60,
        decimal: gsfmt::Decimal::Comma,
    };
    assert_eq!(gsfmt::format(golden, &o).unwrap(), golden);
    assert!(
        golden.contains("0,19") && golden.contains("1000,5"),
        "golden must retain its comma decimals"
    );
    // the same bytes under the wrong locale are a different formula
    assert_ne!(fmt_w(golden, 60).unwrap(), golden);
}

#[test]
fn comma_decimal_output_is_idempotent_and_semantics_preserving() {
    let o = comma_opts();
    for src in ["=SUM(1,5;2,5)", "=LET(x;1,5;x+1)", "=IF(A1>0;1,5;2,5)"] {
        let once = gsfmt::format(src, &o).unwrap();
        assert_eq!(
            gsfmt::format(&once, &o).unwrap(),
            once,
            "not idempotent: {src}"
        );
        assert_eq!(
            gsfmt::minify(&once, &o).unwrap(),
            gsfmt::minify(src, &o).unwrap(),
            "semantics changed: {src}"
        );
    }
}

/// The same text means different things in the two locales, which is exactly
/// why the locale is an explicit input rather than something inferred.
#[test]
fn the_two_locales_disagree_about_the_same_text() {
    assert_eq!(fmt("=SUM(1,5;2,5)"), "=SUM(1, 5; 2, 5)\n");
    assert_eq!(
        gsfmt::format("=SUM(1,5;2,5)", &comma_opts()).unwrap(),
        "=SUM(1,5; 2,5)\n"
    );
}

/// A `,` that cannot be part of a number under `Decimal::Comma` means the input
/// mixes locales. Guessing would silently change what the sheet computes.
#[test]
fn mixed_locale_input_is_rejected_not_rewritten() {
    let o = comma_opts();
    for src in ["=SUM(1,5,2,5)", "=SUM(A1,A2)", "=LET(x,1,x+1)"] {
        let e = gsfmt::format(src, &o).expect_err(&format!("should reject {src}"));
        assert!(e.msg.contains("decimal mark"), "for {src}: {}", e.msg);
    }
}

#[test]
fn a_number_carries_at_most_one_decimal_mark() {
    // dot locale: `.5` shorthand still works
    assert_eq!(fmt("=SUM(.5,1.5)"), "=SUM(.5, 1.5)\n");
    // comma locale: a leading `,5` is a stray separator, not a number
    assert!(gsfmt::format("=SUM(,5)", &comma_opts()).is_err());
}

// ─────────────────────────────────────────────────────────────── errors ──

#[test]
fn malformed_formulas_are_rejected() {
    let cases = [
        ("=SUM(A1", "unbalanced"),
        ("=CONCAT(\"abc", "unterminated string"),
        ("='sheet", "unterminated quoted sheet name"),
        ("=SUM(A1))", "unexpected"),
        ("=SUM(A1}", "mismatched"),
        ("=SUM(A1) @", "unexpected character"),
    ];
    for (src, needle) in cases {
        let e = fmt_w(src, WIDTH).expect_err(&format!("should reject {src}"));
        assert!(
            e.msg.contains(needle),
            "for {src:?} wanted {needle:?}, got {:?}",
            e.msg
        );
    }
}

#[test]
fn error_messages_point_at_a_location() {
    let e = fmt_w("=SUM(A1", WIDTH).unwrap_err();
    assert!(e.to_string().contains("character"), "{e}");
}
