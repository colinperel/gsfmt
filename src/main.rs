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
    several FILEs are only accepted together with --write. A FILE that is
    a directory needs --write and expands to every `.gsfx` file beneath
    it, recursively: hidden entries are skipped, and the walk never
    follows a symlinked directory — though naming one on the command
    line expands it, like `find -H`: an explicit argument is intent.

OPTIONS:
    -m, --minify        Collapse the formula onto a single line (a newline
                        inside a string literal is content and survives)
    -i, --write         Rewrite FILEs in place instead of printing to
                        stdout. A file whose formatting is already clean is
                        left untouched. Errors are reported per file and the
                        remaining files still format. A changed file is
                        replaced (new inode), like `sed -i`: permission bits
                        are preserved; ownership, ACLs, and extended
                        attributes are not.
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
      3. `width = <N>` in the nearest `.gsfmt` walking up from the first
         FILE (from the current directory when reading stdin)
      4. `width = <N>` in $GSFMT_CONFIG, else
         $XDG_CONFIG_HOME/gsfmt/config, else ~/.config/gsfmt/config

DECIMAL:
    Google Sheets takes this from the spreadsheet locale, and it cannot be
    inferred from the text -- a US `{1,2;3,4}` already contains both
    characters. Under `comma`, `1,5` is one number and arguments separate
    with `;`; a `,` used as a separator is an error rather than a silent
    rewrite. Under `dot`, a `;` between arguments is normalized to `,`,
    as Sheets itself does on entry (array rows keep their `;`). Resolved
    like width: --decimal, then $GSFMT_DECIMAL, then
    `decimal = <dot|comma>` in the project `.gsfmt`, then the user config
    file.

CONFIG FILES:
    Two layers, same `key = value` format (`#` comments, unknown keys
    ignored). A project `.gsfmt` — found by walking up from the first
    FILE, or from the current directory for stdin — beats the user
    config, so a checkout formats the same for everyone. Per key: flag,
    then $GSFMT_<KEY>, then project `.gsfmt`, then user config, then
    the default.

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

/// Nearest `.gsfmt` walking up from `anchor` — the first FILE argument, or
/// the current directory when reading stdin. Per-project settings (a
/// comma-locale spreadsheet, a house width) live here and beat the user
/// config, so a checkout formats the same for everyone.
fn project_config(anchor: Option<&str>) -> Option<(std::path::PathBuf, String)> {
    // Not `std::path::absolute`: that is stable since 1.79, above the MSRV.
    let start = match anchor.map(std::path::Path::new) {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => std::env::current_dir().ok()?.join(p),
        None => std::env::current_dir().ok()?,
    };
    let dir = if start.is_dir() {
        start.as_path()
    } else {
        start.parent()?
    };
    // Canonicalized so `..` components resolve for real before the walk: a
    // lexical climb would let `../sibling/f.gsfx` pass back through the
    // invocation directory and pick up a `.gsfmt` that is not actually an
    // ancestor of the file.
    let canon = std::fs::canonicalize(dir).ok()?;
    let mut dir = canon.as_path();
    loop {
        let cand = dir.join(".gsfmt");
        if let Ok(text) = std::fs::read_to_string(&cand) {
            return Some((cand, text));
        }
        dir = dir.parent()?;
    }
}

/// Flag wins, then the environment, then the config files in order (project
/// first, then user), then the default.
fn resolve<T>(
    flag: Option<T>,
    env_key: &str,
    config_key: &str,
    cfgs: &[&(std::path::PathBuf, String)],
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
    for (path, text) in cfgs {
        if let Some(raw) = value_from_config(text, config_key) {
            return parse(raw).map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    Ok(default)
}

/// Resolve every setting through flag → environment → project config →
/// user config → default.
fn resolve_options(
    anchor: Option<&str>,
    width_flag: Option<usize>,
    decimal_flag: Option<gsfmt::Decimal>,
    uppercase_flag: Option<bool>,
) -> Result<gsfmt::Options, String> {
    let project = project_config(anchor);
    let user = config_text();
    let cfg: Vec<&(std::path::PathBuf, String)> = project.iter().chain(user.iter()).collect();
    let width = resolve(
        width_flag,
        "GSFMT_WIDTH",
        "width",
        &cfg,
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
        &cfg,
        parse_decimal,
        gsfmt::Decimal::default(),
    )?;
    let uppercase_functions = resolve(
        uppercase_flag,
        "GSFMT_UPPERCASE",
        "uppercase",
        &cfg,
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

    // Project config anchors at the first FILE; stdin anchors at the cwd.
    let anchor = inputs.iter().flatten().next().map(String::as_str);
    let opts = resolve_options(anchor, width_flag, decimal_flag, uppercase_flag)?;
    let render = |src: &str| {
        if minify {
            gsfmt::minify(src, &opts)
        } else {
            gsfmt::format(src, &opts)
        }
    };

    if write {
        return write_files(&inputs, &render);
    }

    // Filter mode: exactly one input. Silently formatting only the last of
    // several would quietly ignore the user's files.
    if inputs.len() > 1 {
        return Err("several FILEs need --write; without it, pass one".into());
    }
    let src = if let Some(Some(p)) = inputs.first() {
        if std::fs::metadata(p).is_ok_and(|m| m.is_dir()) {
            return Err(format!("{p}: is a directory (directories need --write)"));
        }
        std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("stdin: {e}"))?;
        buf
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

/// In-place mode: every input must be a real file or a directory (which
/// expands to the `.gsfx` files beneath it), and errors are reported per
/// file so one bad formula doesn't block the rest.
fn write_files(
    inputs: &[Option<String>],
    render: &impl Fn(&str) -> Result<String, gsfmt::Error>,
) -> Result<ExitCode, String> {
    if inputs.is_empty() {
        return Err("--write requires at least one FILE".into());
    }
    if inputs.iter().any(Option::is_none) {
        return Err("--write cannot read stdin; pass FILEs".into());
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut worst = 0u8;
    for p in inputs.iter().flatten() {
        // Deliberately `metadata`, not `symlink_metadata`: a directory
        // symlink NAMED as an argument is expanded (`find -H` semantics —
        // an explicit argument is intent), while the walk below never
        // follows one it merely encounters.
        if std::fs::metadata(p).is_ok_and(|m| m.is_dir()) {
            // Traversal failures (an unreadable subtree, a vanished entry)
            // are reported like any other per-file error: the rest of the
            // tree and every other input still format.
            let mut errors = Vec::new();
            collect_gsfx(std::path::Path::new(p), &mut files, &mut errors);
            for e in errors {
                eprintln!("gsfmt: {e}");
                worst = worst.max(1);
            }
        } else {
            files.push(p.into());
        }
    }
    for p in &files {
        if let Err(e) = write_in_place(p, render) {
            match e {
                WriteError::Io(msg) => {
                    eprintln!("gsfmt: {msg}");
                    worst = worst.max(1);
                }
                WriteError::Parse(e) => {
                    eprintln!("gsfmt: {}: {e}", p.display());
                    worst = worst.max(2);
                }
            }
        }
    }
    Ok(ExitCode::from(worst))
}

/// Recursively collect every `.gsfx` file under `dir`, sorted by name so a
/// run visits files in a deterministic order. Hidden entries are skipped
/// and symlinked directories are not followed, so a link cycle cannot hang
/// the walk; a symlink *to* a `.gsfx` file is still formatted. Failures
/// land in `errors` and the walk keeps going, so one unreadable subtree
/// cannot block the rest of the tree.
fn collect_gsfx(
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
    errors: &mut Vec<String>,
) {
    let iter = match std::fs::read_dir(dir) {
        Ok(iter) => iter,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    let mut entries = Vec::new();
    for entry in iter {
        match entry {
            Ok(entry) => entries.push(entry),
            Err(e) => errors.push(format!("{}: {e}", dir.display())),
        }
    }
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if entry.file_name().as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Err(e) => errors.push(format!("{}: {e}", path.display())),
            Ok(ft) if ft.is_dir() => collect_gsfx(&path, out, errors),
            Ok(_) if path.extension().is_some_and(|x| x == "gsfx") => out.push(path),
            Ok(_) => {}
        }
    }
}

/// Format one file in place. Already-clean files are left untouched; a
/// changed one is written to a sibling temp file and renamed over the
/// original, so a crash mid-write cannot leave a truncated formula behind.
///
/// The temp file is opened with `create_new`, which refuses to follow an
/// existing file *or symlink* at the candidate path — a predictable fixed
/// sidecar name could be pre-placed to make this clobber an unrelated
/// file. On Unix the temp is *created* with the original's mode (the
/// umask can only narrow that, never widen it), so no window exists where
/// a `0600` formula's fresh copy sits world-readable while its content is
/// written; the exact bits are then restored before the rename. Elsewhere
/// the original's permissions are applied right after creation, before
/// any content lands. Ownership, ACLs, and extended attributes are beyond
/// what std can carry — `--help` documents the replacement as
/// `sed -i`-like.
fn write_in_place(
    path: &std::path::Path,
    render: &impl Fn(&str) -> Result<String, gsfmt::Error>,
) -> Result<(), WriteError> {
    let io =
        |e: std::io::Error, what: &str| WriteError::Io(format!("{}: {what}: {e}", path.display()));
    let src = std::fs::read_to_string(path).map_err(|e| io(e, "read"))?;
    let out = render(&src).map_err(WriteError::Parse)?;
    if out == src {
        return Ok(());
    }
    let perms = std::fs::metadata(path)
        .map_err(|e| io(e, "stat"))?
        .permissions();
    let mut open_opts = std::fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        open_opts.mode(perms.mode());
    }
    let mut tmp: Option<std::path::PathBuf> = None;
    for n in 0u32..100 {
        // The original name plus a suffix, kept as OsString end to end so a
        // non-UTF-8 filename lands back on the same file, never a lossy
        // `�`-mangled sibling.
        let mut name = path
            .file_name()
            .map_or_else(Default::default, std::ffi::OsStr::to_os_string);
        name.push(format!(".gsfmt-{}-{n}~", std::process::id()));
        let cand = path.with_file_name(name);
        match open_opts.open(&cand) {
            Ok(mut f) => {
                #[cfg(not(unix))]
                if let Err(e) = f.set_permissions(perms.clone()) {
                    drop(f);
                    let _ = std::fs::remove_file(&cand);
                    return Err(io(e, "chmod"));
                }
                if let Err(e) = f.write_all(out.as_bytes()) {
                    drop(f);
                    let _ = std::fs::remove_file(&cand);
                    return Err(io(e, "write"));
                }
                tmp = Some(cand);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e, "create")),
        }
    }
    let Some(tmp) = tmp else {
        return Err(WriteError::Io(format!(
            "{}: could not create a temporary file beside it",
            path.display()
        )));
    };
    let finish = std::fs::set_permissions(&tmp, perms)
        .map_err(|e| io(e, "chmod"))
        .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| io(e, "rename")));
    if finish.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    finish
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
