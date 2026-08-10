//! `gsfmt` CLI — a filter over Google Sheets formulas.
//!
//! Reads stdin (or a file argument) and writes the formatted formula to
//! stdout, so it works as `:%!gsfmt` in vim and as a conform.nvim formatter.

use std::io::{Read, Write};
use std::process::ExitCode;

const USAGE: &str = "\
gsfmt — format Google Sheets formulas

USAGE:
    gsfmt [OPTIONS] [FILE...]

    Reads stdin when FILE is absent or `-`. Writes to stdout.
    With --write, formats each FILE in place instead (stdin not allowed);
    several FILEs are only accepted together with --write.

OPTIONS:
    -m, --minify        Collapse the formula onto a single line (a newline
                        inside a string literal is content and survives)
    -i, --write         Rewrite FILEs in place instead of printing to
                        stdout. A file whose formatting is already clean is
                        left untouched. Errors are reported per file and the
                        remaining files still format.
    -w, --width <N>     Target line width before breaking
    -d, --decimal <K>   Decimal mark: `dot` (1.5, args by `,`) or
                        `comma` (1,5, args by `;`). Default: dot
    -u, --uppercase     Uppercase function names (`sum(` -> `SUM(`), as the
                        Sheets editor does on entry. Names bound by LET or
                        LAMBDA keep their authored case. Also resolved from
                        $GSFMT_UPPERCASE or `uppercase = <true|false>` in
                        the config file. Default: off
    -h, --help          Print this help
    -V, --version       Print version

WIDTH:
    A target, not a hard ceiling: a single token that cannot be broken --
    a long name, reference, or string literal -- still prints in full and
    may overshoot.

    Resolved from the first of these that is set (default 82):
      1. --width <N>
      2. $GSFMT_WIDTH
      3. `width = <N>` in $GSFMT_CONFIG, else
         $XDG_CONFIG_HOME/gsfmt/config, else ~/.config/gsfmt/config

DECIMAL:
    Google Sheets takes this from the spreadsheet locale, and it cannot be
    inferred from the text -- a US `{1,2;3,4}` already contains both
    characters. Under `comma`, `1,5` is one number and arguments separate
    with `;`; a `,` used as a separator is an error rather than a silent
    rewrite. Under `dot`, a `;` between arguments is normalized to `,`,
    as Sheets itself does on entry (array rows keep their `;`). Resolved
    like width: --decimal, then $GSFMT_DECIMAL, then
    `decimal = <dot|comma>` in the config file.

EXIT CODES:
    0  success
    1  usage error, or a FILE could not be read or written
    2  a formula could not be parsed
";

/// Pull `<key> = <value>` out of a config file. Blank lines and `#` comments are
/// ignored; unknown keys are ignored too, so a newer config stays usable with
/// an older binary.
fn value_from_config<'a>(text: &'a str, want: &str) -> Option<&'a str> {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == want {
            return Some(value.trim());
        }
    }
    None
}

fn parse_bool(raw: &str) -> Result<bool, String> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("invalid value {other:?} (expected true or false)")),
    }
}

fn parse_decimal(raw: &str) -> Result<gsfmt::Decimal, String> {
    match raw {
        "dot" => Ok(gsfmt::Decimal::Dot),
        "comma" => Ok(gsfmt::Decimal::Comma),
        other => Err(format!("invalid decimal {other:?} (expected dot or comma)")),
    }
}

/// Config path: `$GSFMT_CONFIG`, else `$XDG_CONFIG_HOME/gsfmt/config`, else
/// `~/.config/gsfmt/config`.
fn config_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("GSFMT_CONFIG") {
        return Some(p.into());
    }
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(std::path::Path::new(&dir).join("gsfmt/config"));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::Path::new(&h).join(".config/gsfmt/config"))
}

/// Read the config file once; every setting resolves against it.
fn config_text() -> Option<(std::path::PathBuf, String)> {
    let path = config_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    Some((path, text))
}

/// Flag wins, then the environment, then the config file, then the default.
fn resolve<T>(
    flag: Option<T>,
    env_key: &str,
    config_key: &str,
    cfg: Option<&(std::path::PathBuf, String)>,
    parse: impl Fn(&str) -> Result<T, String>,
    default: T,
) -> Result<T, String> {
    if let Some(v) = flag {
        return Ok(v);
    }
    if let Ok(raw) = std::env::var(env_key) {
        let raw = raw.trim();
        if !raw.is_empty() {
            return parse(raw).map_err(|e| format!("{env_key}: {e}"));
        }
    }
    if let Some((path, text)) = cfg {
        if let Some(raw) = value_from_config(text, config_key) {
            return parse(raw).map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    Ok(default)
}

/// Resolve every setting through flag → environment → config file → default.
fn resolve_options(
    width_flag: Option<usize>,
    decimal_flag: Option<gsfmt::Decimal>,
    uppercase_flag: Option<bool>,
) -> Result<gsfmt::Options, String> {
    let cfg = config_text();
    let width = resolve(
        width_flag,
        "GSFMT_WIDTH",
        "width",
        cfg.as_ref(),
        |raw| {
            raw.parse::<usize>()
                .map_err(|_| format!("invalid width {raw:?}"))
        },
        gsfmt::DEFAULT_WIDTH,
    )?;
    if width == 0 {
        return Err("width must be greater than 0".into());
    }
    let decimal = resolve(
        decimal_flag,
        "GSFMT_DECIMAL",
        "decimal",
        cfg.as_ref(),
        parse_decimal,
        gsfmt::Decimal::default(),
    )?;
    let uppercase_functions = resolve(
        uppercase_flag,
        "GSFMT_UPPERCASE",
        "uppercase",
        cfg.as_ref(),
        parse_bool,
        false,
    )?;
    Ok(gsfmt::Options {
        width,
        decimal,
        uppercase_functions,
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("gsfmt: {msg}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut minify = false;
    let mut write = false;
    let mut width_flag: Option<usize> = None;
    let mut decimal_flag: Option<gsfmt::Decimal> = None;
    let mut uppercase_flag: Option<bool> = None;
    // `None` in the list means stdin (`-`).
    let mut inputs: Vec<Option<String>> = Vec::new();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("gsfmt {}", env!("CARGO_PKG_VERSION"));
                return Ok(ExitCode::SUCCESS);
            }
            "-m" | "--minify" => minify = true,
            "-i" | "--write" => write = true,
            "-w" | "--width" => {
                let v = args.next().ok_or("--width requires a value")?;
                width_flag = Some(v.parse().map_err(|_| format!("invalid width {v:?}"))?);
            }
            "-d" | "--decimal" => {
                let v = args.next().ok_or("--decimal requires a value")?;
                decimal_flag = Some(parse_decimal(&v)?);
            }
            "-u" | "--uppercase" => uppercase_flag = Some(true),
            // A bare `-` means stdin; anything else starting with `-` is a
            // typo'd flag, not a filename.
            other if other != "-" && other.starts_with('-') => {
                return Err(format!("unknown option {other:?}\n\n{USAGE}"));
            }
            other => inputs.push((other != "-").then(|| other.to_string())),
        }
    }

    let opts = resolve_options(width_flag, decimal_flag, uppercase_flag)?;
    let render = |src: &str| {
        if minify {
            gsfmt::minify(src, &opts)
        } else {
            gsfmt::format(src, &opts)
        }
    };

    if write {
        // In-place mode: every input must be a real file, and errors are
        // reported per file so one bad formula doesn't block the rest.
        if inputs.is_empty() {
            return Err("--write requires at least one FILE".into());
        }
        if inputs.iter().any(Option::is_none) {
            return Err("--write cannot read stdin; pass FILEs".into());
        }
        let mut worst = 0u8;
        for p in inputs.iter().flatten() {
            if let Err(e) = write_in_place(p, &render) {
                match e {
                    WriteError::Io(msg) => {
                        eprintln!("gsfmt: {msg}");
                        worst = worst.max(1);
                    }
                    WriteError::Parse(e) => {
                        eprintln!("gsfmt: {p}: {e}");
                        worst = worst.max(2);
                    }
                }
            }
        }
        return Ok(ExitCode::from(worst));
    }

    // Filter mode: exactly one input. Silently formatting only the last of
    // several would quietly ignore the user's files.
    if inputs.len() > 1 {
        return Err("several FILEs need --write; without it, pass one".into());
    }
    let src = match inputs.first() {
        Some(Some(p)) => std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?,
        _ => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("stdin: {e}"))?;
            buf
        }
    };

    match render(&src) {
        Ok(out) => {
            std::io::stdout()
                .write_all(out.as_bytes())
                .map_err(|e| format!("stdout: {e}"))?;
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("gsfmt: {e}");
            Ok(ExitCode::from(2))
        }
    }
}

enum WriteError {
    Io(String),
    Parse(gsfmt::Error),
}

/// Format one file in place. Already-clean files are left untouched; a
/// changed one is written to a sibling temp file and renamed over the
/// original, so a crash mid-write cannot leave a truncated formula behind.
fn write_in_place(
    path: &str,
    render: &impl Fn(&str) -> Result<String, gsfmt::Error>,
) -> Result<(), WriteError> {
    let io = |e: std::io::Error, what: &str| WriteError::Io(format!("{path}: {what}: {e}"));
    let src = std::fs::read_to_string(path).map_err(|e| io(e, "read"))?;
    let out = render(&src).map_err(WriteError::Parse)?;
    if out == src {
        return Ok(());
    }
    let tmp = format!("{path}.gsfmt~");
    std::fs::write(&tmp, &out).map_err(|e| io(e, "write"))?;
    std::fs::rename(&tmp, path).map_err(|e| io(e, "rename"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_bool, parse_decimal, value_from_config};

    #[test]
    fn config_values_are_parsed() {
        assert_eq!(value_from_config("width = 70", "width"), Some("70"));
        assert_eq!(value_from_config("  width=70  ", "width"), Some("70"));
        assert_eq!(
            value_from_config("decimal = comma", "decimal"),
            Some("comma")
        );
    }

    #[test]
    fn comments_and_unknown_keys_are_ignored() {
        let text = "# gsfmt config\n\nfuture_key = yes\nwidth = 96  # trailing\n";
        assert_eq!(value_from_config(text, "width"), Some("96"));
        assert_eq!(value_from_config("# width = 70", "width"), None);
        assert_eq!(value_from_config("indent = 4", "width"), None);
    }

    #[test]
    fn bool_values_are_validated() {
        assert!(parse_bool("true").unwrap());
        assert!(!parse_bool("false").unwrap());
        assert!(parse_bool("yes").is_err());
    }

    #[test]
    fn decimal_values_are_validated() {
        assert_eq!(parse_decimal("dot").unwrap(), gsfmt::Decimal::Dot);
        assert_eq!(parse_decimal("comma").unwrap(), gsfmt::Decimal::Comma);
        assert!(parse_decimal("period").is_err());
    }
}
