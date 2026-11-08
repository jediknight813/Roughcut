//! Where the library lives on disk, and the backup schedule.
//!
//! The library sits in `~/Library/Application Support/Quill`. Backups go to
//! two other folders: one beside it (frequent, short retention) and one in
//! `~/Documents/Quill Backups` (daily-ish, longer retention, and picked up by
//! Time Machine or any other backup tool the user already runs). Setting
//! `QUILL_HOME` puts all three under one folder instead, for development.

use std::path::PathBuf;
use std::time::Duration;

use quill_store::{Library, Recovery};

pub struct BackupTarget {
    pub dir: PathBuf,
    pub keep: usize,
}

pub struct Locations {
    pub library: PathBuf,
    pub backups: Vec<BackupTarget>,
}

pub const BACKUP_EVERY: Duration = Duration::from_secs(30 * 60);

impl Locations {
    pub fn resolve() -> Locations {
        if let Some(home) = std::env::var_os("QUILL_HOME").map(PathBuf::from) {
            return Locations {
                library: home.join("library.db"),
                backups: vec![
                    BackupTarget { dir: home.join("Backups"), keep: 48 },
                    BackupTarget { dir: home.join("Documents Backups"), keep: 30 },
                ],
            };
        }
        let base = directories::BaseDirs::new().expect("no home directory");
        let app = base.data_dir().join("Quill");
        let documents = directories::UserDirs::new()
            .and_then(|u| u.document_dir().map(PathBuf::from))
            .unwrap_or_else(|| base.home_dir().join("Documents"));
        Locations {
            library: app.join("library.db"),
            backups: vec![
                BackupTarget { dir: app.join("Backups"), keep: 48 },
                BackupTarget { dir: documents.join("Quill Backups"), keep: 30 },
            ],
        }
    }

    pub fn open(&self) -> quill_store::Result<(Library, Recovery)> {
        if let Some(parent) = self.library.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let dirs: Vec<PathBuf> = self.backups.iter().map(|b| b.dir.clone()).collect();
        Library::open_or_recover(&self.library, &dirs)
    }

    /// Backs up to every target. A failure in one doesn't stop the others.
    pub fn back_up(&self, library: &Library) {
        for target in &self.backups {
            if let Err(err) = library.backup_to(&target.dir, target.keep) {
                eprintln!("quill: backup to {} failed: {err}", target.dir.display());
            }
        }
    }
}

pub fn describe(recovery: &Recovery) -> Option<String> {
    match recovery {
        Recovery::Clean | Recovery::New => None,
        Recovery::Restored { from, .. } => {
            Some(format!("Your library was damaged or missing and was restored from {}.", from.display()))
        }
        Recovery::Unrecoverable { damaged } => Some(format!(
            "Your library was damaged and no backup could be read. The damaged file was kept at {}.",
            damaged.display()
        )),
    }
}
