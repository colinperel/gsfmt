# gsfmt

Formatter for [Google Sheets](https://sheets.google.com) formulas.
LET/LAMBDA aware: aligns binding values to the tightest column that
fits, breaks long calls across lines at a configurable width, and
collapses back to a single line with `--minify` for pasting into a
cell (a newline inside a string literal is content, not layout, and
survives even minify).

```sh
gsfmt formula.gsfx           # format to stdout (stdin when no file)
gsfmt --minify formula.gsfx  # one line, ready to paste into Sheets
gsfmt --width 100 -          # format stdin at width 100
gsfmt --write *.gsfx         # format files in place (clean ones untouched)
gsfmt --write formulas/      # every .gsfx beneath the directory, recursively
```

Formatting is idempotent (formatting formatted output is a no-op) and
byte-preserving for content: only whitespace changes, never tokens —
with two exceptions, both mirroring what the Sheets editor does on
entry: in the dot locale, `;` argument separators are normalized to `,`
(array row separators like `{1;2}` are untouched), and with
`--uppercase` function names are rewritten to uppercase (`sum(` →
`SUM(`; names bound by LET or LAMBDA keep their authored case, since
those are user identifiers, not builtins). A leading UTF-8 BOM is a
file-encoding artifact, not formula text, and is dropped. Table
references (`Table1[Column 1]`, `.[chip]` postfixes) are treated as
opaque atoms — laid out but never reflowed internally.

The same opacity applies to language embedded *inside* a string —
`QUERY`'s SELECT text is the common case. String bytes are content, so
gsfmt never reflows them: format the embedded query by hand (real
newlines are fine) and both `format` and `--minify` carry it through
byte-for-byte. One consequence: the query's interior indentation is
absolute, so it does not re-align when the surrounding call moves to a
different depth.

## Installation

```sh
cargo install gsfmt --locked
```

Or grab a prebuilt binary from the
[releases page](https://github.com/colinperel/gsfmt/releases) — Linux
(x86_64/aarch64), macOS (Intel/Apple silicon), and Windows archives,
each with a `SHA256SUMS` alongside. Building from a checkout works
too (`cargo build --release`); the crate has zero dependencies.

## Configuration

Resolved per setting from: flag → environment → project `.gsfmt` →
user config file → built-in.

| Setting | Flag | Env | Config key | Default |
|---------|------|-----|------------|---------|
| line width | `--width <N>` | `$GSFMT_WIDTH` | `width = <N>` | 82 |
| decimal mark | `--decimal <dot\|comma>` | `$GSFMT_DECIMAL` | `decimal = <dot\|comma>` | `dot` |
| uppercase functions | `--uppercase` | `$GSFMT_UPPERCASE` | `uppercase = <true\|false>` | `false` |

The user config file is `$GSFMT_CONFIG`, else
`$XDG_CONFIG_HOME/gsfmt/config`, else `~/.config/gsfmt/config` —
`key = value` lines, `#` comments, unknown keys ignored.

A project can pin its own settings in a `.gsfmt` file (same format),
found by walking up from the first FILE argument — or from the current
directory when reading stdin, which is what editor integrations hit.
It beats the user config, so a comma-locale spreadsheet's checkout
formats the same for everyone without env vars or flags.

`decimal` decides what `,` means: under `dot`, arguments separate with
`,`; under `comma` (de/fr/es locales), numbers use `1,5` and arguments
separate with `;`. A `,` outside a number under `comma` is an error
rather than a silent reinterpretation.

## Editor integration

Formulas live in `.gsfx` scratch files (see
[tree-sitter-gsformula](https://github.com/colinperel/tree-sitter-gsformula)
for the grammar and editor queries). In Neovim, wire gsfmt as the
`gsformula` formatter in conform.nvim, or just `:%!gsfmt`.

The editor grammar is dot-locale only and stricter than the
formatter: comma-locale input may highlight as an error in the
buffer even though gsfmt formats it fine.

## Development

```sh
cargo test      # unit + golden-file + CLI suites
cargo clippy --all-targets -- -D warnings
```

`tests/data/` holds gsfmt-formatted goldens paired with `--minify`
variants; the suite asserts fixed-point formatting, minify round-trips,
and the width invariant against them.

## Provenance

Extracted from my dotfiles with history preserved; built alongside the
tree-sitter-gsformula grammar.
