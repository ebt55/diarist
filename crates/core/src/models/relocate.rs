//! Moving the data root (Settings, "Move data folder"; `docs/01`, "Install
//! layout").
//!
//! 1. [`plan_move`] checks the destination (an empty or new folder, not
//!    inside the current root or around it, with room for everything) and
//!    measures what will be copied.
//! 2. [`copy_data`] copies the database through a caller-supplied backup
//!    (the SQLite backup API: the database is open), then `audio\`,
//!    `engine\`, `models\` and `manifests\` file by file, checking every
//!    file's size after the copy. `logs\` and the window state are copied as
//!    they are (the log grows while copying). Links (junctions) are recreated as
//!    links, never followed. A [`MOVE_MARKER`] in the new root records the
//!    old root and what was copied.
//! 3. The caller points `HKCU\Software\Diarist\DataRoot` at the new root
//!    ([`super::paths::set_registry_root`]) and restarts the app.
//! 4. At the next launch, from the new root, [`finish_pending_move`] deletes
//!    exactly the copied items from the old root (plus the WebView profile
//!    and window state, which are recreated), and the old root itself when
//!    that leaves it empty. Nothing else in the old folder is touched.
//!
//! The old copy is deleted after the restart, not before, because the
//! running app still holds the old database and log files open.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paths::free_disk_bytes;
use crate::error::{CoreError, Result};

/// Marker in the new root naming the old one (deleted after the cleanup).
pub const MOVE_MARKER: &str = "moved-from.json";
/// The database file name under the data root.
pub const DB_FILE: &str = "diarist.db";
/// Folders copied to the new root and verified, in order.
pub const MOVED_DIRS: &[&str] = &["audio", "engine", "models", "manifests"];
/// Copied as they are (they change while the app runs) and removed from
/// the old root after the move.
pub const BEST_EFFORT: &[&str] = &["logs", ".window-state.json"];
/// Removed from the old root after the move but not copied: the WebView
/// profile is recreated in the new root.
const RECREATED: &[&str] = &["EBWebView"];
/// Head room kept free on the destination volume.
const SPACE_MARGIN: u64 = 64 << 20;

fn err(msg: impl Into<String>) -> CoreError {
    CoreError::Models(msg.into())
}

/// What [`copy_data`] will copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePlan {
    pub from: PathBuf,
    pub to: PathBuf,
    /// The database file name under `from`: [`DB_FILE`], or the old name
    /// when a pre-rename data root is moved in ([`super::legacy`]). It is
    /// always written to `to` as [`DB_FILE`].
    pub db_file: String,
    /// Files (the database counts as one).
    pub files: u64,
    pub bytes: u64,
    /// Free bytes on the destination volume, when known.
    pub free_bytes: Option<u64>,
}

/// What [`copy_data`] copied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveReport {
    pub files: u64,
    pub bytes: u64,
    /// Links recreated (junctions in development setups).
    pub links: u64,
}

/// What [`finish_pending_move`] did with the old root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReport {
    pub from: PathBuf,
    /// Items deleted from the old root.
    pub removed: Vec<String>,
    /// Items left in place, with the reason.
    pub kept: Vec<String>,
    /// The old root folder itself was removed (it was empty afterwards).
    pub removed_root: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    from: PathBuf,
    /// Copied items: the database file and the folders that existed.
    items: Vec<String>,
    /// File count and bytes per copied folder, as copied.
    #[serde(default)]
    stats: Vec<(String, u64, u64)>,
    at_ms: i64,
    /// The database's name in the old root when it is not [`DB_FILE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    db_file: Option<String>,
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Lexically normalised absolute path (no `.`/`..`), for containment checks.
fn normalised(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn lower(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// `inner` is `outer` or below it (case-insensitive, like Windows paths).
fn is_within(inner: &Path, outer: &Path) -> bool {
    let (i, o) = (lower(&normalised(inner)), lower(&normalised(outer)));
    i == o || i.starts_with(&format!("{o}\\"))
}

/// `a` and `b` are the same folder or one holds the other.
pub(super) fn overlaps(a: &Path, b: &Path) -> bool {
    is_within(a, b) || is_within(b, a)
}

/// `a` and `b` name the same folder (case-insensitive).
pub(super) fn same_folder(a: &Path, b: &Path) -> bool {
    is_within(a, b) && is_within(b, a)
}

/// File count and bytes under `dir`, links counted as one file of size 0
/// and never followed. A missing folder is (0, 0).
pub fn tree_stats(dir: &Path) -> (u64, u64) {
    let mut files = 0;
    let mut bytes = 0;
    let Ok(rd) = fs::read_dir(dir) else {
        return (0, 0);
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            files += 1;
        } else if ft.is_dir() {
            let (f, b) = tree_stats(&e.path());
            files += f;
            bytes += b;
        } else {
            files += 1;
            bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (files, bytes)
}

/// [`tree_stats`], or (1, 0) when `p` itself is a link.
fn item_stats(p: &Path) -> (u64, u64) {
    if is_link(p) {
        (1, 0)
    } else {
        tree_stats(p)
    }
}

/// Checks that `to` can take the data root at `from` and measures the copy.
pub fn plan_move(from: &Path, to: &Path) -> Result<MovePlan> {
    check_paths(from, to)?;
    if to.exists() && fs::read_dir(to)?.next().is_some() {
        return Err(err(format!(
            "{} is not empty; choose an empty or new folder",
            to.display()
        )));
    }
    measure(from, DB_FILE, to)
}

/// `to` is absolute, a folder (or not there yet), and neither inside `from`
/// nor around it.
pub(super) fn check_paths(from: &Path, to: &Path) -> Result<()> {
    if !to.is_absolute() {
        return Err(err(format!("{} is not an absolute path", to.display())));
    }
    if is_within(to, from) {
        return Err(err(format!(
            "{} is inside the current data folder {}; choose a folder elsewhere",
            to.display(),
            from.display()
        )));
    }
    if is_within(from, to) {
        return Err(err(format!(
            "{} contains the current data folder; choose an empty folder elsewhere",
            to.display()
        )));
    }
    if to.exists() && !to.is_dir() {
        return Err(err(format!("{} is a file, not a folder", to.display())));
    }
    Ok(())
}

/// Measures what [`copy_data`] will copy from `from` (whose database is
/// `db_file`) and checks the free space at `to`.
pub(super) fn measure(from: &Path, db_file: &str, to: &Path) -> Result<MovePlan> {
    let mut files = 1;
    let mut bytes = fs::metadata(from.join(db_file))
        .map(|m| m.len())
        .unwrap_or(0);
    for d in MOVED_DIRS {
        let (f, b) = item_stats(&from.join(d));
        files += f;
        bytes += b;
    }
    let free_bytes = free_disk_bytes(to);
    if let Some(free) = free_bytes {
        if free < bytes + SPACE_MARGIN {
            return Err(CoreError::InsufficientSpace {
                path: to.display().to_string(),
                free_mb: free >> 20,
                needed_mb: (bytes + SPACE_MARGIN) >> 20,
            });
        }
    }
    Ok(MovePlan {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        db_file: db_file.to_owned(),
        files,
        bytes,
        free_bytes,
    })
}

/// The target of a link, without the `\\?\` / `\??\` prefix.
fn link_target(p: &Path) -> Result<PathBuf> {
    let t = fs::read_link(p)?;
    let s = t.to_string_lossy();
    let s = s
        .strip_prefix(r"\\?\")
        .or_else(|| s.strip_prefix(r"\??\"))
        .unwrap_or(&s);
    Ok(PathBuf::from(s))
}

/// Recreates the link `src` at `dest` (a junction for folders on Windows,
/// which needs no privileges).
fn copy_link(src: &Path, dest: &Path) -> Result<()> {
    let target = link_target(src)?;
    let is_dir = fs::metadata(src).map(|m| m.is_dir()).unwrap_or(true);
    #[cfg(windows)]
    {
        if is_dir {
            let out = std::process::Command::new("cmd")
                .arg("/c")
                .arg("mklink")
                .arg("/J")
                .arg(dest)
                .arg(&target)
                .output()?;
            if !out.status.success() {
                return Err(err(format!(
                    "cannot recreate the link {} -> {}: {}",
                    dest.display(),
                    target.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        } else {
            std::os::windows::fs::symlink_file(&target, dest).map_err(|e| {
                err(format!(
                    "cannot recreate the link {} -> {}: {e}",
                    dest.display(),
                    target.display()
                ))
            })?;
        }
    }
    #[cfg(unix)]
    {
        let _ = is_dir;
        std::os::unix::fs::symlink(&target, dest)?;
    }
    Ok(())
}

/// Copies `src` into `dest` (created), checking each file's size; links
/// are recreated. Adds to `report` and reports bytes through `progress`.
fn copy_tree(
    src: &Path,
    dest: &Path,
    report: &mut MoveReport,
    progress: &mut dyn FnMut(u64),
) -> Result<()> {
    fs::create_dir_all(dest)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let ft = e.file_type()?;
        let (s, d) = (e.path(), dest.join(e.file_name()));
        if ft.is_symlink() {
            copy_link(&s, &d)?;
            report.links += 1;
            report.files += 1;
        } else if ft.is_dir() {
            copy_tree(&s, &d, report, progress)?;
        } else {
            let want = e.metadata()?.len();
            let got = fs::copy(&s, &d)
                .map_err(|e| err(format!("copy {} to {}: {e}", s.display(), d.display())))?;
            let on_disk = fs::metadata(&d)?.len();
            if got != want || on_disk != want {
                return Err(err(format!(
                    "{}: copied {on_disk} bytes, expected {want}",
                    d.display()
                )));
            }
            report.files += 1;
            report.bytes += want;
            progress(want);
        }
    }
    Ok(())
}

/// Copies the data root to `plan.to` (see the module docs). `backup_db`
/// writes a consistent copy of the open database to the given path and
/// verifies it. `progress` receives (bytes copied, bytes in the plan).
/// On error the partial copy is removed and the old root is untouched.
pub fn copy_data(
    plan: &MovePlan,
    backup_db: &dyn Fn(&Path) -> Result<()>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<MoveReport> {
    let created = !plan.to.exists();
    // A pre-rename root moves into a root this launch already uses (logs,
    // WebView profile): what was there before stays if the copy fails.
    let existing: Vec<&str> = BEST_EFFORT
        .iter()
        .copied()
        .filter(|n| plan.to.join(n).exists())
        .collect();
    let result = (|| -> Result<MoveReport> {
        fs::create_dir_all(&plan.to)?;
        let mut report = MoveReport::default();
        let mut items = Vec::new();
        let mut stats = Vec::new();
        let total = plan.bytes;
        let mut done = 0u64;
        progress(0, total);
        let db = plan.from.join(&plan.db_file);
        if db.is_file() {
            backup_db(&plan.to.join(DB_FILE))?;
            let n = fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
            done += n;
            report.files += 1;
            report.bytes += n;
            items.push(plan.db_file.clone());
            progress(done, total);
        }
        for d in MOVED_DIRS {
            let src = plan.from.join(d);
            if !src.exists() && !is_link(&src) {
                continue;
            }
            let dest = plan.to.join(d);
            let before = report.clone();
            if is_link(&src) {
                copy_link(&src, &dest)?;
                report.links += 1;
                report.files += 1;
            } else {
                copy_tree(&src, &dest, &mut report, &mut |n| {
                    done += n;
                    progress(done, total);
                })?;
            }
            // Verify the folder as a whole against the source.
            let (sf, sb) = item_stats(&src);
            let (df, db) = item_stats(&dest);
            if df < sf || db < sb {
                return Err(err(format!(
                    "{d}: the copy has {df} files / {db} bytes, the original {sf} / {sb}"
                )));
            }
            stats.push((
                (*d).to_owned(),
                report.files - before.files,
                report.bytes - before.bytes,
            ));
            items.push((*d).to_owned());
        }
        for name in BEST_EFFORT {
            let src = plan.from.join(name);
            let dest = plan.to.join(name);
            if src.is_dir() {
                let mut ignored = MoveReport::default();
                let _ = copy_tree(&src, &dest, &mut ignored, &mut |_| {});
            } else if src.is_file() {
                let _ = fs::copy(&src, &dest);
            }
        }
        let marker = Marker {
            from: plan.from.clone(),
            items,
            stats,
            at_ms: super::now_ms(),
            db_file: (plan.db_file != DB_FILE).then(|| plan.db_file.clone()),
        };
        fs::write(
            plan.to.join(MOVE_MARKER),
            serde_json::to_vec_pretty(&marker).map_err(|e| err(e.to_string()))?,
        )?;
        Ok(report)
    })();
    if result.is_err() {
        remove_copy(&plan.to, created, &existing);
    }
    result
}

/// Removes what [`copy_data`] wrote into `to` (and `to` when it created it),
/// except the `keep` items that were there before.
fn remove_copy(to: &Path, created: bool, keep: &[&str]) {
    for name in [DB_FILE, "diarist.db.tmp", MOVE_MARKER] {
        let _ = fs::remove_file(to.join(name));
    }
    for d in MOVED_DIRS.iter().chain(BEST_EFFORT) {
        if !keep.contains(d) {
            let _ = remove_item(&to.join(d));
        }
    }
    if created {
        let _ = fs::remove_dir(to);
    }
}

/// Runs `op` again while a file is still held open (sharing violation or
/// access denied), until `deadline`: after a move the previous instance may
/// still be exiting when the new one cleans up.
fn retry_while_held(
    deadline: &std::time::Instant,
    mut op: impl FnMut() -> std::io::Result<()>,
) -> std::io::Result<()> {
    loop {
        match op() {
            Err(e)
                if matches!(e.raw_os_error(), Some(5 | 32))
                    && std::time::Instant::now() < *deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            other => return other,
        }
    }
}

/// Deletes a file, a folder tree or a link (the link only, never its target).
fn remove_item(p: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(p)?;
    if meta.file_type().is_symlink() {
        // A junction or directory symlink is removed like an empty folder.
        fs::remove_dir(p).or_else(|_| fs::remove_file(p))
    } else if meta.is_dir() {
        // `remove_dir_all` does not follow links inside the tree.
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

/// Whether `root` holds a [`MOVE_MARKER`] (a move waits for its cleanup).
pub fn pending_move(root: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(root.join(MOVE_MARKER)).ok()?;
    serde_json::from_str::<Marker>(&text).ok().map(|m| m.from)
}

/// After a move, at launch from the new `root`: deletes the copied items
/// from the old root (see the module docs). `None` when no move is pending.
/// A folder whose copy is smaller than the original is kept and reported.
pub fn finish_pending_move(root: &Path) -> Option<CleanupReport> {
    let marker_path = root.join(MOVE_MARKER);
    let text = fs::read_to_string(&marker_path).ok()?;
    let marker: Marker = match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            return Some(CleanupReport {
                kept: vec![format!(
                    "{MOVE_MARKER} is unreadable ({e}); nothing deleted"
                )],
                ..Default::default()
            })
        }
    };
    let old = marker.from.clone();
    // Files the exiting previous instance holds are retried for a few seconds.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut report = CleanupReport {
        from: old.clone(),
        ..Default::default()
    };
    if is_within(root, &old) || is_within(&old, root) {
        report
            .kept
            .push("the old folder overlaps the new one; nothing deleted".into());
        let _ = fs::remove_file(&marker_path);
        return Some(report);
    }
    if !old.exists() {
        let _ = fs::remove_file(&marker_path);
        return Some(report);
    }
    let old_db = marker.db_file.as_deref().unwrap_or(DB_FILE);
    for item in &marker.items {
        let (src, dest) = (old.join(item), root.join(item));
        if !src.exists() && !is_link(&src) {
            continue;
        }
        if item == old_db {
            if !root.join(DB_FILE).is_file() {
                report
                    .kept
                    .push(format!("{item}: missing from the new folder"));
                continue;
            }
            for name in [item.clone(), format!("{item}-wal"), format!("{item}-shm")] {
                let p = old.join(&name);
                if p.exists() {
                    match retry_while_held(&deadline, || fs::remove_file(&p)) {
                        Ok(()) => report.removed.push(name),
                        Err(e) => report.kept.push(format!("{name}: {e}")),
                    }
                }
            }
            continue;
        }
        if !is_link(&src) {
            let (of, ob) = tree_stats(&src);
            let (nf, nb) = if is_link(&dest) {
                (u64::MAX, u64::MAX)
            } else {
                tree_stats(&dest)
            };
            if nf < of || nb < ob {
                report.kept.push(format!(
                    "{item}: the new copy has {nf} files / {nb} bytes, the old {of} / {ob}"
                ));
                continue;
            }
        }
        match retry_while_held(&deadline, || remove_item(&src)) {
            Ok(()) => report.removed.push(item.clone()),
            Err(e) => report.kept.push(format!("{item}: {e}")),
        }
    }
    for name in BEST_EFFORT.iter().chain(RECREATED) {
        let p = old.join(name);
        if p.exists() && retry_while_held(&deadline, || remove_item(&p)).is_ok() {
            report.removed.push((*name).to_owned());
        }
    }
    report.removed_root = fs::remove_dir(&old).is_ok();
    let _ = fs::remove_file(&marker_path);
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn fake_root(root: &Path) {
        write(&root.join(DB_FILE), b"sqlite db bytes");
        write(&root.join(format!("{DB_FILE}-wal")), b"wal");
        write(&root.join("audio/01ABC/playback.flac"), &[7u8; 5000]);
        write(&root.join("audio/01ABC/words.json"), b"{}");
        write(&root.join("models/nvidia/x/y.gguf"), &[1u8; 3000]);
        write(
            &root.join("engine/nemo-speech-0.2.0/bin/nemo-speech.exe"),
            b"MZ",
        );
        write(&root.join("logs/diarist.log"), b"line\n");
        write(&root.join("EBWebView/Default/prefs"), b"x");
        write(&root.join("unrelated.txt"), b"keep me");
    }

    fn backup(from: PathBuf) -> impl Fn(&Path) -> Result<()> {
        move |dest: &Path| {
            fs::copy(from.join(DB_FILE), dest)?;
            Ok(())
        }
    }

    #[test]
    fn moves_copies_verifies_and_cleans_up_only_what_it_copied() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new place"));
        fake_root(&old);

        let plan = plan_move(&old, &new).unwrap();
        assert_eq!(plan.files, 5, "{plan:?}"); // db + flac + json + gguf + exe (logs: best effort)
        assert_eq!(plan.bytes, 15 + 5000 + 2 + 3000 + 2);
        let mut last = (0, 0);
        let report = copy_data(&plan, &backup(old.clone()), &mut |d, t| last = (d, t)).unwrap();
        assert_eq!((report.files, report.bytes), (plan.files, plan.bytes));
        assert_eq!(last, (plan.bytes, plan.bytes));
        assert_eq!(
            fs::read(new.join("audio/01ABC/playback.flac")).unwrap(),
            vec![7u8; 5000]
        );
        assert!(new.join(DB_FILE).is_file());
        assert!(!new.join("EBWebView").exists() && !new.join("unrelated.txt").exists());
        assert_eq!(pending_move(&new), Some(old.clone()));

        // Next launch from the new root.
        let r = finish_pending_move(&new).unwrap();
        assert!(r.kept.is_empty(), "{r:?}");
        for item in [
            DB_FILE,
            "diarist.db-wal",
            "audio",
            "models",
            "engine",
            "logs",
            "EBWebView",
        ] {
            assert!(r.removed.iter().any(|x| x == item), "{item}: {r:?}");
            assert!(!old.join(item).exists(), "{item}");
        }
        // Something we did not copy stays, so the old folder stays too.
        assert_eq!(fs::read(old.join("unrelated.txt")).unwrap(), b"keep me");
        assert!(!r.removed_root);
        assert!(!new.join(MOVE_MARKER).exists());
        assert!(finish_pending_move(&new).is_none());
        // The new copy is intact.
        assert_eq!(fs::read(new.join("logs/diarist.log")).unwrap(), b"line\n");
    }

    #[test]
    fn an_old_root_with_only_our_items_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        fake_root(&old);
        fs::remove_file(old.join("unrelated.txt")).unwrap();
        let plan = plan_move(&old, &new).unwrap();
        copy_data(&plan, &backup(old.clone()), &mut |_, _| {}).unwrap();
        let r = finish_pending_move(&new).unwrap();
        assert!(r.removed_root, "{r:?}");
        assert!(!old.exists());
    }

    #[test]
    fn a_shrunken_copy_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        fake_root(&old);
        let plan = plan_move(&old, &new).unwrap();
        copy_data(&plan, &backup(old.clone()), &mut |_, _| {}).unwrap();
        fs::remove_file(new.join("audio/01ABC/playback.flac")).unwrap();
        let r = finish_pending_move(&new).unwrap();
        assert!(r.kept.iter().any(|k| k.starts_with("audio")), "{r:?}");
        assert!(old.join("audio/01ABC/playback.flac").is_file());
        assert!(!old.join("models").exists());
    }

    #[test]
    fn rejects_bad_destinations_and_cleans_a_failed_copy() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        fake_root(&old);
        assert!(plan_move(&old, &old).is_err());
        assert!(plan_move(&old, &old.join("audio").join("x")).is_err());
        assert!(
            plan_move(&old, dir.path()).is_err(),
            "contains the old root"
        );
        assert!(plan_move(&old, Path::new("relative")).is_err());
        let full = dir.path().join("full");
        write(&full.join("something"), b"1");
        assert!(plan_move(&old, &full).is_err());

        let new = dir.path().join("new");
        let plan = plan_move(&old, &new).unwrap();
        let failing = |_: &Path| -> Result<()> { Err(err("backup refused")) };
        assert!(copy_data(&plan, &failing, &mut |_, _| {}).is_err());
        assert!(!new.exists(), "the partial copy is removed");
        assert!(old.join(DB_FILE).is_file());
    }

    #[test]
    fn links_are_recreated_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        let shared = dir.path().join("shared-models");
        write(&shared.join("big.gguf"), &[3u8; 4000]);
        write(&old.join(DB_FILE), b"db");
        fs::create_dir_all(&old).unwrap();
        if copy_link_for_test(&shared, &old.join("models")).is_err() {
            return; // no way to create a link here
        }
        let plan = plan_move(&old, &new).unwrap();
        assert_eq!(plan.bytes, 2, "the link's target is not counted");
        let report = copy_data(&plan, &backup(old.clone()), &mut |_, _| {}).unwrap();
        assert_eq!(report.links, 1);
        assert!(is_link(&new.join("models")));
        assert_eq!(fs::read(new.join("models/big.gguf")).unwrap().len(), 4000);
        let r = finish_pending_move(&new).unwrap();
        assert!(r.removed.iter().any(|x| x == "models"), "{r:?}");
        // The link went, its target did not.
        assert!(!old.join("models").exists());
        assert!(shared.join("big.gguf").is_file());
        remove_item(&new.join("models")).unwrap();
        assert!(shared.join("big.gguf").is_file());
    }

    fn copy_link_for_test(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            let ok = std::process::Command::new("cmd")
                .arg("/c")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()?
                .status
                .success();
            if ok {
                Ok(())
            } else {
                Err(std::io::Error::other("mklink failed"))
            }
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }
}
