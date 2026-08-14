//! Behaviour tests for the formatter.
//!
//! The contract under test is narrow and load-bearing: only whitespace
//! *outside* string literals may change, plus two sanctioned token
//! rewrites, both mirroring the Sheets editor — dot-locale `;` argument
//! separators normalize to `,`, and (opt-in) call heads uppercase.
//! Anything else — a dropped paren, a blank argument turned into `""`, a
//! renormalised number, a case change outside the opt-in — is a corrupted
//! formula, so those get dedicated tests rather than relying on the
//! golden files to notice.

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
        ..gsfmt::Options::default()
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

fn upper_opts() -> gsfmt::Options {
    gsfmt::Options {
        uppercase_functions: true,
        ..gsfmt::Options::default()
    }
}

fn fmt_upper(src: &str) -> String {
    gsfmt::format(src, &upper_opts()).expect("formats cleanly")
}

fn min_upper(src: &str) -> String {
    gsfmt::minify(src, &upper_opts()).expect("minifies cleanly")
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
    // an operator chain interleaved with a multi-line string (QUERY built
    // from cell references), same no-trailing-space caveat
    "=QUERY(A:R, \"SELECT K\nWHERE A = \"& $C$1 &\"\nORDER BY N\", 1)",
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

/// The uppercase rewrite must be as safe as the default mode: idempotent,
/// and format/minify still agree on token text over the whole corpus.
#[test]
fn uppercase_formatting_is_idempotent_and_semantics_preserving() {
    for src in CORPUS {
        let once = fmt_upper(src);
        assert_eq!(fmt_upper(&once), once, "not idempotent: {src}");
        assert_eq!(
            min_upper(&once),
            min_upper(src),
            "format changed semantics: {src}"
        );
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

/// `uppercase_functions` rewrites call heads the way the Sheets editor
/// does on entry — and nothing else: arguments, references, strings, and
/// LET/LAMBDA-bound names keep their authored case. Bound names match
/// case-insensitively (Sheets names are), and the setting applies to
/// minify too so the two modes keep agreeing on token text.
#[test]
fn uppercase_functions_rewrites_heads_only() {
    // heads uppercase; the reference argument is untouched
    assert_eq!(fmt_upper("=sum(a1:a2)"), "=SUM(a1:a2)\n");
    // string content and sheet names are untouched
    assert_eq!(
        fmt_upper("=iferror(a1, \"use sum()\")"),
        "=IFERROR(a1, \"use sum()\")\n"
    );
    // LET/LAMBDA-bound names keep their authored case at the call site,
    // matched case-insensitively; builtin heads inside still uppercase
    assert_eq!(
        fmt_upper("=let(myTax, lambda(x, round(x * 0.3)), MYTAX(1000))"),
        "=LET(\n  myTax, LAMBDA(x, ROUND(x * 0.3)),\n  MYTAX(1000)\n)\n"
    );
    // a LAMBDA parameter invoked as a call keeps its case
    assert_eq!(
        fmt_upper("=lambda(fn, fn(2))(lambda(x, x))"),
        "=LAMBDA(fn, fn(2))(LAMBDA(x, x))\n"
    );
    // bound-name matching is Unicode case folding, not ASCII: `nÄme`
    // binds `näme(1)`, so the call keeps its authored case (an ASCII
    // fold would miss the match and mangle it to `NäME(1)`)
    assert_eq!(
        fmt_upper("=let(nÄme, lambda(x, x), näme(1))"),
        "=LET(\n  nÄme, LAMBDA(x, x),\n  näme(1)\n)\n"
    );
    // folding, not uppercase conversion: U+212A KELVIN SIGN uppercases to
    // itself but folds to `k`, so it still binds a `k(` call site
    assert_eq!(
        fmt_upper("=let(\u{212a}, lambda(x, x), k(1))"),
        "=LET(\n  \u{212a}, LAMBDA(x, x),\n  k(1)\n)\n"
    );
    // and the expanding cases: `ẞ` lowercases to `ß` while `ß` uppercases
    // to `SS`, so a one-step round-trip keys them apart and mangles the
    // bound call to `SS(1)`
    assert_eq!(
        fmt_upper("=let(\u{1e9e}, lambda(x, x), \u{df}(1))"),
        "=LET(\n  \u{1e9e}, LAMBDA(x, x),\n  \u{df}(1)\n)\n"
    );
    // minify agrees
    assert_eq!(min_upper("=sum(a1:a2)"), "=SUM(a1:a2)\n");
    // idempotent: re-formatting the rewritten output is a no-op
    assert_eq!(fmt_upper("=SUM(a1:a2)"), "=SUM(a1:a2)\n");
    // off by default: authored case survives
    assert_eq!(fmt("=sum(a1:a2)"), "=sum(a1:a2)\n");
}

/// Bound-name suppression follows Sheets' evaluation-order scope, not a
/// formula-wide sweep: a `LET` name is visible only to subsequent value
/// expressions and the final expression, a `LAMBDA` parameter only to its
/// body, and neither leaks to siblings or ancestors of the binder.
#[test]
fn uppercase_bound_names_respect_lexical_scope() {
    // a LET name is not visible in its own value expression: the `sum`
    // call there is the builtin and uppercases; the body call is bound
    assert_eq!(
        min_upper("=let(sum,sum(a1:a2),sum(1))"),
        "=LET(sum,SUM(a1:a2),sum(1))\n"
    );
    // earlier LET names are visible in later value expressions
    assert_eq!(
        min_upper("=let(f,lambda(x,x),g,f(1),g(2))"),
        "=LET(f,LAMBDA(x,x),g,f(1),g(2))\n"
    );
    // a later binding does not reach back into an earlier value
    assert_eq!(
        min_upper("=let(a,sum(1),sum,lambda(x,x),sum(2))"),
        "=LET(a,SUM(1),sum,LAMBDA(x,x),sum(2))\n"
    );
    // bindings pop at the binder's edge: siblings and ancestors see the
    // builtin again
    assert_eq!(
        min_upper("=if(lambda(sum,sum(1))(2),sum(3),sum(4))"),
        "=IF(LAMBDA(sum,sum(1))(2),SUM(3),SUM(4))\n"
    );
    assert_eq!(
        min_upper("=sum(let(sum,lambda(x,x),sum(1)))"),
        "=SUM(LET(sum,LAMBDA(x,x),sum(1)))\n"
    );
    // a bound name shadowing LET/LAMBDA resolves to the passed value
    // (placeholders take precedence over builtins), so its call is an
    // ordinary user-function call: it neither uppercases nor binds —
    // the `sum` calls inside are builtins and uppercase
    assert_eq!(
        min_upper("=lambda(let,let(sum,sum(1),sum(2)))(lambda(a,b,c,c))"),
        "=LAMBDA(let,let(sum,SUM(1),SUM(2)))(LAMBDA(a,b,c,c))\n"
    );
    // same via LET: `lambda` is the builtin binder in its own value
    // expression, a bound user function afterwards
    assert_eq!(
        min_upper("=let(lambda,lambda(x,x),lambda(sum(1),2))"),
        "=LET(lambda,LAMBDA(x,x),lambda(SUM(1),2))\n"
    );
}

/// An operator chain interleaved with a multi-line string stays inline:
/// every physical line already fits the width, and splitting at the `&`s
/// shortens nothing — the span is string content, not layout. The fit
/// check used to measure the string's full span as one line and break the
/// chain. The simple argument before the string still packs onto the
/// opening line, exactly as a group whose first multi-line block is a
/// laid-out argument would pack it.
#[test]
fn operator_chain_around_multiline_string_stays_inline() {
    let src = concat!(
        "=IFERROR(\n",
        "  UNIQUE(\n",
        "    QUERY('Ledger Items (HS Sync)'!A:R,\n",
        "      \"SELECT K, A, N, C, D, E \n",
        "       WHERE A = \"& $C$1 &\" \n",
        "         and E <> 0 \n",
        "       ORDER BY N, K\",\n",
        "      1\n",
        "    )\n",
        "  ),\n",
        "  \"Select house number to view ledger\"\n",
        ")",
    );
    let want = concat!(
        "=IFERROR(\n",
        "  UNIQUE(\n",
        "    QUERY('Ledger Items (HS Sync)'!A:R,\n",
        "      \"SELECT K, A, N, C, D, E \n",
        "       WHERE A = \" & $C$1 & \" \n",
        "         and E <> 0 \n",
        "       ORDER BY N, K\",\n",
        "      1\n",
        "    )\n",
        "  ),\n",
        "  \"Select house number to view ledger\"\n",
        ")\n",
    );
    assert_eq!(fmt(src), want);
    assert_eq!(fmt(want), want, "must be a fixed point");
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
/// token changes, not whitespace changes. The same applies to a leaf operand
/// followed by a *call*: glued, `A1 SUM(1)` re-lexes as the single call
/// `A1SUM(1)`. Headless groups still glue to what precedes them, because
/// `LAMBDA(x, x)(1000)` is a real invocation.
#[test]
fn adjacent_operands_never_fuse() {
    assert_eq!(fmt("=A1 B1"), "=A1 B1\n");
    assert_eq!(min("=A1 B1"), "=A1 B1\n");
    assert_eq!(min("=A1   B1"), "=A1 B1\n");
    assert_eq!(min("=\"a\" \"b\""), "=\"a\" \"b\"\n");
    assert_eq!(fmt("=A1 SUM(1)"), "=A1 SUM(1)\n");
    assert_eq!(min("=A1 SUM(1)"), "=A1 SUM(1)\n");
    // The error-literal `#` scan would swallow a glued head on re-lex, and
    // format and minify must agree on the token stream they emit.
    assert_eq!(fmt("=#N/A SUM(1)"), "=#N/A SUM(1)\n");
    assert_eq!(min("=#N/A SUM(1)"), "=#N/A SUM(1)\n");
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
        ..gsfmt::Options::default()
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

/// The `*IFS` aggregations take (criteria range, criterion) pairs, so a
/// broken call lays them out one pair per line like SWITCH: SUMIFS-shaped
/// calls (also AVERAGEIFS/MAXIFS/MINIFS) put the aggregated range first,
/// COUNTIFS is pairs from the first argument. A call that fits stays
/// inline — pair layout only shapes a break that was happening anyway.
#[test]
fn ifs_aggregations_break_into_criteria_pairs() {
    assert_eq!(
        fmt_w("=SUMIFS(qC,qG,acct,qA,\">=\" & s)", 24).unwrap(),
        "=SUMIFS(\n  qC,\n  qG, acct,\n  qA, \">=\" & s\n)\n"
    );
    assert_eq!(
        fmt_w("=COUNTIFS(qA,\">=\" & s,qG,acct)", 24).unwrap(),
        "=COUNTIFS(\n  qA, \">=\" & s,\n  qG, acct\n)\n"
    );
    assert_eq!(
        fmt_w("=MINIFS(qC,qG,acct,qA,\">=\" & s)", 24).unwrap(),
        "=MINIFS(\n  qC,\n  qG, acct,\n  qA, \">=\" & s\n)\n"
    );
    // fits → stays inline, unlike LET
    assert_eq!(fmt("=SUMIFS(qC, qG, acct)"), "=SUMIFS(qC, qG, acct)\n");
}

/// The remaining builtins whose variadic tail repeats in twos take the
/// same layout: SORT/SORTN's (column, ascending), AVERAGE.WEIGHTED's
/// (values, weights), GETPIVOTDATA's (column, item) — each after its own
/// count of lone leading arguments.
#[test]
fn remaining_pair_shaped_builtins_break_into_pairs() {
    assert_eq!(
        fmt_w(
            "=SORT(A2:C99,colWithLongName1,TRUE,colWithLongName2,FALSE)",
            30
        )
        .unwrap(),
        "=SORT(\n  A2:C99,\n  colWithLongName1, TRUE,\n  colWithLongName2, FALSE\n)\n"
    );
    assert_eq!(
        fmt_w("=SORTN(A2:C99,5,0,colWithLongName1,TRUE,col2,FALSE)", 30).unwrap(),
        "=SORTN(\n  A2:C99,\n  5,\n  0,\n  colWithLongName1, TRUE,\n  col2,             FALSE\n)\n"
    );
    assert_eq!(
        fmt_w(
            "=AVERAGE.WEIGHTED(scoreRange1,weightRange1,scoreRange2,weightRange2)",
            30
        )
        .unwrap(),
        "=AVERAGE.WEIGHTED(\n  scoreRange1, weightRange1,\n  scoreRange2, weightRange2\n)\n"
    );
    assert_eq!(
        fmt_w(
            "=GETPIVOTDATA(\"total spend\",PivotSheet!A1,\"category\",catValue,\"region\",regionValue)",
            40
        )
        .unwrap(),
        "=GETPIVOTDATA(\n  \"total spend\",\n  PivotSheet!A1,\n  \"category\", catValue,\n  \"region\",   regionValue\n)\n"
    );
    // COUNTUNIQUEIFS is Sheets' sixth *IFS aggregation, same lead-1 shape
    assert_eq!(
        fmt_w(
            "=COUNTUNIQUEIFS(bookingIds,creationDate,\"<\" & cutoffDate,checkIn,\"<=\" & endDate)",
            40
        )
        .unwrap(),
        "=COUNTUNIQUEIFS(\n  bookingIds,\n  creationDate, \"<\" & cutoffDate,\n  checkIn,      \"<=\" & endDate\n)\n"
    );
    // short forms stay inline
    assert_eq!(fmt("=SORT(A2:C9, 1, TRUE)"), "=SORT(A2:C9, 1, TRUE)\n");
}

/// Pair layout renders keys inline, so a breakable key that overflows the
/// width — a criteria range built from a call — sends the whole group to
/// the plain per-argument layout, where the key can wrap. An unbreakable
/// oversized key (a long LET binding name) keeps pair layout and
/// overshoots, like any other unbreakable token.
#[test]
fn pair_layout_yields_to_breakable_oversized_keys() {
    assert_eq!(
        fmt_w(
            "=SUMIFS(qC,INDIRECT(\"A\" & someLongNamedCell),criterion)",
            24
        )
        .unwrap(),
        "=SUMIFS(qC,\n  INDIRECT(\n    \"A\"\n      & someLongNamedCell\n  ),\n  criterion\n)\n"
    );
    assert_eq!(
        fmt_w(
            "=LET(veryLongBindingNameHere,1,veryLongBindingNameHere+1)",
            18
        )
        .unwrap(),
        "=LET(\n  veryLongBindingNameHere, 1,\n  veryLongBindingNameHere\n    + 1\n)\n"
    );
}

/// A pair whose key and value are both fine alone but overflow side by
/// side takes the plain per-argument layout — pair layout pins them to one
/// line, and here falling back genuinely fits.
#[test]
fn pair_layout_yields_when_a_whole_pair_overflows() {
    let out = fmt(
        "=SUMIFS(sumRange,criteriaRangeWithDescriptiveNameForInvoices,\
criterionValueWithDescriptiveNameForInvoices)",
    );
    assert_eq!(
        out,
        "=SUMIFS(\n  sumRange,\n  criteriaRangeWithDescriptiveNameForInvoices,\n  \
criterionValueWithDescriptiveNameForInvoices\n)\n"
    );
    assert!(out.lines().all(|l| l.len() <= WIDTH), "overflow:\n{out}");
    // the final argument carries no separator, so a criterion that fits
    // the width exactly at plain indentation still triggers the fallback
    assert_eq!(
        fmt_w("=SUMIFS(qC,someRange,criterionThatFitsAlone)", 24).unwrap(),
        "=SUMIFS(\n  qC,\n  someRange,\n  criterionThatFitsAlone\n)\n"
    );
}

/// A LET/LAMBDA-bound name shadowing a pair-laid builtin is a user call:
/// it gets the plain per-argument layout, not SUMIFS's criteria pairs —
/// the same scope rule the uppercase rewrite follows.
#[test]
fn bound_names_shadowing_pair_builtins_lay_out_plainly() {
    assert_eq!(
        fmt_w("=LET(sumifs,LAMBDA(a,a),sumifs(someRange,otherRange,criterion))", 24).unwrap(),
        "=LET(\n  sumifs, LAMBDA(a, a),\n  sumifs(\n    someRange,\n    otherRange,\n    criterion\n  )\n)\n"
    );
    // the other name-driven layout rules follow the same scope: a bound
    // `let` doesn't always-break, and a bound `lambda` doesn't force its
    // last argument into its own block
    assert_eq!(
        fmt_w("=IF(x,LET(let,LAMBDA(a,a),let(1,2,3)),y)", 30).unwrap(),
        "=IF(x,\n  LET(\n    let, LAMBDA(a, a),\n    let(1, 2, 3)\n  ),\n  y\n)\n"
    );
    assert_eq!(
        fmt_w(
            "=LET(lambda,LAMBDA(a,b,a),lambda(longishArgument1,longishArgument2,tiny))",
            28
        )
        .unwrap(),
        "=LET(\n  lambda, LAMBDA(a, b, a),\n  lambda(\n    longishArgument1,\n    longishArgument2,\n    tiny\n  )\n)\n"
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

/// `layout_items` clamps a broken group's body to `indent + prefix`, capped
/// at one indent step — so a group with no prefix keeps its own column and
/// its arguments sit exactly one step in. The bound has to model that same
/// cap; measuring the body a full step in from the group's column regardless
/// of the prefix compounds INDENT at every nesting level, and a deeply
/// nested value then reads far wider than the layout it will actually get.
/// A LET whose pairs do fit was being sent to the plain per-argument layout
/// over that phantom width, tearing every key off its value.
#[test]
fn the_width_bound_tracks_the_body_clamp_at_every_level() {
    // Narrowing the window must cost line breaks, never the pair layout
    // itself. The phantom width made `pair_layout_fits` believe these
    // bindings could not be aligned, and the whole LET went to the plain
    // per-argument layout — every key stranded on a line of its own.
    let out = fmt_w(include_str!("data/payperiods.gsfx"), 60).unwrap();
    assert!(
        out.lines().any(|l| l == "  targetYear, B1,"),
        "pair alignment abandoned over a phantom width:\n{out}"
    );
    for line in out.lines() {
        assert!(
            line.len() <= 60 + 1,
            "overflow ({} cols):\n{out}",
            line.len()
        );
    }
}

/// Alignment is a first-line affordance: a value that has to break gets its
/// own line one step in from the key, rather than dragging the whole
/// subtree out to the aligned column. Anchoring a multi-line value there
/// indents by horizontal position instead of nesting depth — the skinny
/// right-margin tower `chain_tail_bodies_take_block_indent_not_bracket_column`
/// already rules out — and the wider the widest key, the narrower the tower.
#[test]
fn a_multiline_pair_value_hangs_below_its_key() {
    let leaf = "n".repeat(16);
    assert_eq!(
        fmt_w(&format!("=LET(kk, AA(BB(CC({leaf}))), kk)"), 30).unwrap(),
        format!("=LET(\n  kk,\n    AA(\n      BB(CC({leaf}))\n    ),\n  kk\n)\n")
    );

    // A value that fits beside its key still sits at the aligned column:
    // padding has no geometry, so there is nothing to gain by moving it.
    assert_eq!(
        fmt("=LET(aa,1,bbbb,2,aa+bbbb)"),
        "=LET(\n  aa,   1,\n  bbbb, 2,\n  aa + bbbb\n)\n"
    );

    // An unbreakable value overflows wherever it is put, so it stays beside
    // its key — hanging it would spend a line and fix nothing.
    let long_ref = "aVeryLongUnbreakableReferenceNameThatCannotFitAnywhere";
    assert_eq!(
        fmt_w(&format!("=LET(kk, {long_ref}, kk)"), 30).unwrap(),
        format!("=LET(\n  kk, {long_ref},\n  kk\n)\n")
    );

    // A blank line the author wrote between bindings still belongs to the
    // pair, so it lands above the key rather than between key and value.
    assert_eq!(
        fmt_w("=LET(aa,1,\n\nbb,SUM(alpha,beta,gamma,delta,epsilon,zeta),aa)", 30).unwrap(),
        "=LET(\n  aa, 1,\n\n  bb,\n    SUM(\n      alpha,\n      beta,\n      gamma,\n      delta,\n      epsilon,\n      zeta\n    ),\n  aa\n)\n"
    );
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

/// The alignment cap is a claim about the window ("one long name cannot
/// shove every value off the screen"), so it has to be enforced against
/// one. A fixed 40 columns is about half the default width but wider than
/// a narrow window entirely: at `--width 30` a 26-column key opened the
/// value column at exactly the right margin, and every short-keyed pair
/// was padded out to a column with nothing left beyond it. Requiring the
/// value column to be inside the window drops the gutter there — and only
/// there. A gutter the window can hold is untouched at any width.
#[test]
fn the_alignment_gutter_stays_inside_the_window() {
    let src =
        "=LET(futureShoulderBookedNights,aVeryLongValueReferenceHere1234,shortKey,2,shortKey)";

    // Narrow: no room past the gutter, so values sit against their own key.
    assert_eq!(
        fmt_w(src, 30).unwrap(),
        "=LET(\n  futureShoulderBookedNights, aVeryLongValueReferenceHere1234,\n  \
         shortKey, 2,\n  shortKey\n)\n"
    );

    // Default: the same gutter fits, so alignment is untouched.
    assert_eq!(
        fmt(src),
        "=LET(\n  futureShoulderBookedNights, aVeryLongValueReferenceHere1234,\n  \
         shortKey,                   2,\n  shortKey\n)\n"
    );

    // Dropping the gutter must never widen the result — that is the whole
    // point of dropping it.
    for width in 12..=60 {
        let out = fmt_w(src, width).unwrap();
        let widest = out.lines().map(|l| l.chars().count()).max().unwrap_or(0);
        let baseline = fmt(src)
            .lines()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        assert!(
            widest <= baseline,
            "narrowing to {width} widened the output to {widest}:\n{out}"
        );
    }
}

/// A block's fit test has to count the separator its *caller* will append
/// to the block's final line. Without it a block ending exactly at `width`
/// looked like it fitted, the comma landed one column past, and the width
/// was quietly exceeded by one — everywhere a separator follows a block,
/// which is nearly everywhere. The width bound already counted those
/// columns (`with_sep`), so the layout was the half that had drifted.
#[test]
fn a_block_fits_only_if_its_trailing_separator_fits_too() {
    // The value ends at exactly the width; the pair's comma follows it.
    let value = "SUM(aaaaaaaaaaaaaaaaaaaaaaaaa, bb)";
    assert_eq!(
        fmt_w(&format!("=LET(kk, {value}, zz, 1, kk)"), 40).unwrap(),
        format!("=LET(\n  kk,\n    {value},\n  zz, 1,\n  kk\n)\n")
    );

    // Same gap on a plain argument, which takes its separator the same way.
    assert_eq!(
        fmt_w(
            "=IFERROR(SUM(aaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bb), fallbackValue)",
            40
        )
        .unwrap(),
        "=IFERROR(\n  SUM(\n    aaaaaaaaaaaaaaaaaaaaaaaaaaaaa,\n    bb\n  ),\n  fallbackValue\n)\n"
    );

    // A newline inside a string literal is content: the fragment already
    // spans physical lines, and the separator lands after its last byte,
    // not after its widest one. Charging the separator against the whole
    // span hung this value below its key over a comma that costs the final
    // line seven columns.
    let out = fmt_w("=LET(longkey, \"aaaa\nb\" & c, z, 1, z)", 23).unwrap();
    assert_eq!(
        out,
        "=LET(\n  longkey, \"aaaa\nb\" & c,\n  z,       1,\n  z\n)\n"
    );
    for line in out.lines() {
        assert!(line.chars().count() <= 23, "overflow:\n{out}");
    }

    // Sweep the edge. The old gap put exactly one line at `width + 1` for
    // whichever lengths landed a block on the boundary, so walk both. Only
    // widths that can hold the atom at its deepest indent are fair game —
    // a token wider than the room left for it overflows on its own merits,
    // which is the one overshoot the formatter does not promise to avoid.
    for len in 10..=45 {
        let arg = "a".repeat(len);
        for src in [
            format!("=LET(kk, SUM({arg}, bb), zz, 1, kk)"),
            format!("=IFERROR(SUM({arg}, bb), fallbackValue)"),
            format!("=SUM(SUM({arg}, bb), tail)"),
        ] {
            for width in (len + 12).max(30)..=60 {
                let out = fmt_w(&src, width).unwrap();
                for line in out.lines() {
                    assert!(
                        line.chars().count() <= width,
                        "overflow at len={len} width={width} ({} cols):\n{out}",
                        line.chars().count()
                    );
                }
            }
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
    let golden = include_str!("data/comma_locale.gsfx");
    let o = gsfmt::Options {
        width: 60,
        decimal: gsfmt::Decimal::Comma,
        ..gsfmt::Options::default()
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
    let deep = format!("={}1{}", "(".repeat(101), ")".repeat(101));
    let e = fmt_w(&deep, WIDTH).expect_err("should reject depth 101");
    assert!(e.msg.contains("nesting"), "{}", e.msg);

    let ok = format!("={}1{}", "(".repeat(99), ")".repeat(99));
    let start = std::time::Instant::now();
    assert!(fmt_w(&ok, WIDTH).is_ok(), "depth 99 must still format");
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "formatting at the depth cap took {elapsed:?}"
    );
}

/// Pair layout (`LET`/`IFS`/`SWITCH`) re-measures its subtree at every
/// level, so its cost is roughly cubic in depth — the worst shape the cap
/// has to bound. At the cap it takes ~1.6s in a debug build (~0.45s
/// release); the cap used to sit at 200, where the same shape took ~4s
/// *release* — an editor freeze on save that the paren-tower test above is
/// far too cheap a shape to notice. The bound is loose on purpose: it is
/// not a benchmark, it only has to be unreachable for the next power of
/// depth, so a loaded CI machine cannot make it flake.
#[test]
fn pair_nesting_at_the_cap_stays_fast() {
    let mut inner = String::from("1");
    for _ in 0..99 {
        inner = format!("LET(aaaaaaaaaaaaaaaa, SUM(A1:A100)+{inner}, x)");
    }
    let src = format!("={inner}");

    let start = std::time::Instant::now();
    assert!(fmt_w(&src, WIDTH).is_ok(), "depth-99 LET chain must format");
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "pair layout at the depth cap took {elapsed:?} — the cubic cost regressed"
    );
}

/// A UTF-8 BOM is a file-encoding artifact, not formula text: it is
/// stripped rather than rejected, and output never carries one, so a BOM'd
/// file formats identically to its clean twin (and a second pass is a
/// no-op).
#[test]
fn a_leading_bom_is_stripped_not_rejected() {
    assert_eq!(fmt("\u{feff}=SUM( 1,2 )"), "=SUM(1, 2)\n");
    assert_eq!(min("\u{feff}=SUM( 1,2 )"), "=SUM(1,2)\n");
    assert_eq!(fmt("\u{feff}"), "");
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

/// A chain's opened tail always takes the block indent, never the column
/// its prefix pushed the open bracket to. Hanging the body under the
/// bracket produced a skinny right-margin tower the moment a
/// `INDEX(…):INDEX(…)` range broke inside a deep LET value — legal, but
/// most of the width sat unused. One rule, every width: narrowing the
/// window changes where lines break, not the geometry.
#[test]
fn chain_tail_bodies_take_block_indent_not_bracket_column() {
    assert_eq!(
        fmt_w(
            "=LET(above,INDEX($A:H,firstRow,relCol):INDEX($A:H,prevRow,relCol),above)",
            50
        )
        .unwrap(),
        "=LET(\n  above,\n    INDEX($A:H, firstRow, relCol):INDEX(\n        $A:H,\n        prevRow,\n        relCol\n      ),\n  above\n)\n"
    );
    // short prefixes clamp too — same shape, one rule
    assert_eq!(
        fmt_w("=SUM(B6:INDEX(longArgumentName1,longArgumentName2))", 30).unwrap(),
        "=SUM(\n  B6:INDEX(\n      longArgumentName1,\n      longArgumentName2\n    )\n)\n"
    );
}
