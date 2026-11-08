//! The permanent edit history. Revisions form a tree (each points at its
//! parent, like git commits), so undoing and then editing starts a new
//! branch instead of throwing the old future away.
//!
//! Storage is delta-compressed: a revision's JSON is zstd-compressed using
//! its parent's JSON as the dictionary, which turns a one-word edit to a long
//! manuscript into a few dozen bytes. Every `KEYFRAME_EVERY` revisions one is
//! stored standalone, bounding how far back a load has to walk. Each frame
//! carries a zstd checksum, so a damaged row fails loudly instead of
//! returning wrong text.

use std::io;

use jiff::Timestamp;
use quill_doc::Pos;
use rusqlite::{Connection, OptionalExtension, params};
use zstd::bulk::{Compressor, Decompressor};
use zstd::stream::raw::CParameter;

use crate::{DocId, Error, Result};

pub type RevId = i64;

#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    pub id: RevId,
    pub parent: Option<RevId>,
    pub created_at: Timestamp,
    pub word_count: i64,
}

const KEYFRAME_EVERY: i64 = 64;
const LEVEL: i32 = 3;

pub(crate) fn append(
    conn: &Connection,
    doc_id: DocId,
    parent: Option<(RevId, &str)>,
    json: &str,
    word_count: i64,
    cursor: Pos,
) -> Result<RevId> {
    let parent_depth = match parent {
        Some((rev, _)) => {
            Some(conn.query_row("SELECT depth FROM revisions WHERE id = ?1", [rev], |r| r.get::<_, i64>(0))?)
        }
        None => None,
    };
    let (dict, depth) = match (parent, parent_depth) {
        (Some((_, parent_json)), Some(d)) if d + 1 < KEYFRAME_EVERY => (Some(parent_json), d + 1),
        _ => (None, 0),
    };
    let data = compress(json, dict)?;
    conn.execute(
        "INSERT INTO revisions
            (doc_id, parent, created_at, cursor_block, cursor_offset, word_count, keyframe, depth, size, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            doc_id,
            parent.map(|(rev, _)| rev),
            Timestamp::now().as_second(),
            cursor.block as i64,
            cursor.offset as i64,
            word_count,
            dict.is_none(),
            depth,
            json.len() as i64,
            data,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Rebuilds a revision's JSON by walking back to the nearest keyframe and
/// replaying the deltas forward.
pub(crate) fn load_json(conn: &Connection, doc_id: DocId, rev: RevId) -> Result<String> {
    let mut stmt = conn.prepare_cached("SELECT doc_id, parent, keyframe, size, data FROM revisions WHERE id = ?1")?;
    let mut chain = Vec::new();
    let mut next = Some(rev);
    while let Some(id) = next {
        let (owner, parent, keyframe, size, data): (DocId, Option<RevId>, bool, i64, Vec<u8>) = stmt
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .optional()?
            .ok_or(Error::NoRevision(id))?;
        if owner != doc_id {
            return Err(Error::NoRevision(rev));
        }
        chain.push((size as usize, data));
        next = if keyframe { None } else { parent };
    }
    let mut json: Option<String> = None;
    for (size, data) in chain.iter().rev() {
        json = Some(decompress(data, *size, json.as_deref())?);
    }
    json.ok_or(Error::NoRevision(rev))
}

pub(crate) fn parent(conn: &Connection, rev: RevId) -> Result<Option<RevId>> {
    Ok(conn.query_row("SELECT parent FROM revisions WHERE id = ?1", [rev], |r| r.get(0))?)
}

pub(crate) fn newest_child(conn: &Connection, rev: RevId) -> Result<Option<RevId>> {
    Ok(conn
        .query_row("SELECT id FROM revisions WHERE parent = ?1 ORDER BY id DESC LIMIT 1", [rev], |r| r.get(0))
        .optional()?)
}

pub(crate) fn cursor(conn: &Connection, rev: RevId) -> Result<Pos> {
    Ok(conn.query_row("SELECT cursor_block, cursor_offset FROM revisions WHERE id = ?1", [rev], |r| {
        Ok(Pos::new(r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize))
    })?)
}

pub(crate) fn list(conn: &Connection, doc_id: DocId) -> Result<Vec<Revision>> {
    let mut stmt =
        conn.prepare("SELECT id, parent, created_at, word_count FROM revisions WHERE doc_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([doc_id], |r| {
        Ok(Revision {
            id: r.get(0)?,
            parent: r.get(1)?,
            created_at: Timestamp::from_second(r.get(2)?).unwrap_or(Timestamp::UNIX_EPOCH),
            word_count: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn compress(json: &str, dict: Option<&str>) -> io::Result<Vec<u8>> {
    let mut compressor = match dict {
        Some(dict) => Compressor::with_dictionary(LEVEL, dict.as_bytes())?,
        None => Compressor::new(LEVEL)?,
    };
    compressor.set_parameter(CParameter::ChecksumFlag(true))?;
    compressor.compress(json.as_bytes())
}

fn decompress(data: &[u8], size: usize, dict: Option<&str>) -> io::Result<String> {
    let mut decompressor = match dict {
        Some(dict) => Decompressor::with_dictionary(dict.as_bytes())?,
        None => Decompressor::new()?,
    };
    let bytes = decompressor.decompress(data, size)?;
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use quill_doc::{Document, Marks};

    use crate::Library;

    use super::*;

    const DAY: jiff::civil::Date = date(2026, 11, 7);

    fn type_words(lib: &mut Library, id: DocId, doc: &mut Document, words: &[&str]) -> Vec<RevId> {
        words
            .iter()
            .map(|w| {
                let cursor = doc.insert_text(doc.end(), w);
                lib.commit(id, doc, cursor, DAY).unwrap()
            })
            .collect()
    }

    #[test]
    fn undo_redo_walks_the_history() {
        let mut lib = Library::open_in_memory().unwrap();
        let mut doc = Document::new("t");
        let id = lib.create(&doc, None).unwrap();
        type_words(&mut lib, id, &mut doc, &["one", " two", " three"]);

        assert_eq!(lib.undo(id).unwrap().unwrap().0.plain_text(), "one two");
        assert_eq!(lib.undo(id).unwrap().unwrap().0.plain_text(), "one");
        let (doc, cursor) = lib.redo(id).unwrap().unwrap();
        assert_eq!(doc.plain_text(), "one two");
        assert_eq!(cursor, Pos::new(0, 7), "cursor comes back with the revision");
        assert_eq!(lib.open_document(id).unwrap().plain_text(), "one two", "head is persisted");

        lib.undo(id).unwrap();
        assert!(lib.undo(id).unwrap().is_some_and(|(d, _)| d.plain_text().is_empty()));
        assert!(lib.undo(id).unwrap().is_none(), "nothing before the first revision");
    }

    #[test]
    fn editing_after_undo_branches_and_keeps_both_futures() {
        let mut lib = Library::open_in_memory().unwrap();
        let mut doc = Document::new("t");
        let id = lib.create(&doc, None).unwrap();
        let first = type_words(&mut lib, id, &mut doc, &["base", " old-ending"]);

        let (mut doc, _) = lib.undo(id).unwrap().unwrap();
        type_words(&mut lib, id, &mut doc, &[" new-ending"]);
        assert_eq!(lib.undo(id).unwrap().unwrap().0.plain_text(), "base");
        assert_eq!(lib.redo(id).unwrap().unwrap().0.plain_text(), "base new-ending", "redo follows the newest branch");

        let (old, _) = lib.checkout(id, first[1]).unwrap();
        assert_eq!(old.plain_text(), "base old-ending", "the abandoned branch is still there");
        assert_eq!(lib.revisions(id).unwrap().len(), 4);
    }

    #[test]
    fn long_histories_round_trip_through_keyframes() {
        let mut lib = Library::open_in_memory().unwrap();
        let mut doc = Document::new("t");
        let id = lib.create(&doc, None).unwrap();
        let mut expected = vec![String::new()];
        let mut revs = vec![lib.head(id).unwrap()];
        for i in 0..(KEYFRAME_EVERY * 2 + 5) {
            let cursor = doc.insert_text(doc.end(), &format!(" w{i}"));
            if i % 10 == 0 {
                doc.toggle_mark(Pos::new(0, 0), cursor, Marks::ITALIC);
            }
            revs.push(lib.commit(id, &doc, cursor, DAY).unwrap());
            expected.push(doc.plain_text());
        }
        let mut fresh = Library { conn: lib.conn, heads: Default::default() };
        for (rev, text) in revs.iter().zip(&expected) {
            assert_eq!(&fresh.checkout(id, *rev).unwrap().0.plain_text(), text);
        }
    }

    #[test]
    fn deltas_are_small() {
        let mut lib = Library::open_in_memory().unwrap();
        let paragraph = "The lamp had burned every night for forty-one years. ".repeat(40);
        let mut doc = Document::from_plain_text("t", &vec![paragraph.as_str(); 50].join("\n"));
        let id = lib.create(&doc, None).unwrap();
        let rev = type_words(&mut lib, id, &mut doc, &[" Then the storm came."])[0];
        let size: i64 =
            lib.conn.query_row("SELECT length(data) FROM revisions WHERE id = ?1", [rev], |r| r.get(0)).unwrap();
        assert!(size < 400, "a one-sentence edit to a ~100 KB document stored {size} bytes");
    }

    #[test]
    fn unchanged_commit_is_a_no_op() {
        let mut lib = Library::open_in_memory().unwrap();
        let doc = Document::from_plain_text("t", "same");
        let id = lib.create(&doc, None).unwrap();
        let head = lib.head(id).unwrap();
        assert_eq!(lib.commit(id, &doc, Pos::default(), DAY).unwrap(), head);
        assert_eq!(lib.revisions(id).unwrap().len(), 1);
    }

    #[test]
    fn damaged_revision_is_detected() {
        let mut lib = Library::open_in_memory().unwrap();
        let mut doc = Document::from_plain_text("t", "some text worth keeping");
        let id = lib.create(&doc, None).unwrap();
        let rev = type_words(&mut lib, id, &mut doc, &[" more"])[0];
        lib.conn
            .execute(
                "UPDATE revisions SET data = substr(data, 1, length(data) - 6) || x'000000000000' WHERE id = ?1",
                [rev],
            )
            .unwrap();
        assert!(load_json(&lib.conn, id, rev).is_err());
    }
}
