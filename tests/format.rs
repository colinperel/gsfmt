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

fn fmt(src: &str) -> String {
    gsfmt::format(src, WIDTH).expect("formats cleanly")
}

fn min(src: &str) -> String {
    gsfmt::minify(src).expect("minifies cleanly")
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
    assert_eq!(gsfmt::format(src, 88).unwrap(), "=IFS(a, 1, b, 2)\n");
    assert_eq!(
        gsfmt::format(src, 10).unwrap(),
        "=IFS(\n  a, 1,\n  b, 2\n)\n"
    );
}

#[test]
fn blank_lines_between_let_groups_are_preserved_never_invented() {
    let grouped = "=LET(\n  alpha, 1,\n\n  beta,  2,\n\n  alpha + beta\n)\n";
    assert_eq!(fmt(grouped), grouped, "author's blank lines must survive");

    let ungrouped = gsfmt::format("=LET(alpha,1,beta,2,alpha+beta)", 12).unwrap();
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

#[test]
fn empty_input_is_a_no_op() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("   \n"), "");
    assert_eq!(min(""), "");
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
        let e = gsfmt::format(src, WIDTH).expect_err(&format!("should reject {src}"));
        assert!(
            e.msg.contains(needle),
            "for {src:?} wanted {needle:?}, got {:?}",
            e.msg
        );
    }
}

#[test]
fn error_messages_point_at_a_location() {
    let e = gsfmt::format("=SUM(A1", WIDTH).unwrap_err();
    assert!(e.to_string().contains("character"), "{e}");
}
