//! `diarist-cli crash --panic|--segv|--engine` against a scratch data root:
//! each leaves its report in `<root>\logs`, and "Copy report" text has no
//! profile folder (docs/plans/11-hardening.md 8a).

use std::path::Path;
use std::process::{Command, Output};

use diarist_core::crash::{self, CrashKind};

fn cli(root: &Path, what: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_diarist-cli"))
        .args(["--data-dir", &root.display().to_string(), "crash", what])
        .env_remove("RUST_BACKTRACE")
        .output()
        .expect("run diarist-cli")
}

fn assert_scrubbed(logs: &Path, name: &str) -> String {
    let text = crash::read_report(logs, name).unwrap();
    if let Some(profile) = diarist_core::diagnostics::profile_dir() {
        assert!(
            !text
                .to_ascii_lowercase()
                .contains(&profile.to_ascii_lowercase()),
            "profile folder left in:\n{text}"
        );
    }
    text
}

#[test]
fn panic_leaves_a_text_report() {
    let root = tempfile::tempdir().unwrap();
    let out = cli(root.path(), "--panic");
    assert_eq!(out.status.code(), Some(101), "{out:?}");
    let logs = root.path().join("logs");
    let reports = crash::list_reports(&logs);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let r = &reports[0];
    assert_eq!((r.kind, r.dump.is_some()), (CrashKind::Panic, false));
    let text = std::fs::read_to_string(logs.join(&r.name)).unwrap();
    for want in [
        "Diarist crash report: panic\n",
        "app: diarist-cli ",
        "message: deliberate panic from `diarist-cli crash --panic`",
        "location: crates",
        "backtrace:\n",
        "app log: none",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }
    assert_eq!(crash::pending_notice(&logs).as_ref(), Some(r));
    assert_scrubbed(&logs, &r.name);
}

#[cfg(windows)]
#[test]
fn access_violation_leaves_a_report_and_a_minidump() {
    let root = tempfile::tempdir().unwrap();
    let out = cli(root.path(), "--segv");
    assert_eq!(out.status.code(), Some(0xC000_0005_u32 as i32), "{out:?}");
    let logs = root.path().join("logs");
    let reports = crash::list_reports(&logs);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let r = &reports[0];
    assert_eq!(r.kind, CrashKind::Native);
    let dump = logs.join(r.dump.as_deref().expect("a minidump"));
    let bytes = std::fs::read(&dump).unwrap();
    assert!(
        bytes.starts_with(b"MDMP") && bytes.len() > 4_096,
        "{}",
        bytes.len()
    );
    let text = assert_scrubbed(&logs, &r.name);
    for want in [
        "Diarist crash report: native exception\n",
        "exception: 0xC0000005 (access violation, writing address 0x8)",
        "in diarist-cli.exe + 0x",
        "minidump: crash-",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }
    assert!(!text.contains("minidump not written"), "{text}");
}

#[cfg(windows)]
#[test]
fn engine_crash_leaves_an_engine_report_and_no_notice() {
    let root = tempfile::tempdir().unwrap();
    let out = cli(root.path(), "--engine");
    assert!(out.status.success(), "{out:?}");
    let logs = root.path().join("logs");
    let reports = crash::list_reports(&logs);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let r = &reports[0];
    assert_eq!(r.kind, CrashKind::Engine);
    let text = assert_scrubbed(&logs, &r.name);
    for want in [
        "Diarist engine exit report\n",
        "engine: nemo-speech transcribe\n",
        "exit: exit code: 0xc0000005 (access violation)\n",
        "  [nemo-speech] simulated fault while decoding\n",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }
    // The app did not close: Diagnostics lists it, the launch notice does not.
    assert_eq!(crash::pending_notice(&logs), None);
}
