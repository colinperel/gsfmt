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
        .env_remove("GSFMT_WIDTH")
        .env_remove("GSFMT_CONFIG")
        // Keep the developer's own ~/.config/gsfmt/config out of the test.
        .env("XDG_CONFIG_HOME", "/nonexistent-gsfmt-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn gsfmt");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
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
    let a = dir.join("a.gsf");
    let b = dir.join("b.gsf");
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
