//! Seeded property sweep: hundreds of generated formulas, every core
//! invariant, at several widths.
//!
//! The generator is deterministic — a fixed-seed xorshift, no clock, no
//! platform entropy — so any failure reproduces verbatim from the panic
//! message, which carries the offending formula.

/// xorshift64: tiny, deterministic, good enough to shuffle a grammar.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).expect("usize fits u64");
        usize::try_from(self.next_u64() % n64).expect("modulo keeps it below n")
    }

    fn chance(&mut self, pct: u64) -> bool {
        self.next_u64() % 100 < pct
    }
}

/// Leaves cover every token shape the lexer knows: references (absolute,
/// sheet-qualified, quoted-sheet, open-ended), numbers in every spelling,
/// strings full of separator and bracket bytes plus `""` escapes, booleans,
/// error literals, multibyte names, table references with compound
/// selectors, chip extraction, and array literals.
const ATOMS: &[&str] = &[
    "A1",
    "$B$2",
    "'my sheet'!C3",
    "stock_data!$AE$3",
    "1.5",
    ".5",
    "1.5E+10",
    "50%",
    "\"s, (t); {u}\"",
    "\"say \"\"hi\"\"\"",
    "TRUE",
    "#N/A",
    "name_1",
    "näme",
    "Table1[Col 1]",
    "T2[[#ALL],[Col 1]:[Col 2]]",
    "A1.[email]",
    "B6:B",
    "{1, 2; 3, 4}",
];

const BINOPS: &[&str] = &[
    " + ", " - ", " * ", " / ", " & ", " <= ", " <> ", " = ", "^", ":",
];

const HEADS: &[&str] = &["SUM", "IF", "IFS", "LET", "SWITCH", "LAMBDA", "X.Y", "sum"];

fn gen_expr(rng: &mut Rng, depth: usize) -> String {
    if depth >= 5 {
        return ATOMS[rng.below(ATOMS.len())].to_string();
    }
    match rng.below(10) {
        0..=2 => ATOMS[rng.below(ATOMS.len())].to_string(),
        3 | 4 => format!(
            "{}{}{}",
            gen_expr(rng, depth + 1),
            BINOPS[rng.below(BINOPS.len())],
            gen_expr(rng, depth + 1)
        ),
        5 => format!("({})", gen_expr(rng, depth + 1)),
        6 => format!("-{}", gen_expr(rng, depth + 1)),
        // Adjacent operands: invalid in Sheets, but the formatter must
        // preserve the two tokens rather than fuse them.
        7 => format!(
            "{} {}",
            ATOMS[rng.below(ATOMS.len())],
            ATOMS[rng.below(ATOMS.len())]
        ),
        _ => {
            let head = HEADS[rng.below(HEADS.len())];
            let n = rng.below(4) + 1;
            let args: Vec<String> = (0..n)
                .map(|_| {
                    // Blank arguments are meaningful and must survive.
                    if rng.chance(15) {
                        String::new()
                    } else {
                        gen_expr(rng, depth + 1)
                    }
                })
                .collect();
            format!("{head}({})", args.join(","))
        }
    }
}

/// The invariants, per formula and width:
///   1. formatting succeeds (the generator emits only lexable text)
///   2. `format(format(x)) == format(x)`         — idempotence
///   3. `minify(format(x)) == minify(x)`          — semantic preservation
///   4. `minify(minify(x)) == minify(x)`          — minify idempotence
///   5. formatted output re-parses (2 and 3 both re-run the full pipeline)
///   6. no output line ends in whitespace
#[test]
fn generated_formulas_hold_every_invariant() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut skipped = 0usize;
    const CASES: usize = 400;

    for i in 0..CASES {
        let src = format!("={}", gen_expr(&mut rng, 0));

        let Ok(min0) = gsfmt::minify(&src, &gsfmt::Options::default()) else {
            // The generator should only produce lexable dot-locale text; a
            // rejection here is tolerated but counted, so a generator bug
            // cannot quietly skip the whole sweep.
            skipped += 1;
            continue;
        };
        let min_twice = gsfmt::minify(&min0, &gsfmt::Options::default())
            .unwrap_or_else(|e| panic!("case {i}: minify output rejected: {e}\n{src}"));
        assert_eq!(min_twice, min0, "minify not idempotent (case {i}):\n{src}");

        for width in [1, 10, 30, 82] {
            let opts = gsfmt::Options {
                width,
                ..gsfmt::Options::default()
            };
            let once = gsfmt::format(&src, &opts)
                .unwrap_or_else(|e| panic!("case {i} width {width}: {e}\n{src}"));
            let twice = gsfmt::format(&once, &opts).unwrap_or_else(|e| {
                panic!("case {i} width {width}: reformat rejected: {e}\n{src}")
            });
            assert_eq!(
                twice, once,
                "not idempotent (case {i}, width {width}):\n{src}"
            );

            let m = gsfmt::minify(&once, &gsfmt::Options::default())
                .unwrap_or_else(|e| panic!("case {i} width {width}: {e}\n{once}"));
            assert_eq!(
                m, min0,
                "semantics changed (case {i}, width {width}):\n{src}\n→\n{once}"
            );

            for line in once.lines() {
                assert_eq!(
                    line,
                    line.trim_end(),
                    "trailing whitespace (case {i}, width {width}):\n{src}"
                );
            }
        }
    }

    assert!(
        skipped < CASES / 10,
        "generator produced {skipped} unlexable formulas out of {CASES}"
    );
}
