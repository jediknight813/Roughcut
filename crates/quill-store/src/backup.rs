//! Backups and recovery. A backup is a consistent `VACUUM INTO` copy of the
//! whole library (history included), verified with `integrity_check` before
//! it replaces anything. On open, a damaged or missing library is moved
//! aside (never deleted) and the newest healthy backup is restored.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::{Timestamp, Zoned};
use rusqlite::{Connection, OpenFlags};

use crate::{Library, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// The library was healthy.
    Clean,
    /// There was no library and no backup: a fresh one was created.
    New,
    /// The library was damaged (moved to `damaged`) or missing, and `from`
    /// was restored in its place.
    Restored {
        from: PathBuf,
        damaged: Option<PathBuf>,
    },
    /// The library was damaged and no healthy backup exists. It was moved to
    /// `damaged` and a fresh library created; nothing was deleted.
    Unrecoverable { damaged: PathBuf },
}

const PREFIX: &str = "library-";

impl Library {
    /// Writes a verified backup into `dir` and keeps only the newest `keep`.
    pub fn backup_to(&self, dir: &Path, keep: usize) -> Result<PathBuf> {
        fs::create_dir_all(dir)?;
        let now = Zoned::now();
        let name = format!(
            "{PREFIX}{}.{:03}.db",
            now.strftime("%Y-%m-%d_%H.%M.%S"),
            now.timestamp().as_millisecond().rem_euclid(1000)
        );
        let target = dir.join(&name);
        let partial = dir.join(format!("{name}.partial"));
        let _ = fs::remove_file(&partial);
        self.conn
            .execute("VACUUM INTO ?1", [partial.to_string_lossy()])?;
        if !is_healthy(&partial) {
            let _ = fs::remove_file(&partial);
            return Err(
                io::Error::new(io::ErrorKind::InvalidData, "backup failed verification").into(),
            );
        }
        fs::rename(&partial, &target)?;
        for old in backups_in(dir).into_iter().skip(keep) {
            let _ = fs::remove_file(old);
        }
        Ok(target)
    }

    /// Opens the library at `path`, restoring from the newest healthy backup
    /// in `backup_dirs` if it is damaged or missing.
    pub fn open_or_recover(path: &Path, backup_dirs: &[PathBuf]) -> Result<(Library, Recovery)> {
        let damaged = if !path.exists() {
            None
        } else if is_healthy(path) {
            return Ok((Library::open(path)?, Recovery::Clean));
        } else {
            Some(move_aside(path)?)
        };

        let mut candidates: Vec<PathBuf> = backup_dirs.iter().flat_map(|d| backups_in(d)).collect();
        candidates.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        if let Some(from) = candidates.into_iter().find(|b| is_healthy(b)) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, path)?;
            return Ok((Library::open(path)?, Recovery::Restored { from, damaged }));
        }

        let library = Library::open(path)?;
        Ok(match damaged {
            Some(damaged) => (library, Recovery::Unrecoverable { damaged }),
            None => (library, Recovery::New),
        })
    }
}

/// Opens read-only and runs SQLite's full integrity check.
fn is_healthy(path: &Path) -> bool {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return false;
    };
    let ok = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
        .is_ok_and(|s| s == "ok");
    ok && conn
        .query_row("SELECT count(*) FROM documents", [], |r| r.get::<_, i64>(0))
        .is_ok()
}

/// Renames the library and its WAL files out of the way, keeping them for
/// manual recovery. A stale WAL must never be applied to a restored copy.
fn move_aside(path: &Path) -> io::Result<PathBuf> {
    let suffix = format!("damaged-{}", Timestamp::now().as_millisecond());
    let with_suffix = |p: &Path| PathBuf::from(format!("{}.{suffix}", p.display()));
    let target = with_suffix(path);
    fs::rename(path, &target)?;
    for ext in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{ext}", path.display()));
        if side.exists() {
            fs::rename(&side, with_suffix(&side))?;
        }
    }
    Ok(target)
}

/// Finished backups in `dir`, newest first.
fn backups_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".db"))
        })
        .collect();
    found.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    found
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom, Write};

    use quill_doc::Document;

    use super::*;

    fn library_with_doc(path: &Path, text: &str) -> Library {
        let mut lib = Library::open(path).unwrap();
        lib.create(&Document::from_plain_text("Novel", text), None)
            .unwrap();
        lib
    }

    fn corrupt(path: &Path) {
        let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&[0u8; 100]).unwrap();
    }

    #[test]
    fn backups_are_pruned_to_keep_count() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library_with_doc(&dir.path().join("library.db"), "hello");
        let backups = dir.path().join("backups");
        for _ in 0..4 {
            lib.backup_to(&backups, 2).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let left = backups_in(&backups);
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|p| is_healthy(p)));
    }

    #[test]
    fn damaged_library_is_restored_from_newest_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let (near, far) = (dir.path().join("near"), dir.path().join("far"));
        {
            let lib = library_with_doc(&path, "older");
            lib.backup_to(&far, 10).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        {
            let mut lib = Library::open(&path).unwrap();
            lib.create(&Document::from_plain_text("Second", "newer"), None)
                .unwrap();
            lib.backup_to(&near, 10).unwrap();
        }
        corrupt(&path);

        let (lib, recovery) =
            Library::open_or_recover(&path, &[far.clone(), near.clone()]).unwrap();
        let Recovery::Restored {
            from,
            damaged: Some(damaged),
        } = recovery
        else {
            panic!("expected a restore, got {recovery:?}");
        };
        assert!(from.starts_with(&near), "newest backup wins across folders");
        assert!(damaged.exists(), "the damaged file is kept, not deleted");
        assert_eq!(lib.list(None).unwrap().len(), 2);
    }

    #[test]
    fn missing_library_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let backups = dir.path().join("b");
        library_with_doc(&path, "keep me")
            .backup_to(&backups, 5)
            .unwrap();
        fs::remove_file(&path).unwrap();
        let _ = fs::remove_file(dir.path().join("library.db-wal"));
        let _ = fs::remove_file(dir.path().join("library.db-shm"));

        let (lib, recovery) = Library::open_or_recover(&path, &[backups]).unwrap();
        assert!(matches!(recovery, Recovery::Restored { damaged: None, .. }));
        assert_eq!(lib.search("keep").unwrap().len(), 1);
    }

    #[test]
    fn healthy_library_opens_clean_and_damaged_without_backups_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        drop(library_with_doc(&path, "x"));
        let (_, recovery) = Library::open_or_recover(&path, &[]).unwrap();
        assert_eq!(recovery, Recovery::Clean);

        corrupt(&path);
        let (lib, recovery) = Library::open_or_recover(&path, &[dir.path().join("none")]).unwrap();
        let Recovery::Unrecoverable { damaged } = recovery else {
            panic!("{recovery:?}")
        };
        assert!(damaged.exists());
        assert!(lib.list(None).unwrap().is_empty());
    }
}
