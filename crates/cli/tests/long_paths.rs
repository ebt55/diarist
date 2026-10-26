//! `diarist-cli` with a database or data folder longer than `MAX_PATH`:
//! a long `--db`, and a short relative `--data-dir` that resolves long
//! from the working directory. Both open, write and list.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A folder under `base` whose absolute path is `len` characters long.
fn long_dir(base: &Path, len: usize) -> PathBuf {
    let mut dir = std::path::absolute(base).unwrap();
    let mut i = 0;
    while dir.as_os_str().len() < len {
        let left = len - dir.as_os_str().len() - 1;
        let take = if left <= 40 { left } else { 40.min(left - 2) };
        dir.push(
            format!("d{i}{}", "x".repeat(60))
                .chars()
                .take(take.max(1))
                .collect::<String>(),
        );
        i += 1;
    }
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(dir.as_os_str().len(), len, "{}", dir.display());
    dir
}

fn cli(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_diarist-cli"))
        .current_dir(cwd)
        .args(args)
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("run diarist-cli")
}

fn assert_ok(out: &Output) -> String {
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_long_db_opens_and_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    let db = long_dir(&tmp.path().join("db"), 264).join("diarist.db");
    let (root_s, db_s) = (root.display().to_string(), db.display().to_string());
    let args = ["--data-dir", &root_s, "--db", &db_s];
    assert_ok(&cli(tmp.path(), &[&args[..], &["list"]].concat()));
    assert!(db.is_file());
    // A second open finds the same database.
    assert_ok(&cli(tmp.path(), &[&args[..], &["list"]].concat()));
}

#[test]
fn a_short_relative_data_dir_that_resolves_long_opens() {
    let tmp = tempfile::tempdir().unwrap();
    // The working directory stays below MAX_PATH (CreateProcess needs
    // that); the data folder under it does not.
    let cwd = long_dir(tmp.path(), 200);
    let rel = "r".repeat(70);
    assert!(rel.len() < 240 && cwd.join(&rel).as_os_str().len() > 260);
    assert_ok(&cli(&cwd, &["--data-dir", &rel, "list"]));
    assert!(cwd.join(&rel).join("diarist.db").is_file());
}
