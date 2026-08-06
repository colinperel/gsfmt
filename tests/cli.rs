//! CLI-surface tests: argument handling, exit codes, and width resolution.
//!
//! These drive the built binary rather than the library, because the
//! precedence chain (flag > env > config file > default) and the exit codes
//! are part of the contract conform.nvim and `:%!gsfmt` rely on.

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_gsfmt");

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], env: &[(&str, &str)], stdin: &str) -> Out {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        // Keep the developer's own ~/.config/gsfmt/config out of the test.
        .env("XDG_CONFIG_HOME", "/nonexistent-gsfmt-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Clear every GSFMT_* variable rather than listing them: an enumerated
    // list silently stops isolating the moment a new option is added, which
    // is exactly how GSFMT_DECIMAL started leaking in. Tests opt back in
    // explicitly via `env` below.
    for (key, _) in std::env::vars() {
        if key.starts_with("GSFMT_") {
            cmd.env_remove(key);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn gsfmt");
    // A usage error (bad flag, second FILE) makes gsfmt exit before it ever
    // reads stdin, so this write races the child's exit and can land on a
    // closed pipe. That is expected here and says nothing about the binary;
    // any other write error still fails the test.
    if let Err(e) = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
    {
        assert_eq!(
            e.kind(),
            std::io::ErrorKind::BrokenPipe,
            "unexpected error writing stdin: {e}"
        );
    }
    let out = child.wait_with_output().expect("wait");
    Out {
        code: out.status.code().expect("exit code"),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn formats_stdin_by_default() {
    let out = run(&[], &[], "=LET(x,1,x+1)\n");
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "=LET(\n  x, 1,\n  x + 1\n)\n");
}

#[test]
fn a_bare_dash_means_stdin() {
    let out = run(&["-"], &[], "=SUM(A1)\n");
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "=SUM(A1)\n");
}

#[test]
fn minify_collapses_to_one_line() {
    let out = run(&["--minify"], &[], "=LET(\n  x, 1,\n  x + 1\n)\n");
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "=LET(x,1,x+1)\n");
}

/// Formatting only the last of several files would quietly ignore the rest.
#[test]
fn a_second_file_argument_is_rejected() {
    let dir = std::env::temp_dir().join("gsfmt-cli-test");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.gsfx");
    let b = dir.join("b.gsfx");
    std::fs::write(&a, "=SUM(A1)\n").unwrap();
    std::fs::write(&b, "=SUM(B2)\n").unwrap();

    let out = run(&[a.to_str().unwrap(), b.to_str().unwrap()], &[], "");
    assert_eq!(out.code, 1, "stdout was: {}", out.stdout);
    assert!(out.stderr.contains("only one FILE"), "{}", out.stderr);

    // one file is still fine
    let ok = run(&[a.to_str().unwrap()], &[], "");
    assert_eq!(ok.code, 0);
    assert_eq!(ok.stdout, "=SUM(A1)\n");
}

#[test]
fn unknown_options_are_rejected() {
    let out = run(&["--bogus"], &[], "");
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("unknown option"), "{}", out.stderr);
}

#[test]
fn an_unparseable_formula_exits_two_without_writing_output() {
    let out = run(&[], &[], "=SUM(A1\n");
    assert_eq!(out.code, 2, "parse failures must not look like success");
    assert!(out.stdout.is_empty(), "stdout was: {}", out.stdout);
    assert!(out.stderr.contains("unbalanced"), "{}", out.stderr);
}

/// Nesting past the parser's depth cap is an ordinary parse failure — exit 2
/// with nothing on stdout — never a stack-overflow abort that conform would
/// surface as a crashed formatter.
#[test]
fn deeply_nested_input_exits_two_without_output() {
    let deep = format!("={}1{}\n", "(".repeat(300), ")".repeat(300));
    let out = run(&[], &[], &deep);
    assert_eq!(out.code, 2, "stdout was: {}", out.stdout);
    assert!(out.stdout.is_empty(), "stdout was: {}", out.stdout);
    assert!(out.stderr.contains("nesting"), "{}", out.stderr);
}

#[test]
fn width_precedence_is_flag_then_env_then_config() {
    // 54 chars: fits at 82, breaks at 40.
    let f = "=IFS(alpha, oneValue, beta, twoValue, gamma, threeVal)\n";
    let dir = std::env::temp_dir().join("gsfmt-cli-test");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config");
    std::fs::write(&cfg, "# comment\nwidth = 40\n").unwrap();
    let cfg = cfg.to_str().unwrap();

    let inline = |o: &Out| o.stdout.lines().count() == 1;

    assert!(inline(&run(&[], &[], f)), "default 82 should fit");
    assert!(
        !inline(&run(&[], &[("GSFMT_CONFIG", cfg)], f)),
        "config width=40 should break"
    );
    assert!(
        inline(&run(
            &[],
            &[("GSFMT_CONFIG", cfg), ("GSFMT_WIDTH", "100")],
            f
        )),
        "env must beat config"
    );
    assert!(
        !inline(&run(
            &["--width", "24"],
            &[("GSFMT_CONFIG", cfg), ("GSFMT_WIDTH", "100")],
            f
        )),
        "flag must beat env and config"
    );
}

#[test]
fn decimal_locale_is_selectable_and_defaults_to_dot() {
    let f = "=SUM(1,5;2,5)\n";
    assert_eq!(run(&[], &[], f).stdout, "=SUM(1, 5; 2, 5)\n");
    assert_eq!(
        run(&["--decimal", "comma"], &[], f).stdout,
        "=SUM(1,5; 2,5)\n"
    );
    assert_eq!(
        run(&[], &[("GSFMT_DECIMAL", "comma")], f).stdout,
        "=SUM(1,5; 2,5)\n"
    );
    // flag beats env
    assert_eq!(
        run(&["--decimal", "dot"], &[("GSFMT_DECIMAL", "comma")], f).stdout,
        "=SUM(1, 5; 2, 5)\n"
    );
}

#[test]
fn decimal_comes_from_the_config_file_too() {
    let dir = std::env::temp_dir().join("gsfmt-cli-test");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("decimal-config");
    std::fs::write(&cfg, "decimal = comma\n").unwrap();
    let out = run(
        &[],
        &[("GSFMT_CONFIG", cfg.to_str().unwrap())],
        "=SUM(1,5;2,5)\n",
    );
    assert_eq!(out.stdout, "=SUM(1,5; 2,5)\n");
}

#[test]
fn a_bad_decimal_value_is_a_usage_error() {
    let out = run(&["--decimal", "period"], &[], "=SUM(A1)\n");
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("invalid decimal"), "{}", out.stderr);

    let out = run(&[], &[("GSFMT_DECIMAL", "nope")], "=SUM(A1)\n");
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("GSFMT_DECIMAL"), "{}", out.stderr);
}

/// Mixed-locale input exits 2 with nothing on stdout, so conform surfaces a
/// format error instead of writing a reinterpreted formula into the buffer.
#[test]
fn mixed_locale_input_exits_two_without_output() {
    let out = run(&["--decimal", "comma"], &[], "=SUM(A1,A2)\n");
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "stdout was: {}", out.stdout);
    assert!(out.stderr.contains("decimal mark"), "{}", out.stderr);
}

/// A malformed width is a usage error, not a silent fall back to the default.
#[test]
fn bad_width_values_are_usage_errors() {
    let out = run(&[], &[("GSFMT_WIDTH", "wide")], "=SUM(A1)\n");
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("GSFMT_WIDTH"), "{}", out.stderr);

    let dir = std::env::temp_dir().join("gsfmt-cli-test");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("bad-config");
    std::fs::write(&cfg, "width = huge\n").unwrap();
    let out = run(
        &[],
        &[("GSFMT_CONFIG", cfg.to_str().unwrap())],
        "=SUM(A1)\n",
    );
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("invalid width"), "{}", out.stderr);
}
