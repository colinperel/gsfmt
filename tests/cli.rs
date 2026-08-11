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
    // Run from the temp dir, not the repo checkout, so a stray `.gsfmt`
    // in the developer's tree can't leak into stdin-anchored discovery.
    run_in(&std::env::temp_dir(), args, env, stdin)
}

fn run_in(cwd: &std::path::Path, args: &[&str], env: &[(&str, &str)], stdin: &str) -> Out {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(cwd)
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
    assert!(out.stderr.contains("need --write"), "{}", out.stderr);

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
    // dot locale reads four arguments and normalizes the `;` to `,`
    assert_eq!(run(&[], &[], f).stdout, "=SUM(1, 5, 2, 5)\n");
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
        "=SUM(1, 5, 2, 5)\n"
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

/// A project `.gsfmt` — the nearest one walking up from the FILE — beats
/// the user config, while env and flag still beat the project file. Stdin
/// anchors discovery at the current directory.
#[test]
fn project_config_is_discovered_and_layered() {
    // 54 chars: fits at 82 and 100, breaks at 40.
    let f = "=IFS(alpha, oneValue, beta, twoValue, gamma, threeVal)\n";
    let root = std::env::temp_dir().join("gsfmt-cli-project-test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join(".gsfmt"), "width = 40\n").unwrap();
    let file = root.join("nested/f.gsfx");
    std::fs::write(&file, f).unwrap();
    let user = root.join("user-config");
    std::fs::write(&user, "width = 100\n").unwrap();
    let user = user.to_str().unwrap();

    let multiline = |o: &Out| o.stdout.lines().count() > 1;

    // project width=40 wins over user config width=100
    let out = run(&[file.to_str().unwrap()], &[("GSFMT_CONFIG", user)], "");
    assert!(
        multiline(&out),
        "project must beat user config: {}",
        out.stdout
    );

    // env still beats the project file
    let out = run(&[file.to_str().unwrap()], &[("GSFMT_WIDTH", "100")], "");
    assert!(!multiline(&out), "env must beat project: {}", out.stdout);

    // stdin anchors at the cwd
    let out = run_in(&root, &[], &[("GSFMT_CONFIG", user)], f);
    assert!(multiline(&out), "stdin must anchor at cwd: {}", out.stdout);

    // and a cwd without a `.gsfmt` above it falls through to the user config
    let out = run(&[], &[("GSFMT_CONFIG", user)], f);
    assert!(
        !multiline(&out),
        "no project file: user config applies: {}",
        out.stdout
    );
}

/// A relative FILE with `..` components resolves before the ancestor walk:
/// the invocation directory's `.gsfmt` must not leak onto a sibling tree
/// that is not actually below it.
#[test]
fn relative_anchor_does_not_leak_the_invocation_dirs_config() {
    // 54 chars: fits at the default 82, breaks at 40.
    let f = "=IFS(alpha, oneValue, beta, twoValue, gamma, threeVal)\n";
    let root = std::env::temp_dir().join("gsfmt-cli-sibling-test");
    let _ = std::fs::remove_dir_all(&root);
    let proj_a = root.join("proj-a");
    let proj_b = root.join("proj-b");
    std::fs::create_dir_all(&proj_a).unwrap();
    std::fs::create_dir_all(&proj_b).unwrap();
    std::fs::write(proj_a.join(".gsfmt"), "width = 40\n").unwrap();
    std::fs::write(proj_b.join("f.gsfx"), f).unwrap();

    let multiline = |o: &Out| o.stdout.lines().count() > 1;

    // control: inside proj-a the config applies (stdin anchors at cwd)
    let out = run_in(&proj_a, &[], &[], f);
    assert!(multiline(&out), "control: {}", out.stdout);

    // the sibling file must resolve `..` and never see proj-a's config
    let out = run_in(&proj_a, &["../proj-b/f.gsfx"], &[], "");
    assert!(
        !multiline(&out),
        "proj-a's .gsfmt leaked onto a sibling: {}",
        out.stdout
    );
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

/// `--uppercase` resolves through the same flag > env > config chain as the
/// other settings and defaults to off.
#[test]
fn uppercase_is_selectable_and_defaults_to_off() {
    let out = run(&[], &[], "=sum(a1:a2)\n");
    assert_eq!(out.stdout, "=sum(a1:a2)\n");

    let out = run(&["-u"], &[], "=sum(a1:a2)\n");
    assert_eq!(out.stdout, "=SUM(a1:a2)\n");

    let out = run(&[], &[("GSFMT_UPPERCASE", "true")], "=sum(a1:a2)\n");
    assert_eq!(out.stdout, "=SUM(a1:a2)\n");

    let dir = std::env::temp_dir().join("gsfmt-cli-test");
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("uppercase-config");
    std::fs::write(&cfg, "uppercase = true\n").unwrap();
    let out = run(
        &[],
        &[("GSFMT_CONFIG", cfg.to_str().unwrap())],
        "=sum(a1)\n",
    );
    assert_eq!(out.stdout, "=SUM(a1)\n");

    let out = run(&[], &[("GSFMT_UPPERCASE", "yes")], "=sum(a1)\n");
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("GSFMT_UPPERCASE"), "{}", out.stderr);
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

/// `--write` formats every FILE in place, leaves already-clean files
/// untouched, keeps going past a bad formula (reporting it, exit 2), and
/// refuses stdin.
#[test]
fn write_mode_formats_files_in_place() {
    let dir = std::env::temp_dir().join("gsfmt-cli-write-test");
    std::fs::create_dir_all(&dir).unwrap();
    let messy = dir.join("messy.gsfx");
    let clean = dir.join("clean.gsfx");
    let broken = dir.join("broken.gsfx");
    std::fs::write(&messy, "=sum( A1,B2 )").unwrap();
    std::fs::write(&clean, "=SUM(A1, B2)\n").unwrap();
    std::fs::write(&broken, "=SUM(A1").unwrap();

    let out = run(
        &["--write", messy.to_str().unwrap(), clean.to_str().unwrap()],
        &[],
        "",
    );
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "", "in-place mode prints nothing");
    assert_eq!(std::fs::read_to_string(&messy).unwrap(), "=sum(A1, B2)\n");
    assert_eq!(std::fs::read_to_string(&clean).unwrap(), "=SUM(A1, B2)\n");

    // a parse failure is reported per file; the good file still formats
    std::fs::write(&messy, "=sum( A1,B2 )").unwrap();
    let out = run(
        &["-i", broken.to_str().unwrap(), messy.to_str().unwrap()],
        &[],
        "",
    );
    assert_eq!(out.code, 2, "stderr: {}", out.stderr);
    assert!(out.stderr.contains("broken.gsfx"), "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&messy).unwrap(), "=sum(A1, B2)\n");
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), "=SUM(A1");

    // stdin and empty FILE lists are usage errors
    let out = run(&["--write"], &[], "=SUM(A1)\n");
    assert_eq!(out.code, 1);
    let out = run(&["--write", "-"], &[], "=SUM(A1)\n");
    assert_eq!(out.code, 1);
}

/// A directory under `--write` expands to every `.gsfx` file beneath it:
/// nested files format, other extensions and hidden entries are left
/// alone. Without `--write` a directory is a usage error, not an opaque
/// "Is a directory" read failure.
#[test]
fn write_mode_expands_directories() {
    let dir = std::env::temp_dir().join("gsfmt-cli-write-dir-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("nested")).unwrap();
    std::fs::create_dir_all(dir.join(".hidden")).unwrap();
    let top = dir.join("top.gsfx");
    let nested = dir.join("nested/deep.gsfx");
    let other = dir.join("notes.txt");
    let hidden = dir.join(".hidden/skip.gsfx");
    std::fs::write(&top, "=sum( A1,B2 )").unwrap();
    std::fs::write(&nested, "=sum( C3 )").unwrap();
    std::fs::write(&other, "=sum( A1 )").unwrap();
    std::fs::write(&hidden, "=sum( A1 )").unwrap();

    let out = run(&["--write", dir.to_str().unwrap()], &[], "");
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(std::fs::read_to_string(&top).unwrap(), "=sum(A1, B2)\n");
    assert_eq!(std::fs::read_to_string(&nested).unwrap(), "=sum(C3)\n");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "=sum( A1 )");
    assert_eq!(std::fs::read_to_string(&hidden).unwrap(), "=sum( A1 )");

    // without --write a directory is rejected up front
    let out = run(&[dir.to_str().unwrap()], &[], "");
    assert_eq!(out.code, 1, "stdout was: {}", out.stdout);
    assert!(out.stderr.contains("need --write"), "{}", out.stderr);

    // a symlink cycle must not hang the walk (symlinked dirs not followed),
    // while a symlink to a .gsfx file still formats
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
        std::fs::write(&top, "=sum( A1,B2 )").unwrap();
        let link = dir.join("link.gsfx");
        std::os::unix::fs::symlink(&nested, &link).unwrap();
        std::fs::write(&nested, "=sum( C3 )").unwrap();

        let out = run(&["--write", dir.to_str().unwrap()], &[], "");
        assert_eq!(out.code, 0, "stderr: {}", out.stderr);
        assert_eq!(std::fs::read_to_string(&top).unwrap(), "=sum(A1, B2)\n");
        assert_eq!(std::fs::read_to_string(&nested).unwrap(), "=sum(C3)\n");

        // a directory symlink NAMED as the argument is expanded — an
        // explicit argument is intent (`find -H` semantics), unlike one
        // the walk merely encounters
        let real = std::env::temp_dir().join("gsfmt-cli-symlink-target");
        let _ = std::fs::remove_dir_all(&real);
        std::fs::create_dir_all(&real).unwrap();
        let via_link = real.join("t.gsfx");
        std::fs::write(&via_link, "=sum( D4 )").unwrap();
        let alias = std::env::temp_dir().join("gsfmt-cli-symlink-alias");
        let _ = std::fs::remove_file(&alias);
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let out = run(&["--write", alias.to_str().unwrap()], &[], "");
        assert_eq!(out.code, 0, "stderr: {}", out.stderr);
        assert_eq!(
            std::fs::read_to_string(&via_link).unwrap(),
            "=sum(D4)\n",
            "an explicitly named directory symlink must expand"
        );
    }
}

/// An unreadable subtree is a per-file-style error, not a fatal one: it is
/// reported (exit 1) while the rest of the tree and other explicit inputs
/// still format.
#[cfg(unix)]
#[test]
fn an_unreadable_subtree_does_not_block_other_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join("gsfmt-cli-unreadable-test");
    let locked = dir.join("locked");
    // restore perms from a previous run so the cleanup can see inside
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("in.gsfx"), "=sum( A1 )").unwrap();
    let sibling = dir.join("sibling.gsfx");
    std::fs::write(&sibling, "=sum( A1,B2 )").unwrap();
    let explicit = std::env::temp_dir().join("gsfmt-cli-unreadable-explicit.gsfx");
    std::fs::write(&explicit, "=sum( C3 )").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let out = run(
        &["--write", dir.to_str().unwrap(), explicit.to_str().unwrap()],
        &[],
        "",
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.code, 1, "stderr: {}", out.stderr);
    assert!(out.stderr.contains("locked"), "{}", out.stderr);
    assert_eq!(
        std::fs::read_to_string(&sibling).unwrap(),
        "=sum(A1, B2)\n",
        "the readable part of the tree must still format"
    );
    assert_eq!(
        std::fs::read_to_string(&explicit).unwrap(),
        "=sum(C3)\n",
        "an explicit file after a failing directory must still format"
    );
}

/// A discovered filename that is not valid UTF-8 must format in place, not
/// be skipped or written to a `�`-mangled sibling. Linux only: macOS
/// filesystems reject non-UTF-8 names outright.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_filenames_format_in_place() {
    use std::os::unix::ffi::OsStrExt;
    let dir = std::env::temp_dir().join("gsfmt-cli-non-utf8-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let name = std::ffi::OsStr::from_bytes(b"bad-\xff.gsfx");
    let file = dir.join(name);
    std::fs::write(&file, "=sum( A1,B2 )").unwrap();

    let out = run(&["--write", dir.to_str().unwrap()], &[], "");
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "=sum(A1, B2)\n");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "no mangled sibling may appear next to the original"
    );
}

/// A non-UTF-8 FILE argument must land on the ordinary per-file error path,
/// not panic inside argv decoding (`env::args` aborts with exit 101 and a
/// backtrace pointer before `main` sees the argument). The file need not
/// exist — the crash was in decoding, before any I/O.
#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_an_io_error_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let out = Command::new(BIN)
        .arg(std::ffi::OsStr::from_bytes(b"no-such-\xff\xfe.gsfx"))
        .current_dir(std::env::temp_dir())
        .env("XDG_CONFIG_HOME", "/nonexistent-gsfmt-test")
        .stdin(Stdio::null())
        .output()
        .expect("spawn gsfmt");
    assert_eq!(out.status.code(), Some(1), "want a plain IO error, not 101");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("gsfmt: "), "stderr: {stderr}");
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
    assert!(out.stdout.is_empty());
}

/// The in-place replacement must not disturb neighbours or metadata: a
/// pre-existing sidecar file survives (the temp name is created
/// exclusively, never a fixed predictable path), and the original's
/// permission bits carry over instead of the process umask.
#[test]
fn write_mode_preserves_neighbours_and_permissions() {
    let dir = std::env::temp_dir().join("gsfmt-cli-write-meta-test");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("meta.gsfx");
    let sidecar = dir.join("meta.gsfx.gsfmt~");
    std::fs::write(&f, "=sum( A1,B2 )").unwrap();
    std::fs::write(&sidecar, "precious bytes").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let out = run(&["--write", f.to_str().unwrap()], &[], "");
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "=sum(A1, B2)\n");
    assert_eq!(
        std::fs::read_to_string(&sidecar).unwrap(),
        "precious bytes",
        "sidecar file must survive an in-place write"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "permissions must survive an in-place write");
    }
}
