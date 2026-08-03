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
    -w, --width <N>     Max line width before breaking (default: 88)
    -h, --help          Print this help
    -V, --version       Print version

EXIT CODES:
    0  success
    1  usage error
    2  the formula could not be parsed
";

const DEFAULT_WIDTH: usize = 88;

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
    let mut width = DEFAULT_WIDTH;
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
                width = v.parse().map_err(|_| format!("invalid width {v:?}"))?;
                if width == 0 {
                    return Err("--width must be greater than 0".into());
                }
            }
            "-" => path = None,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}\n\n{USAGE}"));
            }
            other => path = Some(other.to_string()),
        }
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
