//! `gsfmt` CLI — a filter over Google Sheets formulas.
//!
//! Reads stdin (or a file argument) and writes the formatted formula to
//! stdout, so it works as `:%!gsfmt` in vim and as a conform.nvim formatter.

use std::io::{Read, Write};
use std::process::ExitCode;

const USAGE: &str = "\
gsfmt — format Google Sheets formulas

USAGE:
    gsfmt [OPTIONS] [FILE]

    Reads stdin when FILE is absent or `-`. Writes to stdout.

OPTIONS:
    -m, --minify        Collapse the formula onto a single line
    -w, --width <N>     Max line width before breaking
    -h, --help          Print this help
    -V, --version       Print version

WIDTH:
    Resolved from the first of these that is set (default 82):
      1. --width <N>
      2. $GSFMT_WIDTH
      3. `width = <N>` in $GSFMT_CONFIG, else
         $XDG_CONFIG_HOME/gsfmt/config, else ~/.config/gsfmt/config

EXIT CODES:
    0  success
    1  usage error
    2  the formula could not be parsed
";

const DEFAULT_WIDTH: usize = 82;

/// Pull `width = <N>` out of a config file. Blank lines and `#` comments are
/// ignored; unknown keys are ignored too, so a newer config stays usable with
/// an older binary.
fn width_from_config(text: &str) -> Option<Result<usize, String>> {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "width" {
            continue;
        }
        let value = value.trim();
        return Some(
            value
                .parse()
                .map_err(|_| format!("invalid width {value:?} in config")),
        );
    }
    None
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

/// `--width` wins, then `$GSFMT_WIDTH`, then the config file, then the default.
fn resolve_width(flag: Option<usize>) -> Result<usize, String> {
    if let Some(w) = flag {
        return Ok(w);
    }
    if let Ok(raw) = std::env::var("GSFMT_WIDTH") {
        let raw = raw.trim();
        if !raw.is_empty() {
            return raw
                .parse()
                .map_err(|_| format!("invalid GSFMT_WIDTH {raw:?}"));
        }
    }
    if let Some(path) = config_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(found) = width_from_config(&text) {
                return found.map_err(|e| format!("{}: {e}", path.display()));
            }
        }
    }
    Ok(DEFAULT_WIDTH)
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
    let mut width_flag: Option<usize> = None;
    let mut path: Option<String> = None;
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
            "-w" | "--width" => {
                let v = args.next().ok_or("--width requires a value")?;
                width_flag = Some(v.parse().map_err(|_| format!("invalid width {v:?}"))?);
            }
            "-" => path = None,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}\n\n{USAGE}"));
            }
            other => path = Some(other.to_string()),
        }
    }

    let width = resolve_width(width_flag)?;
    if width == 0 {
        return Err("width must be greater than 0".into());
    }

    let src = if let Some(p) = &path {
        std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("stdin: {e}"))?;
        buf
    };

    let result = if minify {
        gsfmt::minify(&src)
    } else {
        gsfmt::format(&src, width)
    };

    match result {
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

#[cfg(test)]
mod tests {
    use super::width_from_config;

    #[test]
    fn config_width_is_parsed() {
        assert_eq!(width_from_config("width = 70").unwrap().unwrap(), 70);
        assert_eq!(width_from_config("  width=70  ").unwrap().unwrap(), 70);
    }

    #[test]
    fn comments_and_unknown_keys_are_ignored() {
        let text = "# gsfmt config\n\nfuture_key = yes\nwidth = 96  # trailing\n";
        assert_eq!(width_from_config(text).unwrap().unwrap(), 96);
        assert!(width_from_config("# width = 70").is_none());
        assert!(width_from_config("indent = 4").is_none());
    }

    #[test]
    fn a_bad_width_is_an_error_not_a_silent_default() {
        assert!(width_from_config("width = wide").unwrap().is_err());
    }
}
