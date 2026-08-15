use crate::error::{err, Error};
use crate::Decimal;

// ──────────────────────────────────────────────────────────── tokenize ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Number, name, cell/range reference, `'quoted sheet'`, `#REF!`, or a
    /// name glued to a table selector / chip field (`Table1[Column 1]`,
    /// `A1.[email]`) — the selector bytes ride inside the token.
    Atom,
    /// `"..."` string literal, stored with its quotes and `""` escapes intact.
    Str,
    /// `+ - * / ^ & = < > <= >= <> % : !`
    Op,
    /// `(` or `{`
    Open,
    /// `)` or `}`
    Close,
    /// `,` or `;`
    Sep,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub text: String,
    pub kind: Kind,
    /// A blank line stood before this token in the source. Only consulted
    /// for `LET`-style pair layout, where it reproduces the author's
    /// logical grouping. Never synthesised.
    pub blank_before: bool,
    pub pos: usize,
}

pub(crate) fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || matches!(c, '_' | '$' | '\\')
}

pub(crate) fn is_ident_body(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '\\' | '.')
}

/// Break a formula into tokens, preserving every byte of every token.
///
/// # Errors
///
/// Returns an error for an unterminated string or quoted sheet name, a
/// character that cannot start any token, or a `,` used as an argument
/// separator under [`crate::Decimal::Comma`].
// One dispatch chain over the token kinds; splitting it into per-kind
// helpers would scatter the shared cursor and read worse.
#[allow(clippy::too_many_lines)]
pub fn tokenize(src: &str, decimal: Decimal) -> Result<Vec<Token>, Error> {
    let dec = decimal.mark();
    let c: Vec<char> = src.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;
    let mut newlines = 0usize;

    while i < c.len() {
        let ch = c[i];

        // Whitespace: skipped, but a blank line is remembered for the next token.
        if ch.is_whitespace() {
            if ch == '\n' {
                newlines += 1;
            }
            i += 1;
            continue;
        }

        let start = i;
        let blank_before = newlines >= 2;
        newlines = 0;

        let (text, kind) = if ch == '"' {
            // String literal. `""` is an escaped quote, not a terminator.
            i += 1;
            loop {
                if i >= c.len() {
                    return err("unterminated string literal", start);
                }
                if c[i] == '"' {
                    if i + 1 < c.len() && c[i + 1] == '"' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            (c[start..i].iter().collect::<String>(), Kind::Str)
        } else if ch == '\'' {
            // Single-quoted sheet name, e.g. `'my sheet'!A1`. `''` escapes.
            i += 1;
            loop {
                if i >= c.len() {
                    return err("unterminated quoted sheet name", start);
                }
                if c[i] == '\'' {
                    if i + 1 < c.len() && c[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if ch == '#' {
            // Error literal: #REF! #N/A #DIV/0! #NAME? #NULL! #NUM! #VALUE!
            i += 1;
            while i < c.len() && (c[i].is_alphanumeric() || matches!(c[i], '_' | '/' | '.')) {
                i += 1;
            }
            if i < c.len() && matches!(c[i], '!' | '?') {
                i += 1;
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if ch.is_ascii_digit()
            // A leading decimal mark (`.5`) is ordinary US shorthand, but a
            // leading `,5` under Decimal::Comma is indistinguishable from a
            // stray separator. Requiring a digit in front means mixed-locale
            // input is reported rather than quietly reinterpreted.
            || (decimal == Decimal::Dot
                && ch == dec
                && i + 1 < c.len()
                && c[i + 1].is_ascii_digit())
        {
            // Number, including a trailing exponent. Under Decimal::Comma the
            // mark is `,`, so `1,5` is one token and never gets a space
            // inserted into it.
            //
            // At most one decimal mark: without this the scan is greedy and
            // `1,5,2,5` becomes a single bogus token instead of a number
            // followed by a stray separator (and `1.5.2` likewise in dot
            // mode). Stopping here lets the caller report the real problem.
            let mut seen_mark = false;
            while i < c.len() {
                if c[i].is_ascii_digit() {
                    i += 1;
                } else if c[i] == dec && !seen_mark {
                    seen_mark = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if i < c.len()
                && matches!(c[i], 'e' | 'E')
                && i + 1 < c.len()
                && (c[i + 1].is_ascii_digit()
                    || (matches!(c[i + 1], '+' | '-')
                        && i + 2 < c.len()
                        && c[i + 2].is_ascii_digit()))
            {
                i += if matches!(c[i + 1], '+' | '-') { 2 } else { 1 };
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if is_ident_start(ch) {
            // Name or reference. `!` and `:` are emitted as their own tight
            // operators, so `stock_data!P3` and `B6:B` rejoin on output.
            while i < c.len() && is_ident_body(c[i]) {
                i += 1;
            }
            // Structured table reference — Table1[Column 1], Table1[#ALL],
            // Table1[[#ALL],[Col 1]:[Col 3]] — and chip extraction, the
            // `.[field]` postfix (also on cells: A1.[email]). The whole
            // bracketed selector is swallowed into this atom, bytes
            // verbatim: inner commas, colons, and spaces are selector
            // syntax, not formula syntax, so layout must never reflow
            // them and the comma-locale separator guard must not see
            // them. The outer `while` glues each `.[field]` postfix (the
            // ident scan above already consumed the dot of a cell chip,
            // since `.` is an identifier-body character).
            //
            // Only balance and termination are checked — selector *shape*
            // (empty `[]`, a specifier in chip position) is not. The
            // grammar rejects those; a formatter preserves them, same
            // contract split as the `\` tolerance note in grammar.js.
            while i < c.len() && c[i] == '[' {
                let open = i;
                let mut depth = 0usize;
                while i < c.len() {
                    match c[i] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if i >= c.len() {
                    return err("unterminated table selector", open);
                }
                i += 1;
                if i + 1 < c.len() && c[i] == '.' && c[i + 1] == '[' {
                    i += 1;
                }
            }
            (c[start..i].iter().collect::<String>(), Kind::Atom)
        } else if i + 1 < c.len() && matches!((ch, c[i + 1]), ('<', '=' | '>') | ('>', '=')) {
            i += 2;
            (c[start..i].iter().collect::<String>(), Kind::Op)
        } else if matches!(
            ch,
            '+' | '-' | '*' | '/' | '^' | '&' | '=' | '<' | '>' | '%' | ':' | '!'
        ) {
            i += 1;
            (ch.to_string(), Kind::Op)
        } else if matches!(ch, '(' | '{') {
            i += 1;
            (ch.to_string(), Kind::Open)
        } else if matches!(ch, ')' | '}') {
            i += 1;
            (ch.to_string(), Kind::Close)
        } else if ch == ';' {
            i += 1;
            (ch.to_string(), Kind::Sep)
        } else if ch == ',' {
            match decimal {
                Decimal::Dot => {
                    i += 1;
                    (ch.to_string(), Kind::Sep)
                }
                // Reaching here means a `,` that is not part of a number, so
                // the input mixes locales. Guessing either way would corrupt
                // the formula silently; say so instead.
                Decimal::Comma => {
                    return err(
                        "',' is the decimal mark in this locale — separate arguments with ';' \
                         (or set decimal = dot)",
                        start,
                    );
                }
            }
        } else {
            return err(format!("unexpected character {ch:?}"), start);
        };

        out.push(Token {
            text,
            kind,
            blank_before,
            pos: start,
        });
    }

    Ok(out)
}
