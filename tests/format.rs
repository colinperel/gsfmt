//! Behaviour tests for the formatter.
//!
//! The contract under test is narrow and load-bearing: only whitespace
//! *outside* string literals may change, plus exactly one sanctioned token
//! rewrite — dot-locale `;` argument separators normalize to `,`, as the
//! Sheets editor itself does. Anything else — a dropped paren, a blank
//! argument turned into `""`, a renormalised number, a case change — is a
//! corrupted formula, so those get dedicated tests rather than relying on
//! the golden files to notice.

/// Matches the binary's built-in default (src/main.rs). The library takes
/// width as a parameter, so these tests are unaffected by whatever config
/// the host machine has.
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
    include_str!("data/payperiods.gsfx"),
    include_str!("data/payperiods.min.gsfx"),
    include_str!("data/gnarly.gsfx"),
    include_str!("data/gnarly.min.gsfx"),
    include_str!("data/monthly.gsfx"),
    include_str!("data/monthly.min.gsfx"),
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
    "=SUM(Table1[Column 1].[file name]) + COUNTA(T2[[#ALL],[Col 1]:[Col 2]])",
    "=MAP(A1:A9, LAMBDA(v, IF(v <> \"\", v * 2, \"\")))",
    "=NOW()",
    "=A1 B1",
    "=IF(näme <> \"\", \"héllo, wörld\", ééé)",
    // a real newline inside a string is content, not layout (no trailing
    // space before it — that would trip the trailing-whitespace invariant,
    // which cannot see token boundaries)
    "=IF(A1, \"line one\nline two\", \"x\")",
];

// ─────────────────────────────────────────────────────────────── golden ──

#[test]
fn payperiods_golden_is_a_fixed_point() {
    let golden = include_str!("data/payperiods.gsfx");
    assert_eq!(
        fmt(golden),
        golden,
        "formatting the golden must reproduce it"
    );
}

#[test]
fn payperiods_minifies_to_its_golden() {
    assert_eq!(
        min(include_str!("data/payperiods.gsfx")),
        include_str!("data/payperiods.min.gsfx")
    );
}

/// Blank lines are the one thing a minified formula cannot carry, so the
/// round trip lands on the golden with its blank lines removed — not on
/// some third layout.
#[test]
fn payperiods_minify_then_format_matches_golden_without_blank_lines() {
    let golden = include_str!("data/payperiods.gsfx");
    let mut flattened = String::new();
    for line in golden.lines().filter(|l| !l.trim().is_empty()) {
        flattened.push_str(line);
        flattened.push('\n');
    }
    assert_eq!(fmt(include_str!("data/payperiods.min.gsfx")), flattened);
}

#[test]
fn gnarly_golden_is_a_fixed_point() {
    let golden = include_str!("data/gnarly.gsfx");
    assert_eq!(fmt(golden), golden);
}

#[test]
fn gnarly_minify_round_trips_to_its_golden() {
    let golden = include_str!("data/gnarly.gsfx");
    assert_eq!(min(golden), include_str!("data/gnarly.min.gsfx"));
    assert_eq!(fmt(include_str!("data/gnarly.min.gsfx")), golden);
}

/// A real 60-line production formula: nested LAMBDAs, MAP/SCAN/MMULT,
/// `B6:INDEX(...)` open ranges, sheet-qualified refs, and two blank
/// arguments. Exercises the layout at a depth the smaller goldens do not.
#[test]
fn monthly_golden_is_a_fixed_point() {
    let golden = include_str!("data/monthly.gsfx");
    assert_eq!(fmt(golden), golden);
}

#[test]
fn monthly_minify_round_trips() {
    let golden = include_str!("data/monthly.gsfx");
    assert_eq!(min(golden), include_str!("data/monthly.min.gsfx"));
    let mut flattened = String::new();
    for line in golden.lines().filter(|l| !l.trim().is_empty()) {
        flattened.push_str(line);
        flattened.push('\n');
    }
    assert_eq!(fmt(include_str!("data/monthly.min.gsfx")), flattened);
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

/// Minify collapses layout to one line — but a newline *inside* a string
/// literal is content (Sheets accepts multi-line strings via Alt+Enter),
/// so the invariant is "no newlines outside string tokens", not "one line".
#[test]
fn minify_emits_no_newlines_outside_string_literals() {
    for src in CORPUS {
        let out = min(src);
        let toks = gsfmt::tokenize(&out, gsfmt::Decimal::Dot).expect("re-tokenizes");
        let inside: usize = toks
            .iter()
            .filter(|t| t.kind == gsfmt::Kind::Str)
            .map(|t| t.text.matches('\n').count())
            .sum();
        assert_eq!(
            out.matches('\n').count(),
            inside + 1,
            "newline outside a string: {src}"
        );
        assert!(out.ends_with('\n'));
    }
}

/// Every corpus formula, every width from 1 to 100: idempotent and
/// semantics-preserving. Line length is deliberately NOT asserted here —
/// an unbreakable token legitimately overshoots a narrow width.
#[test]
fn every_width_is_idempotent_and_semantics_preserving() {
    for src in CORPUS {
        let reference = min(src);
        for width in 1..=100 {
            let out = fmt_w(src, width).expect("formats cleanly");
            assert_eq!(
                fmt_w(&out, width).unwrap(),
                out,
                "not idempotent at width {width}: {src}"
            );
            assert_eq!(
                min(&out),
                reference,
                "semantics changed at width {width}: {src}"
            );
        }
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

/// Adjacent operand tokens are never valid Sheets syntax (there is no
/// space-intersection operator), so the formatter's job is to preserve the
/// two tokens, not to fuse them: glued, `A1 B1` re-lexes as one atom and
/// `"a" "b"` re-lexes as a *single* string with an escaped quote — both are
/// token changes, not whitespace changes. Groups still glue to what precedes
/// them, because `LAMBDA(x, x)(1000)` is a real invocation.
#[test]
fn adjacent_operands_never_fuse() {
    assert_eq!(fmt("=A1 B1"), "=A1 B1\n");
    assert_eq!(min("=A1 B1"), "=A1 B1\n");
    assert_eq!(min("=A1   B1"), "=A1 B1\n");
    assert_eq!(min("=\"a\" \"b\""), "=\"a\" \"b\"\n");
    assert_eq!(fmt("=LAMBDA(x, x)(1000)"), "=LAMBDA(x, x)(1000)\n");
    assert_eq!(min("=LAMBDA(x, x)(1000)"), "=LAMBDA(x,x)(1000)\n");
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

/// The `#` scan is greedy through `/`, `.`, and one `!`/`?`, so an operator
/// glued straight onto an open error literal is swallowed on re-lex:
/// `#N/A/A1` is one token. Found by the property sweep; the emission guard
/// keeps a space there in both modes.
#[test]
fn operators_never_fuse_into_an_error_literal() {
    assert_eq!(min("=#N/A / A1"), "=#N/A /A1\n");
    assert_eq!(min(&min("=#N/A / A1")), min("=#N/A / A1"));
    assert_eq!(
        min(&fmt("=IF(A1, #N/A / 2, 3)")),
        min("=IF(A1, #N/A / 2, 3)")
    );
    // a closed literal needs no guard
    assert_eq!(min("=#REF! + 1"), "=#REF!+1\n");
    // and `#DIV/0!` round-trips as the single token it is
    assert_eq!(min("=#DIV/0! + 1"), "=#DIV/0!+1\n");
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
    let src = include_str!("data/payperiods.gsfx");
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

/// No emitted line ever ends in whitespace: editors that strip trailing
/// whitespace on save must not fight the formatter, so gsfmt output has to
/// be a fixed point under that hygiene too, at every width.
#[test]
fn no_line_ever_ends_with_whitespace() {
    for src in CORPUS {
        for w in [10, 30, WIDTH] {
            let out = fmt_w(src, w).expect("formats cleanly");
            for line in out.lines() {
                assert_eq!(
                    line,
                    line.trim_end(),
                    "trailing whitespace at width {w} for {src}:\n{out}"
                );
            }
        }
    }
}

/// A *final* blank argument in a broken layout appends nothing: the line
/// already ends with the previous argument's separator, which re-parses to
/// the same blank. (Inline, the tested `values, )` shape is unchanged.)
#[test]
fn a_trailing_blank_argument_leaves_no_trailing_space() {
    assert_eq!(
        fmt_w("=IF(someLongCondition, someValue,)", 20).unwrap(),
        "=IF(\n  someLongCondition,\n  someValue,\n)\n"
    );
    // pair layout pads a blank value with alignment spaces; they must not
    // survive to the end of the line
    assert_eq!(fmt("=LET(a,1,x,)"), "=LET(\n  a, 1,\n  x,\n)\n");
}

/// Sheets accepts `;` between arguments in the dot locale and rewrites it
/// to `,` the moment the formula is entered; gsfmt mirrors that — the one
/// sanctioned token rewrite. Array row separators are semantic (`{1;2}` is
/// a column, `{1,2}` a row) and are never touched.
#[test]
fn dot_locale_normalizes_semicolon_argument_separators() {
    assert_eq!(fmt("=SUM(1;2)"), "=SUM(1, 2)\n");
    assert_eq!(min("=SUM(1;2)"), "=SUM(1,2)\n");
    assert_eq!(
        fmt_w("=LET(x;1;x+1)", 14).unwrap(),
        "=LET(\n  x, 1,\n  x + 1\n)\n"
    );
    assert_eq!(
        fmt_w("=SWITCH(v;1;\"one\";2;\"two\")", 14).unwrap(),
        "=SWITCH(\n  v,\n  1, \"one\",\n  2, \"two\"\n)\n"
    );
    // array rows keep their `;`, including inside normalized calls
    assert_eq!(fmt("=SUM({1,2;3,4})"), "=SUM({1, 2; 3, 4})\n");
    assert_eq!(fmt("={1;2}"), "={1; 2}\n");
    assert_eq!(min("={1; 2}"), "={1;2}\n");
}

/// Under `Decimal::Comma` the `;` is the only argument separator there is,
/// so it round-trips untouched — swapping it there would corrupt the
/// formula. Pair layout (LET/IFS/SWITCH) previously hardcoded commas.
#[test]
fn comma_locale_argument_separators_are_preserved() {
    let o = gsfmt::Options {
        width: 14,
        decimal: gsfmt::Decimal::Comma,
    };
    assert_eq!(
        gsfmt::format("=LET(x;1;x+1)", &o).unwrap(),
        "=LET(\n  x; 1;\n  x + 1\n)\n"
    );
    assert_eq!(
        gsfmt::format("=IFS(a;1;b;2)", &o).unwrap(),
        "=IFS(\n  a; 1;\n  b; 2\n)\n"
    );
    // and the default dot locale still emits commas
    assert_eq!(fmt("=LET(x,1,x+1)"), "=LET(\n  x, 1,\n  x + 1\n)\n");
}

#[test]
fn separators_survive_a_minify_round_trip() {
    for src in ["=LET(x;1;x+1)", "=IFS(a;1;b;2)", "=SUM({1,2;3,4})"] {
        assert_eq!(min(&fmt(src)), min(src), "separator changed: {src}");
    }
}

/// Width is a target, not a hard ceiling: a token that cannot be split has
/// to print in full. Documented in `--help`.
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

/// The width bound has to count the separator a non-final argument carries:
/// `layout_group` appends `,` to the last line of every argument block but
/// the final one. Measuring the bare argument reads one column narrow, and
/// exactly at the edge the clamp is skipped for a line that then overflows
/// by that one omitted comma.
#[test]
fn the_width_bound_accounts_for_argument_separators() {
    for sheet_len in 24..=30 {
        let sheet = "S".repeat(sheet_len);
        for arg_len in 40..=50 {
            let arg = "a".repeat(arg_len);
            for src in [
                format!("={sheet}!$A$3:INDEX({arg}, 1)"),
                format!("=LET(q, {sheet}!$A$3:INDEX({arg}, 1), q)"),
            ] {
                let out = fmt(&src);
                for line in out.lines() {
                    assert!(
                        line.len() <= WIDTH,
                        "avoidable overflow at sheet_len={sheet_len} \
                         arg_len={arg_len} ({} cols):\n{out}",
                        line.len()
                    );
                }
            }
        }
    }
}

/// A blank argument gets no line of its own: `layout_group` hangs its
/// punctuation off the previous block's final line (`, ,`). The bound has to
/// accumulate those columns onto that final-line figure, or the hang pushes
/// an at-the-edge line over the width after the clamp already declined.
#[test]
fn the_width_bound_accounts_for_blank_argument_hangs() {
    let sheet = "S".repeat(28);
    for arg_len in 40..=50 {
        let arg = "a".repeat(arg_len);
        for src in [
            format!("={sheet}!$A$3:INDEX({arg}, , 1)"),
            format!("={sheet}!$A$3:INDEX({arg}, , , 1)"),
            format!("={sheet}!$A$3:INDEX({arg}, ,)"),
        ] {
            let out = fmt(&src);
            for line in out.lines() {
                assert!(
                    line.len() <= WIDTH,
                    "avoidable overflow at arg_len={arg_len} ({} cols):\n{out}",
                    line.len()
                );
            }
        }
    }
}

// ────────────────────────────────────────────────────────────── unicode ──

/// Width is measured in characters, not bytes. A multibyte formula that
/// fits within the width must stay inline — measuring bytes made a 61-char
/// formula look 111 columns wide and broke it across four lines.
#[test]
fn multibyte_content_is_measured_in_characters() {
    let s = "é".repeat(50);
    let src = format!("=IF(x, \"{s}\")");
    assert!(src.chars().count() <= WIDTH, "test premise: fits in chars");
    let out = fmt(&src);
    assert_eq!(
        out.lines().count(),
        1,
        "premature break on multibyte content:\n{out}"
    );
}

/// Pair alignment pads to a column, and a column is a character count: a
/// multibyte key must land its value on the same visual column as its
/// ASCII neighbours, not the same byte count.
#[test]
fn multibyte_let_keys_align_their_values_by_column() {
    let out = fmt("=LET(éé,1,aaaa,2,éé+aaaa)");
    let col = |line: &str, ch: char| line.chars().position(|c| c == ch);
    let l1 = out.lines().nth(1).expect("key line");
    let l2 = out.lines().nth(2).expect("key line");
    assert_eq!(col(l1, '1'), col(l2, '2'), "values misaligned:\n{out}");
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
    let golden = include_str!("data/comma_locale.gsfx");
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
    assert_eq!(fmt("=SUM(1,5;2,5)"), "=SUM(1, 5, 2, 5)\n");
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

/// The leading `=` occupies a column of line 0 even though it is not
/// indentation. Fit decisions used to ignore it, so a first line landing
/// exactly at the width overshot by one avoidable column.
#[test]
fn the_leading_equals_counts_against_the_width() {
    let out = fmt_w("=SUM(abcde) + e", 10).unwrap();
    for line in out.lines() {
        assert!(
            line.len() <= 10,
            "avoidable overflow ({} cols): {line:?}\n{out}",
            line.len()
        );
    }
    // and without the `=` the same body may use the full width
    assert_eq!(fmt_w("SUM(abcdef)", 11).unwrap(), "SUM(abcdef)\n");
}

/// Parsing recurses per nesting level, so unbounded depth used to overflow
/// the stack (a hard abort, not an error) around ten thousand levels, and
/// layout cost grows super-linearly with depth — a hostile paren tower hung
/// the formatter long before that. The cap turns both into an ordinary
/// parse error while sitting far above any real formula.
#[test]
fn nesting_beyond_the_cap_is_rejected_not_fatal() {
    let deep = format!("={}1{}", "(".repeat(201), ")".repeat(201));
    let e = fmt_w(&deep, WIDTH).expect_err("should reject depth 201");
    assert!(e.msg.contains("nesting"), "{}", e.msg);

    let ok = format!("={}1{}", "(".repeat(199), ")".repeat(199));
    let start = std::time::Instant::now();
    assert!(fmt_w(&ok, WIDTH).is_ok(), "depth 199 must still format");
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "formatting at the depth cap took {elapsed:?}"
    );
}

// ─────────────────────────────────────────────────── table references ──

/// A table selector is an opaque atom: everything between the brackets
/// (spaces, commas, `:` ranges) is selector syntax, not formula syntax,
/// and survives byte-for-byte. The selector stays glued to the table
/// name, and the chip-extraction postfix stays glued to the selector.
#[test]
fn table_references_survive_verbatim() {
    let cases = [
        ("=SUM(Table1[Column 1])", "=SUM(Table1[Column 1])"),
        ("=COUNTA(Table1[#ALL])", "=COUNTA(Table1[#ALL])"),
        (
            "=Table1[[#ALL],[Column 1]:[Column 3]]",
            "=Table1[[#ALL],[Column 1]:[Column 3]]",
        ),
        // chip extraction on a table column
        (
            "=Table1[Column 1].[file name]",
            "=Table1[Column 1].[file name]",
        ),
        // chip extraction on a cell reference
        ("=A1.[email]", "=A1.[email]"),
        // layout around the reference still normalises
        (
            "=SUM(DeptSales[Amount])/COUNTA(DeptSales[Region])",
            "=SUM(DeptSales[Amount]) / COUNTA(DeptSales[Region])",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(fmt(src), format!("{want}\n"), "input: {src}");
        assert_eq!(fmt(&fmt(src)), format!("{want}\n"), "not idempotent: {src}");
    }
}

/// Bytes inside the selector are the author's — minify strips whitespace
/// between tokens, never inside one, so `[[#ALL], [Col 1]]` keeps its
/// inner space. Deliberate consequence of the opaque-atom design.
#[test]
fn minify_does_not_touch_table_selector_bytes() {
    assert_eq!(min("=SUM( Table1[Column 1] )"), "=SUM(Table1[Column 1])\n");
    assert_eq!(
        min("=Table1[[#ALL], [Col 1]]"),
        "=Table1[[#ALL], [Col 1]]\n"
    );
}

/// Selector commas belong to the selector even when `,` is the decimal
/// mark — the comma-locale guard must not fire inside brackets.
#[test]
fn table_selector_commas_are_not_separators_in_comma_locale() {
    assert_eq!(
        gsfmt::format("=SUM(Table1[[#ALL],[Col 1]])", &comma_opts()).expect("formats cleanly"),
        "=SUM(Table1[[#ALL],[Col 1]])\n"
    );
}

#[test]
fn malformed_table_references_are_rejected() {
    let cases = [
        ("=SUM(Table1[Column 1", "unterminated table selector"),
        ("=SUM(Table1[[#ALL],[Col 1]", "unterminated table selector"),
        // a selector needs a table name in front of it
        ("=SUM([Column 1])", "unexpected character"),
        // a stray closing bracket is not silently absorbed
        ("=SUM(A1])", "unexpected character"),
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

/// Deliberate: gsfmt does not validate selector shape. The grammar
/// (github.com/colinperel/tree-sitter-gsformula) rejects `Table1[]` and friends with an
/// ERROR node — that is a highlighter's contract. A formatter's is the
/// opposite: never crash on garbage, only preserve it (see the identifier
/// note in that repo's grammar.js). gsfmt checks what layout needs — balance and
/// termination — not whether the selector is semantically valid, exactly
/// as it accepts calls to functions that do not exist. `Table1.[x]` also
/// cannot be told apart from the valid cell chip `A1.[email]` lexically.
#[test]
fn selector_shape_is_preserved_not_validated() {
    let cases = [
        "=SUM(Table1[])",
        "=Table1[[#ALL],[]]",
        "=Table1[Column 1].[]",
        "=Table1[Column 1].[#ALL]",
        "=Table1[#ALL].[file name]",
        "=Table1.[file name]",
    ];
    for src in cases {
        assert_eq!(fmt(src), format!("{src}\n"), "input: {src}");
    }
}
