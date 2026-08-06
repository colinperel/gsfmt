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
```

Formatting is idempotent (formatting formatted output is a no-op) and
byte-preserving for content: only whitespace changes, never tokens —
with one exception: in the dot locale, `;` argument separators are
normalized to `,`, exactly what the Sheets editor does on entry (array
row separators like `{1;2}` are untouched). Table references
(`Table1[Column 1]`, `.[chip]` postfixes) are treated as opaque atoms —
laid out but never reflowed internally.

## Configuration

Resolved per setting from: flag → environment → config file → built-in.

| Setting | Flag | Env | Config key | Default |
|---------|------|-----|------------|---------|
| line width | `--width <N>` | `$GSFMT_WIDTH` | `width = <N>` | 82 |
| decimal mark | `--decimal <dot\|comma>` | `$GSFMT_DECIMAL` | `decimal = <dot\|comma>` | `dot` |

The config file is `$GSFMT_CONFIG`, else
`$XDG_CONFIG_HOME/gsfmt/config`, else `~/.config/gsfmt/config` —
`key = value` lines, `#` comments, unknown keys ignored.

`decimal` decides what `,` means: under `dot`, arguments separate with
`,`; under `comma` (de/fr/es locales), numbers use `1,5` and arguments
separate with `;`. A `,` outside a number under `comma` is an error
rather than a silent reinterpretation.

## Editor integration

Formulas live in `.gsfx` scratch files (see
[tree-sitter-gsformula](https://github.com/colinperel/tree-sitter-gsformula)
for the grammar and editor queries). In Neovim, wire gsfmt as the
`gsformula` formatter in conform.nvim, or just `:%!gsfmt`.

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
