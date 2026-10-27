//! One-time move of a pre-rename data root into Diarist's.
//!
//! The project was called meet-assist until 15 Oct 2026 (now Diarist). Its
//! data root was `%LOCALAPPDATA%\app.meetassist.desktop`, or the folder named
//! by `HKCU\Software\meet-assist\DataRoot` after a move, with the database
//! `meet-assist.db`. At the first launch under the new name, when the new
//! data root has no `diarist.db` yet, the app asks "Move your existing
//! meet-assist data to Diarist?" and either moves that data in or starts
//! fresh. Moving is the data-folder move ([`super::relocate`]): copy and
//! verify into the new root, restart, then delete the old copy at the next
//! launch, and with it the old registry value ([`forget_legacy_registry`]).
//! Starting fresh leaves the old folder alone; the new database then exists,
//! so the question is not asked again.
//!
//! `DIARIST_LEGACY_ROOT` names the old root instead of the registry value and
//! the default folder (tests and development, with a scratch copy).

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::paths::{read_registry_path, write_registry_path, DataPaths, DataRootSource};
use super::relocate::{self, CleanupReport, MovePlan, DB_FILE, MOVED_DIRS, MOVE_MARKER};
use crate::error::{CoreError, Result};

/// The pre-rename bundle identifier: the old default data root's folder name.
pub const LEGACY_IDENTIFIER: &str = "app.meetassist.desktop";
/// The pre-rename registry key (under HKCU) naming a moved data root.
pub const LEGACY_REGISTRY_KEY: &str = r"Software\meet-assist";
/// The pre-rename database file name.
pub const LEGACY_DB_FILE: &str = "meet-assist.db";
/// Overrides where the old data root is looked for (tests, development).
pub const LEGACY_ROOT_ENV: &str = "DIARIST_LEGACY_ROOT";

/// A pre-rename data root that holds a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRoot {
    pub root: PathBuf,
    /// Named by the old registry value (the old app had moved its data).
    pub from_registry: bool,
}

fn err(msg: impl Into<String>) -> CoreError {
    CoreError::Models(msg.into())
}

fn has_db(root: &Path) -> bool {
    root.join(LEGACY_DB_FILE).is_file()
}

fn legacy_override() -> Option<PathBuf> {
    std::env::var_os(LEGACY_ROOT_ENV)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// The old data root, if one with a database exists: `DIARIST_LEGACY_ROOT`
/// when set, else the old registry value, else the old default folder.
pub fn find_legacy_root() -> Option<LegacyRoot> {
    if let Some(root) = legacy_override() {
        return has_db(&root).then_some(LegacyRoot {
            root,
            from_registry: false,
        });
    }
    if let Some(root) = read_registry_path(LEGACY_REGISTRY_KEY) {
        if has_db(&root) {
            return Some(LegacyRoot {
                root,
                from_registry: true,
            });
        }
    }
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(LEGACY_IDENTIFIER);
    has_db(&root).then_some(LegacyRoot {
        root,
        from_registry: false,
    })
}

/// The old data root to offer at this launch, if any: only when the new root
/// has no database and no move in progress, and is not set by portable mode
/// or (without `DIARIST_LEGACY_ROOT`) by `DIARIST_DATA_DIR`.
pub fn pending_legacy(paths: &DataPaths) -> Option<LegacyRoot> {
    offer(paths, legacy_override().is_some(), find_legacy_root)
}

fn offer(
    paths: &DataPaths,
    overridden: bool,
    find: impl FnOnce() -> Option<LegacyRoot>,
) -> Option<LegacyRoot> {
    match paths.source {
        DataRootSource::Portable => return None,
        DataRootSource::Explicit if !overridden => return None,
        _ => {}
    }
    if paths.db().exists() || paths.root.join(MOVE_MARKER).exists() {
        return None;
    }
    find().filter(|l| !relocate::overlaps(&l.root, &paths.root))
}

/// Checks that the old root at `from` can move into the new root `to` and
/// measures the copy. Unlike [`relocate::plan_move`], `to` may already hold
/// what this launch created (logs, the WebView profile), but no database and
/// no non-empty audio, engine, models or manifests folder.
pub fn plan_legacy_move(from: &Path, to: &Path) -> Result<MovePlan> {
    relocate::check_paths(from, to)?;
    if to.join(DB_FILE).exists() || to.join(MOVE_MARKER).exists() {
        return Err(err(format!(
            "{} already holds Diarist data; nothing was moved",
            to.display()
        )));
    }
    for d in MOVED_DIRS {
        let p = to.join(d);
        if p.is_dir() && std::fs::read_dir(&p)?.next().is_some() {
            return Err(err(format!(
                "{} is not empty; nothing was moved",
                p.display()
            )));
        }
    }
    relocate::measure(from, LEGACY_DB_FILE, to)
}

/// Copies the old database at `src` to `dest` with the SQLite backup API
/// (the old app is not running, so nothing else has it open), upgrades the
/// copy to the current schema, and checks that it has as many meetings as
/// the original. `src` is only read.
pub fn copy_legacy_database(src: &Path, dest: &Path) -> Result<()> {
    // SQLite gets long paths in the extended-length form (`path_arg`).
    let sqlite = |p: &Path| PathBuf::from(crate::engine::process::path_arg(p));
    let sql = |e: rusqlite::Error| err(format!("{}: {e}", src.display()));
    let conn = Connection::open_with_flags(sqlite(src), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .or_else(|_| Connection::open_with_flags(sqlite(src), OpenFlags::SQLITE_OPEN_READ_WRITE))
        .map_err(sql)?;
    let want: i64 = conn
        .query_row("SELECT count(*) FROM meetings", [], |r| r.get(0))
        .map_err(sql)?;
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    conn.backup(rusqlite::MAIN_DB, sqlite(&tmp), None)
        .map_err(sql)?;
    drop(conn);
    std::fs::rename(&tmp, dest)?;
    // Runs the schema migrations on the copy.
    drop(crate::store::SqliteStore::open(dest)?);
    let copy = Connection::open_with_flags(sqlite(dest), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| err(format!("{}: {e}", dest.display())))?;
    let got: i64 = copy
        .query_row("SELECT count(*) FROM meetings", [], |r| r.get(0))
        .map_err(|e| err(format!("{}: {e}", dest.display())))?;
    if got != want {
        return Err(err(format!(
            "the database copy has {got} meetings, the original {want}"
        )));
    }
    Ok(())
}

/// After the cleanup of a moved-in old root: removes the old registry value
/// when it named that root and its database is gone.
pub fn forget_legacy_registry(report: &CleanupReport) -> Result<bool> {
    if !report.removed.iter().any(|r| r == LEGACY_DB_FILE) {
        return Ok(false);
    }
    let Some(named) = read_registry_path(LEGACY_REGISTRY_KEY) else {
        return Ok(false);
    };
    if !relocate::same_folder(&named, &report.from) {
        return Ok(false);
    }
    write_registry_path(LEGACY_REGISTRY_KEY, None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewMeeting, SqliteStore, Store};
    use crate::types::{MeetingStatus, Source};
    use std::fs;

    fn write(p: &Path, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// An old-style root: `meet-assist.db` with two meetings, audio, models.
    fn old_root(root: &Path) {
        fs::create_dir_all(root).unwrap();
        let store = SqliteStore::open(root.join(LEGACY_DB_FILE)).unwrap();
        for title in ["First", "Second"] {
            store
                .create_meeting(NewMeeting {
                    title: title.into(),
                    source: Source::File,
                    source_path: None,
                    language: "en".into(),
                    diarize_mic: false,
                    started_at: 1_700_000_000_000,
                    status: MeetingStatus::Done,
                })
                .unwrap();
        }
        drop(store);
        write(&root.join("audio/01ABC/playback.flac"), &[7u8; 4000]);
        write(&root.join("models/nvidia/x/y.gguf"), &[1u8; 3000]);
        write(&root.join("logs/meet-assist.log"), b"old log\n");
        write(&root.join("EBWebView/Default/prefs"), b"x");
    }

    /// What a first launch under the new name creates before the question.
    fn new_root(root: &Path) {
        write(&root.join("logs/diarist.log"), b"new log\n");
        write(&root.join("EBWebView/Default/prefs"), b"y");
    }

    #[test]
    fn offered_only_when_the_new_root_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        old_root(&old);
        let found = || {
            Some(LegacyRoot {
                root: old.clone(),
                from_registry: false,
            })
        };
        let new = DataPaths {
            source: DataRootSource::Default,
            ..DataPaths::at(dir.path().join("new"))
        };
        assert_eq!(offer(&new, false, found).unwrap().root, old);
        // A scratch DIARIST_DATA_DIR never offers the real old root...
        assert!(offer(&DataPaths::at(dir.path().join("new")), false, found).is_none());
        // ...unless DIARIST_LEGACY_ROOT names one.
        assert!(offer(&DataPaths::at(dir.path().join("new")), true, found).is_some());
        let portable = DataPaths {
            source: DataRootSource::Portable,
            ..new.clone()
        };
        assert!(offer(&portable, true, found).is_none());
        // Same folder, or the new root already has a database: nothing to do.
        let same = DataPaths {
            source: DataRootSource::Default,
            ..DataPaths::at(&old)
        };
        assert!(offer(&same, false, found).is_none());
        write(&new.db(), b"");
        assert!(offer(&new, false, found).is_none());
    }

    #[test]
    fn moves_an_old_root_in_and_cleans_it_up_after_the_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        old_root(&old);
        new_root(&new);

        let plan = plan_legacy_move(&old, &new).unwrap();
        assert_eq!(plan.db_file, LEGACY_DB_FILE);
        let report = relocate::copy_data(
            &plan,
            &|dest: &Path| copy_legacy_database(&old.join(LEGACY_DB_FILE), dest),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(report.files, plan.files);
        let copy = SqliteStore::open(new.join(DB_FILE)).unwrap();
        assert_eq!(copy.list_meetings().unwrap().len(), 2);
        drop(copy);
        assert_eq!(
            fs::read(new.join("audio/01ABC/playback.flac"))
                .unwrap()
                .len(),
            4000
        );
        // The new log stays; the old one is carried over next to it.
        assert_eq!(
            fs::read(new.join("logs/diarist.log")).unwrap(),
            b"new log\n"
        );
        assert!(new.join("logs/meet-assist.log").is_file());
        // The old root is untouched until the next launch.
        assert!(old.join(LEGACY_DB_FILE).is_file());

        let r = relocate::finish_pending_move(&new).unwrap();
        assert!(r.kept.is_empty(), "{r:?}");
        assert!(r.removed.iter().any(|x| x == LEGACY_DB_FILE), "{r:?}");
        assert!(r.removed_root, "{r:?}");
        assert!(!old.exists());
        assert!(new.join(DB_FILE).is_file());
    }

    #[test]
    fn refuses_a_new_root_that_already_has_data_and_keeps_its_logs_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        old_root(&old);
        new_root(&new);
        write(&new.join("models/z.gguf"), b"z");
        assert!(plan_legacy_move(&old, &new).is_err());
        fs::remove_dir_all(new.join("models")).unwrap();

        let plan = plan_legacy_move(&old, &new).unwrap();
        let failing = |_: &Path| -> Result<()> { Err(err("backup refused")) };
        assert!(relocate::copy_data(&plan, &failing, &mut |_, _| {}).is_err());
        assert!(!new.join(DB_FILE).exists());
        assert_eq!(
            fs::read(new.join("logs/diarist.log")).unwrap(),
            b"new log\n"
        );
        assert!(old.join(LEGACY_DB_FILE).is_file());
        assert!(old.join("audio/01ABC/playback.flac").is_file());
    }

    /// Both roots longer than `MAX_PATH`: the old database is read and
    /// copied, and the copy is migrated and checked, there.
    #[cfg(windows)]
    #[test]
    fn copies_the_old_database_between_folders_longer_than_max_path() {
        use crate::engine::process::long_test_dir;
        let dir = tempfile::tempdir().unwrap();
        let old = long_test_dir(&dir.path().join("old"), 264);
        let new = long_test_dir(&dir.path().join("new"), 270);
        old_root(&old);
        copy_legacy_database(&old.join(LEGACY_DB_FILE), &new.join(DB_FILE)).unwrap();
        let copy = SqliteStore::open(new.join(DB_FILE)).unwrap();
        assert_eq!(copy.list_meetings().unwrap().len(), 2);
    }
}
