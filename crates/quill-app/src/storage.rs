//! Where the library lives on disk, and the backup schedule.
//!
//! New libraries live in `~/Library/Application Support/Roughcut`. Backups go to
//! two other folders: one beside it (frequent, short retention) and one in
//! `~/Documents/Roughcut Backups` (daily-ish, longer retention, and picked up by
//! Time Machine or any other backup tool the user already runs). Setting
//! `ROUGHCUT_HOME` puts all three under one folder instead, for development.
//! Existing Quill libraries, backups and the QUILL_HOME override remain supported.

use std::path::{Path, PathBuf};
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
        if let Some(home) =
            std::env::var_os("ROUGHCUT_HOME").or_else(|| std::env::var_os("QUILL_HOME")).map(PathBuf::from)
        {
            return Locations {
                library: home.join("library.db"),
                backups: vec![
                    BackupTarget { dir: home.join("Backups"), keep: 48 },
                    BackupTarget { dir: home.join("Documents Backups"), keep: 30 },
                ],
            };
        }
        let base = directories::BaseDirs::new().expect("no home directory");
        let documents = directories::UserDirs::new()
            .and_then(|u| u.document_dir().map(PathBuf::from))
            .unwrap_or_else(|| base.home_dir().join("Documents"));
        Self::under(base.data_dir(), &documents)
    }

    fn under(data: &Path, documents: &Path) -> Locations {
        let current = data.join("Roughcut");
        let legacy = data.join("Quill");
        let current_data = current.join("library.db").exists()
            || current.join("Backups").exists()
            || documents.join("Roughcut Backups").exists();
        let use_legacy = !current_data && (legacy.exists() || documents.join("Quill Backups").exists());
        let app = if use_legacy { legacy } else { current };
        let backup_name = if use_legacy { "Quill Backups" } else { "Roughcut Backups" };
        Locations {
            library: app.join("library.db"),
            backups: vec![
                BackupTarget { dir: app.join("Backups"), keep: 48 },
                BackupTarget { dir: documents.join(backup_name), keep: 30 },
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
                eprintln!("roughcut: backup to {} failed: {err}", target.dir.display());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebrand_reopens_existing_libraries_and_their_backups() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("Application Support");
        let docs = root.path().join("Documents");
        let fresh = Locations::under(&data, &docs);
        assert_eq!(fresh.library, data.join("Roughcut/library.db"));
        assert_eq!(fresh.backups[1].dir, docs.join("Roughcut Backups"));

        std::fs::create_dir_all(data.join("Quill")).unwrap();
        let mut library = Library::open(data.join("Quill/library.db")).unwrap();
        let id = library.create(&quill_doc::Document::new("Keep my writing"), None).unwrap();
        drop(library);
        let existing = Locations::under(&data, &docs);
        assert_eq!(existing.backups[1].dir, docs.join("Quill Backups"));
        let (mut reopened, _) = existing.open().unwrap();
        assert_eq!(reopened.open_document(id).unwrap().title, "Keep my writing");

        std::fs::create_dir_all(data.join("Roughcut")).unwrap();
        // An empty folder must not hide the established library.
        assert_eq!(Locations::under(&data, &docs).library, existing.library);
        let _ = Library::open(data.join("Roughcut/library.db")).unwrap();
        assert_eq!(Locations::under(&data, &docs).library, fresh.library);
    }

    #[test]
    fn legacy_backups_remain_discoverable_without_a_library() {
        let root = tempfile::tempdir().unwrap();
        let docs = root.path().join("Documents");
        std::fs::create_dir_all(docs.join("Quill Backups")).unwrap();
        let locations = Locations::under(root.path(), &docs);
        assert_eq!(locations.library, root.path().join("Quill/library.db"));
        assert_eq!(locations.backups[1].dir, docs.join("Quill Backups"));
    }
}
