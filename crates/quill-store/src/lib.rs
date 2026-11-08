//! The on-device library: every document, its full edit history, the
//! full-text index and the daily writing stats live in one SQLite file, with
//! verified backups copied elsewhere on the machine. Nothing here touches the
//! network.

mod backup;
mod revisions;

pub use backup::Recovery;
pub use revisions::{RevId, Revision};

use std::collections::HashMap;
use std::path::Path;

use jiff::Timestamp;
use jiff::civil::Date;
use quill_doc::{Document, Pos};
use rusqlite::{Connection, OptionalExtension, Row, params};

pub type DocId = i64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("corrupt document body: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no document with id {0}")]
    NotFound(DocId),
    #[error("no revision {0} for this document")]
    NoRevision(RevId),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq)]
pub struct DocSummary {
    pub id: DocId,
    pub title: String,
    pub collection: Option<String>,
    pub word_count: i64,
    pub opened_at: Timestamp,
    /// The start of the document's text, for the home-screen card.
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub total_words_written: i64,
    pub documents: i64,
    pub today: i64,
    pub current_streak: u32,
    pub best_streak: u32,
    /// Words written per day, oldest first, ending today; days with no
    /// writing are included as zero.
    pub recent_days: Vec<(Date, i64)>,
}

const PREVIEW_CHARS: usize = 280;

const SCHEMA: &str = "
CREATE TABLE documents (
    id          INTEGER PRIMARY KEY,
    title       TEXT NOT NULL,
    collection  TEXT,
    body        TEXT NOT NULL,
    plain_text  TEXT NOT NULL,
    word_count  INTEGER NOT NULL,
    head        INTEGER,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    opened_at   INTEGER NOT NULL,
    trashed_at  INTEGER
);
CREATE INDEX documents_opened ON documents(opened_at DESC);

-- Append-only: rows are never updated or deleted. Each one is the whole
-- document, zstd-compressed against its parent's JSON (or standalone when
-- `keyframe` is set), so the full history costs little more than the edits.
CREATE TABLE revisions (
    id             INTEGER PRIMARY KEY,
    doc_id         INTEGER NOT NULL REFERENCES documents(id),
    parent         INTEGER REFERENCES revisions(id),
    created_at     INTEGER NOT NULL,
    cursor_block   INTEGER NOT NULL,
    cursor_offset  INTEGER NOT NULL,
    word_count     INTEGER NOT NULL,
    keyframe       INTEGER NOT NULL,
    depth          INTEGER NOT NULL,
    size           INTEGER NOT NULL,
    data           BLOB NOT NULL
);
CREATE INDEX revisions_doc ON revisions(doc_id, id);
CREATE INDEX revisions_parent ON revisions(parent);

CREATE VIRTUAL TABLE documents_fts USING fts5(
    title, plain_text, content='documents', content_rowid='id'
);
CREATE TRIGGER documents_ai AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts(rowid, title, plain_text) VALUES (new.id, new.title, new.plain_text);
END;
CREATE TRIGGER documents_ad AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, plain_text)
        VALUES ('delete', old.id, old.title, old.plain_text);
END;
CREATE TRIGGER documents_au AFTER UPDATE OF title, plain_text ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, plain_text)
        VALUES ('delete', old.id, old.title, old.plain_text);
    INSERT INTO documents_fts(rowid, title, plain_text) VALUES (new.id, new.title, new.plain_text);
END;

CREATE TABLE writing_days (
    day    TEXT PRIMARY KEY,
    words  INTEGER NOT NULL
);
";

const SUMMARY_COLUMNS: &str =
    "d.id, d.title, d.collection, d.word_count, d.opened_at, substr(d.plain_text, 1, 280)";

pub struct Library {
    conn: Connection,
    /// Each open document's head revision JSON, the dictionary its next
    /// revision is compressed against.
    heads: HashMap<DocId, (RevId, String)>,
}

impl Library {
    pub fn open(path: impl AsRef<Path>) -> Result<Library> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Library> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Library> {
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        Ok(Library {
            conn,
            heads: HashMap::new(),
        })
    }

    /// Adds a document with its first revision. Its existing words don't
    /// count as written today, so importing an old manuscript doesn't
    /// inflate the stats.
    pub fn create(&mut self, doc: &Document, collection: Option<&str>) -> Result<DocId> {
        let now = Timestamp::now().as_second();
        let json = serde_json::to_string(doc)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO documents (title, collection, body, plain_text, word_count, created_at, updated_at, opened_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)",
            params![doc.title, collection, json, doc.plain_text(), doc.word_count() as i64, now],
        )?;
        let id = tx.last_insert_rowid();
        let rev = revisions::append(
            &tx,
            id,
            None,
            &json,
            doc.word_count() as i64,
            Pos::default(),
        )?;
        tx.execute(
            "UPDATE documents SET head = ?2 WHERE id = ?1",
            params![id, rev],
        )?;
        tx.commit()?;
        self.heads.insert(id, (rev, json));
        Ok(id)
    }

    /// Loads a document and marks it as just opened.
    pub fn open_document(&mut self, id: DocId) -> Result<Document> {
        let (body, head): (String, RevId) = self
            .conn
            .query_row(
                "SELECT body, head FROM documents WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(Error::NotFound(id))?;
        self.conn.execute(
            "UPDATE documents SET opened_at = ?2 WHERE id = ?1",
            params![id, Timestamp::now().as_second()],
        )?;
        let doc = serde_json::from_str(&body)?;
        self.heads.insert(id, (head, body));
        Ok(doc)
    }

    /// Records `doc` as a new revision on top of the current one, makes it
    /// current, and credits any net new words to `today`. Call once per
    /// undo step (a typing run, a paste, a formatting change).
    pub fn commit(&mut self, id: DocId, doc: &Document, cursor: Pos, today: Date) -> Result<RevId> {
        let json = serde_json::to_string(doc)?;
        let (parent, parent_json) = match self.heads.get(&id) {
            Some((rev, json)) => (*rev, json.clone()),
            None => {
                let head = self.head(id)?;
                (head, revisions::load_json(&self.conn, id, head)?)
            }
        };
        if parent_json == json {
            return Ok(parent);
        }
        let words = doc.word_count() as i64;
        let tx = self.conn.transaction()?;
        let old: i64 = tx.query_row(
            "SELECT word_count FROM documents WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let rev = revisions::append(&tx, id, Some((parent, &parent_json)), &json, words, cursor)?;
        set_current(&tx, id, doc, &json, rev)?;
        if words > old {
            tx.execute(
                "INSERT INTO writing_days (day, words) VALUES (?1, ?2)
                 ON CONFLICT(day) DO UPDATE SET words = words + excluded.words",
                params![today.to_string(), words - old],
            )?;
        }
        tx.commit()?;
        self.heads.insert(id, (rev, json));
        Ok(rev)
    }

    /// Steps back to the parent revision. Nothing is discarded: the undone
    /// revision stays in the history and `redo` returns to it.
    pub fn undo(&mut self, id: DocId) -> Result<Option<(Document, Pos)>> {
        let head = self.head(id)?;
        match revisions::parent(&self.conn, head)? {
            Some(parent) => self.checkout(id, parent).map(Some),
            None => Ok(None),
        }
    }

    /// Steps forward to the newest revision made on top of the current one.
    pub fn redo(&mut self, id: DocId) -> Result<Option<(Document, Pos)>> {
        let head = self.head(id)?;
        match revisions::newest_child(&self.conn, head)? {
            Some(child) => self.checkout(id, child).map(Some),
            None => Ok(None),
        }
    }

    /// Makes any past revision current, e.g. from the version history view.
    /// Edits made after this branch off from it; the other branch is kept.
    pub fn checkout(&mut self, id: DocId, rev: RevId) -> Result<(Document, Pos)> {
        let json = revisions::load_json(&self.conn, id, rev)?;
        let doc: Document = serde_json::from_str(&json)?;
        let cursor = revisions::cursor(&self.conn, rev)?;
        let tx = self.conn.transaction()?;
        set_current(&tx, id, &doc, &json, rev)?;
        tx.commit()?;
        self.heads.insert(id, (rev, json));
        Ok((doc, cursor))
    }

    /// Every revision of a document, oldest first.
    pub fn revisions(&self, id: DocId) -> Result<Vec<Revision>> {
        revisions::list(&self.conn, id)
    }

    pub fn head(&self, id: DocId) -> Result<RevId> {
        self.conn
            .query_row("SELECT head FROM documents WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or(Error::NotFound(id))
    }

    /// Moves a document to the trash. It and its history stay in the
    /// library and can be restored.
    pub fn trash(&mut self, id: DocId) -> Result<()> {
        self.conn.execute(
            "UPDATE documents SET trashed_at = ?2 WHERE id = ?1",
            params![id, Timestamp::now().as_second()],
        )?;
        Ok(())
    }

    pub fn restore(&mut self, id: DocId) -> Result<()> {
        self.conn
            .execute("UPDATE documents SET trashed_at = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    /// All documents (or one collection's) not in the trash, most recently
    /// opened first.
    pub fn list(&self, collection: Option<&str>) -> Result<Vec<DocSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM documents d
             WHERE d.trashed_at IS NULL AND (?1 IS NULL OR d.collection = ?1)
             ORDER BY d.opened_at DESC, d.id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([collection], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn trashed(&self) -> Result<Vec<DocSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM documents d WHERE d.trashed_at IS NOT NULL ORDER BY d.trashed_at DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn collections(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT collection FROM documents
             WHERE collection IS NOT NULL AND trashed_at IS NULL ORDER BY collection",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Full-text search over titles and bodies; each word matches as a prefix.
    pub fn search(&self, query: &str) -> Result<Vec<DocSummary>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
            .collect();
        if terms.is_empty() {
            return self.list(None);
        }
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM documents_fts f JOIN documents d ON d.id = f.rowid
             WHERE documents_fts MATCH ?1 AND d.trashed_at IS NULL ORDER BY rank"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([terms.join(" ")], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn stats(&self, today: Date, recent: usize) -> Result<Stats> {
        let documents: i64 = self.conn.query_row(
            "SELECT count(*) FROM documents WHERE trashed_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        let mut stmt = self
            .conn
            .prepare("SELECT day, words FROM writing_days WHERE words > 0 ORDER BY day")?;
        let days = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .filter_map(|row| {
                row.ok()
                    .and_then(|(d, w)| d.parse::<Date>().ok().map(|d| (d, w)))
            })
            .collect::<Vec<_>>();

        let words_on = |date: Date| days.iter().find(|(d, _)| *d == date).map_or(0, |(_, w)| *w);

        let mut best = 0;
        let mut run = 0;
        let mut prev: Option<Date> = None;
        for &(day, _) in &days {
            run = match prev {
                Some(p) if p.tomorrow().ok() == Some(day) => run + 1,
                _ => 1,
            };
            best = best.max(run);
            prev = Some(day);
        }

        // A streak survives until the end of the day after the last writing day.
        let mut current = 0;
        let mut cursor = if words_on(today) > 0 {
            Some(today)
        } else {
            today.yesterday().ok()
        };
        while let Some(day) = cursor.filter(|&d| words_on(d) > 0) {
            current += 1;
            cursor = day.yesterday().ok();
        }

        let mut recent_days = Vec::with_capacity(recent);
        let mut day = today;
        for _ in 0..recent {
            recent_days.push((day, words_on(day)));
            match day.yesterday() {
                Ok(d) => day = d,
                Err(_) => break,
            }
        }
        recent_days.reverse();

        Ok(Stats {
            total_words_written: days.iter().map(|(_, w)| w).sum(),
            documents,
            today: words_on(today),
            current_streak: current,
            best_streak: best,
            recent_days,
        })
    }
}

/// Today's date in the system time zone.
pub fn today() -> Date {
    jiff::Zoned::now().date()
}

fn set_current(
    tx: &rusqlite::Transaction<'_>,
    id: DocId,
    doc: &Document,
    json: &str,
    rev: RevId,
) -> Result<()> {
    tx.execute(
        "UPDATE documents SET title = ?2, body = ?3, plain_text = ?4, word_count = ?5, head = ?6, updated_at = ?7
         WHERE id = ?1",
        params![id, doc.title, json, doc.plain_text(), doc.word_count() as i64, rev, Timestamp::now().as_second()],
    )?;
    Ok(())
}

fn summary_from_row(r: &Row<'_>) -> rusqlite::Result<DocSummary> {
    let preview: String = r.get(5)?;
    Ok(DocSummary {
        id: r.get(0)?,
        title: r.get(1)?,
        collection: r.get(2)?,
        word_count: r.get(3)?,
        opened_at: Timestamp::from_second(r.get(4)?).unwrap_or(Timestamp::UNIX_EPOCH),
        preview: preview.chars().take(PREVIEW_CHARS).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    const DAY: Date = date(2026, 11, 7);

    fn lib_with(docs: &[(&str, &str, Option<&str>)]) -> (Library, Vec<DocId>) {
        let mut lib = Library::open_in_memory().unwrap();
        let ids = docs
            .iter()
            .map(|(title, text, col)| {
                lib.create(&Document::from_plain_text(*title, text), *col)
                    .unwrap()
            })
            .collect();
        (lib, ids)
    }

    #[test]
    fn create_open_commit_round_trip() {
        let (mut lib, ids) = lib_with(&[("Draft", "The lamp had burned", None)]);
        let mut doc = lib.open_document(ids[0]).unwrap();
        let end = doc.end();
        let cursor = doc.insert_text(end, " every night.");
        lib.commit(ids[0], &doc, cursor, DAY).unwrap();
        assert_eq!(lib.open_document(ids[0]).unwrap(), doc);
        assert!(matches!(lib.open_document(999), Err(Error::NotFound(999))));
    }

    #[test]
    fn list_filters_by_collection_and_hides_trash() {
        let (mut lib, ids) = lib_with(&[
            ("A", "", Some("Novel")),
            ("B", "", Some("Letters")),
            ("C", "", None),
        ]);
        assert_eq!(lib.list(None).unwrap().len(), 3);
        let novel = lib.list(Some("Novel")).unwrap();
        assert_eq!(novel.len(), 1);
        assert_eq!(novel[0].title, "A");
        assert_eq!(lib.collections().unwrap(), ["Letters", "Novel"]);

        lib.trash(ids[1]).unwrap();
        assert_eq!(lib.list(None).unwrap().len(), 2);
        assert_eq!(lib.collections().unwrap(), ["Novel"]);
        assert_eq!(lib.trashed().unwrap()[0].id, ids[1]);
        lib.restore(ids[1]).unwrap();
        assert_eq!(lib.list(None).unwrap().len(), 3);
    }

    #[test]
    fn search_matches_prefixes_and_follows_edits() {
        let (mut lib, ids) = lib_with(&[
            ("Lighthouse", "Elias let the lamp go dark", None),
            ("Letter", "Dear Grandma, thank you for the scarf", None),
        ]);
        let hits = lib.search("ligh").unwrap();
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), [ids[0]]);
        assert_eq!(lib.search("scarf").unwrap()[0].id, ids[1]);
        assert!(lib.search("\"unbalanced").unwrap().is_empty());

        let mut doc = lib.open_document(ids[1]).unwrap();
        let cursor = doc.delete(Pos::new(0, 0), doc.end());
        lib.commit(ids[1], &doc, cursor, DAY).unwrap();
        assert!(lib.search("scarf").unwrap().is_empty());
    }

    #[test]
    fn stats_track_words_and_streaks() {
        let (mut lib, ids) = lib_with(&[("Imported", "these five words are old", None)]);
        let id = ids[0];
        let mut doc = lib.open_document(id).unwrap();
        let mut write = |doc: &mut Document, words: &str, day: Date| {
            let end = doc.end();
            let cursor = doc.insert_text(end, words);
            lib.commit(id, doc, cursor, day).unwrap();
        };
        write(&mut doc, " one two", date(2026, 11, 1));
        // Nov 2 skipped: breaks the first streak.
        write(&mut doc, " three", date(2026, 11, 3));
        write(&mut doc, " four five", date(2026, 11, 4));
        write(&mut doc, " six", date(2026, 11, 4));
        write(&mut doc, " seven", date(2026, 11, 5));

        // Deleting words never subtracts from what was written.
        let cursor = doc.delete(Pos::new(0, 0), Pos::new(0, 6));
        lib.commit(id, &doc, cursor, date(2026, 11, 5)).unwrap();

        let stats = lib.stats(date(2026, 11, 6), 7).unwrap();
        assert_eq!(stats.total_words_written, 7);
        assert_eq!(stats.documents, 1);
        assert_eq!(stats.today, 0);
        assert_eq!(
            stats.current_streak, 3,
            "streak survives a day with no writing yet"
        );
        assert_eq!(stats.best_streak, 3);
        assert_eq!(stats.recent_days.len(), 7);
        assert_eq!(stats.recent_days[0], (date(2026, 10, 31), 0));
        assert_eq!(stats.recent_days[4], (date(2026, 11, 4), 3));

        assert_eq!(lib.stats(date(2026, 11, 7), 1).unwrap().current_streak, 0);
    }

    #[test]
    fn reopening_a_file_keeps_data_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let id = {
            let mut lib = Library::open(&path).unwrap();
            let mut doc = Document::from_plain_text("Kept", "hi");
            let id = lib.create(&doc, None).unwrap();
            let cursor = doc.insert_text(doc.end(), " there");
            lib.commit(id, &doc, cursor, DAY).unwrap();
            id
        };
        let mut lib = Library::open(&path).unwrap();
        assert_eq!(lib.open_document(id).unwrap().plain_text(), "hi there");
        let (doc, _) = lib.undo(id).unwrap().unwrap();
        assert_eq!(doc.plain_text(), "hi", "undo works across restarts");
    }
}
