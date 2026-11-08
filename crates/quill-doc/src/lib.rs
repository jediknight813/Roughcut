//! Quill's document model: a list of blocks, each a run of marked text spans.
//!
//! Positions are `(block, byte offset)` pairs and must always sit on a
//! grapheme boundary; the editing methods keep them there.

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// Inline formatting, as a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Marks(u8);

impl Marks {
    pub const NONE: Marks = Marks(0);
    pub const BOLD: Marks = Marks(1);
    pub const ITALIC: Marks = Marks(1 << 1);
    pub const UNDERLINE: Marks = Marks(1 << 2);
    pub const STRIKE: Marks = Marks(1 << 3);
    pub const HIGHLIGHT: Marks = Marks(1 << 4);

    pub fn contains(self, other: Marks) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn with(self, other: Marks) -> Marks {
        Marks(self.0 | other.0)
    }

    pub fn without(self, other: Marks) -> Marks {
        Marks(self.0 & !other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    #[default]
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Quote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    pub marks: Marks,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub spans: Vec<Span>,
}

impl Block {
    pub fn new(kind: BlockKind, text: &str) -> Block {
        let mut block = Block { kind, spans: Vec::new() };
        block.insert(0, text, Marks::NONE);
        block
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.spans.iter().map(|s| s.text.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Marks that text typed at `offset` should inherit: those of the
    /// character just before it, or of the first character at the start.
    pub fn marks_at(&self, offset: usize) -> Marks {
        let mut start = 0;
        for span in &self.spans {
            let end = start + span.text.len();
            if offset > start && offset <= end {
                return span.marks;
            }
            start = end;
        }
        self.spans.first().map_or(Marks::NONE, |s| s.marks)
    }

    pub fn insert(&mut self, offset: usize, text: &str, marks: Marks) {
        if text.is_empty() {
            return;
        }
        let idx = self.split_at(offset);
        self.spans.insert(idx, Span { text: text.to_owned(), marks });
        self.normalize();
    }

    pub fn delete(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let first = self.split_at(start);
        let last = self.split_at(end);
        self.spans.drain(first..last);
        self.normalize();
    }

    /// Splits the block at `offset`, keeping the head and returning the tail.
    /// A heading's tail becomes a paragraph, as in most word processors.
    pub fn split_off(&mut self, offset: usize) -> Block {
        let idx = self.split_at(offset);
        let tail = self.spans.split_off(idx);
        let kind = match self.kind {
            BlockKind::Heading(_) => BlockKind::Paragraph,
            kind => kind,
        };
        self.normalize();
        Block { kind, spans: tail }
    }

    pub fn append(&mut self, other: Block) {
        self.spans.extend(other.spans);
        self.normalize();
    }

    pub fn update_marks(&mut self, start: usize, end: usize, f: impl Fn(Marks) -> Marks) {
        if start >= end {
            return;
        }
        let first = self.split_at(start);
        let last = self.split_at(end);
        for span in &mut self.spans[first..last] {
            span.marks = f(span.marks);
        }
        self.normalize();
    }

    pub fn all_have(&self, start: usize, end: usize, mark: Marks) -> bool {
        let mut pos = 0;
        for span in &self.spans {
            let span_end = pos + span.text.len();
            if span_end > start && pos < end && !span.marks.contains(mark) {
                return false;
            }
            pos = span_end;
        }
        true
    }

    /// Ensures a span boundary at `offset` and returns the index of the span
    /// that starts there (or `spans.len()` at the end).
    fn split_at(&mut self, offset: usize) -> usize {
        let mut start = 0;
        for i in 0..self.spans.len() {
            let len = self.spans[i].text.len();
            if offset == start {
                return i;
            }
            if offset < start + len {
                let tail = self.spans[i].text.split_off(offset - start);
                let marks = self.spans[i].marks;
                self.spans.insert(i + 1, Span { text: tail, marks });
                return i + 1;
            }
            start += len;
        }
        assert!(offset == start, "offset {offset} past end of block ({start})");
        self.spans.len()
    }

    fn normalize(&mut self) {
        let mut merged: Vec<Span> = Vec::with_capacity(self.spans.len());
        for span in self.spans.drain(..) {
            if span.text.is_empty() {
                continue;
            }
            match merged.last_mut() {
                Some(prev) if prev.marks == span.marks => prev.text.push_str(&span.text),
                _ => merged.push(span),
            }
        }
        self.spans = merged;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Pos {
    pub block: usize,
    pub offset: usize,
}

impl Pos {
    pub fn new(block: usize, offset: usize) -> Pos {
        Pos { block, offset }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub title: String,
    pub blocks: Vec<Block>,
}

impl Document {
    pub fn new(title: impl Into<String>) -> Document {
        Document { title: title.into(), blocks: vec![Block::default()] }
    }

    /// One paragraph per line.
    pub fn from_plain_text(title: impl Into<String>, text: &str) -> Document {
        let blocks = text.lines().map(|line| Block::new(BlockKind::Paragraph, line)).collect::<Vec<_>>();
        let mut doc = Document { title: title.into(), blocks };
        if doc.blocks.is_empty() {
            doc.blocks.push(Block::default());
        }
        doc
    }

    pub fn plain_text(&self) -> String {
        self.blocks.iter().map(Block::text).collect::<Vec<_>>().join("\n")
    }

    pub fn word_count(&self) -> usize {
        self.blocks.iter().map(|b| b.text().unicode_words().count()).sum()
    }

    pub fn end(&self) -> Pos {
        let last = self.blocks.len() - 1;
        Pos::new(last, self.blocks[last].len())
    }

    /// Inserts `text` at `pos`, turning each `\n` into a new block, and
    /// returns the position just after the inserted text.
    pub fn insert_text(&mut self, pos: Pos, text: &str) -> Pos {
        let marks = self.blocks[pos.block].marks_at(pos.offset);
        let mut pos = pos;
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                pos = self.split_block(pos);
            }
            self.blocks[pos.block].insert(pos.offset, line, marks);
            pos.offset += line.len();
        }
        pos
    }

    /// Deletes the text between two positions (in either order) and returns
    /// where the cursor should land.
    pub fn delete(&mut self, a: Pos, b: Pos) -> Pos {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if start.block == end.block {
            self.blocks[start.block].delete(start.offset, end.offset);
            return start;
        }
        let tail = self.blocks[end.block].split_off(end.offset);
        self.blocks.drain(start.block + 1..=end.block);
        let first = &mut self.blocks[start.block];
        first.delete(start.offset, first.len());
        first.append(tail);
        start
    }

    /// Enter: splits the block at `pos` and returns the start of the new one.
    pub fn split_block(&mut self, pos: Pos) -> Pos {
        let tail = self.blocks[pos.block].split_off(pos.offset);
        self.blocks.insert(pos.block + 1, tail);
        Pos::new(pos.block + 1, 0)
    }

    /// Adds `mark` to the range, or removes it if the whole range already has it.
    pub fn toggle_mark(&mut self, a: Pos, b: Pos, mark: Marks) {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let ranges = self.block_ranges(start, end);
        let all = ranges.iter().all(|&(i, s, e)| self.blocks[i].all_have(s, e, mark));
        for (i, s, e) in ranges {
            self.blocks[i].update_marks(s, e, |m| if all { m.without(mark) } else { m.with(mark) });
        }
    }

    pub fn set_block_kind(&mut self, block: usize, kind: BlockKind) {
        self.blocks[block].kind = kind;
    }

    /// The position one grapheme before `pos`, crossing into the previous block.
    pub fn prev_pos(&self, pos: Pos) -> Pos {
        if pos.offset == 0 {
            return match pos.block {
                0 => pos,
                b => Pos::new(b - 1, self.blocks[b - 1].len()),
            };
        }
        let text = self.blocks[pos.block].text();
        let step = text[..pos.offset].graphemes(true).next_back().map_or(0, str::len);
        Pos::new(pos.block, pos.offset - step)
    }

    /// The position one grapheme after `pos`, crossing into the next block.
    pub fn next_pos(&self, pos: Pos) -> Pos {
        let text = self.blocks[pos.block].text();
        if pos.offset >= text.len() {
            return if pos.block + 1 < self.blocks.len() { Pos::new(pos.block + 1, 0) } else { pos };
        }
        let step = text[pos.offset..].graphemes(true).next().map_or(0, str::len);
        Pos::new(pos.block, pos.offset + step)
    }

    /// Markdown for export. Underline and highlight have no Markdown syntax
    /// and are written as plain text.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let mut number = 0;
        for (i, block) in self.blocks.iter().enumerate() {
            let list = matches!(block.kind, BlockKind::Bullet | BlockKind::Numbered);
            let prev_list = i > 0 && matches!(self.blocks[i - 1].kind, BlockKind::Bullet | BlockKind::Numbered);
            if i > 0 {
                out.push_str(if list && prev_list { "\n" } else { "\n\n" });
            }
            number = if block.kind == BlockKind::Numbered { number + 1 } else { 0 };
            match block.kind {
                BlockKind::Heading(1) => out.push_str("# "),
                BlockKind::Heading(_) => out.push_str("## "),
                BlockKind::Bullet => out.push_str("- "),
                BlockKind::Numbered => out.push_str(&format!("{number}. ")),
                BlockKind::Quote => out.push_str("> "),
                BlockKind::Paragraph => {}
            }
            for span in &block.spans {
                let mut wrap = String::new();
                if span.marks.contains(Marks::BOLD) {
                    wrap.push_str("**");
                }
                if span.marks.contains(Marks::ITALIC) {
                    wrap.push('_');
                }
                if span.marks.contains(Marks::STRIKE) {
                    wrap.push_str("~~");
                }
                // Emphasis markers must hug the text, so surrounding spaces move outside.
                let text = escape_markdown(&span.text);
                let trimmed = text.trim();
                if wrap.is_empty() || trimmed.is_empty() {
                    out.push_str(&text);
                    continue;
                }
                let lead = &text[..text.len() - text.trim_start().len()];
                let trail = &text[text.trim_end().len()..];
                let closing: String = wrap.chars().rev().collect();
                out.push_str(&format!("{lead}{wrap}{trimmed}{closing}{trail}"));
            }
        }
        out.push('\n');
        out
    }

    /// Byte offset of `pos` in [`Document::plain_text`], where blocks are
    /// joined by `\n`. Platform text input (IME) addresses text this way.
    pub fn flat_offset(&self, pos: Pos) -> usize {
        self.blocks[..pos.block].iter().map(|b| b.len() + 1).sum::<usize>() + pos.offset
    }

    /// Inverse of [`Document::flat_offset`], clamped to the document.
    pub fn pos_at_flat(&self, mut offset: usize) -> Pos {
        for (i, block) in self.blocks.iter().enumerate() {
            if offset <= block.len() {
                return Pos::new(i, offset);
            }
            offset -= block.len() + 1;
        }
        self.end()
    }

    fn block_ranges(&self, start: Pos, end: Pos) -> Vec<(usize, usize, usize)> {
        (start.block..=end.block)
            .map(|i| {
                let s = if i == start.block { start.offset } else { 0 };
                let e = if i == end.block { end.offset } else { self.blocks[i].len() };
                (i, s, e)
            })
            .collect()
    }
}

fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '*' | '_' | '~' | '`') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &Document) -> Vec<String> {
        doc.blocks.iter().map(Block::text).collect()
    }

    #[test]
    fn insert_and_split() {
        let mut doc = Document::new("t");
        let pos = doc.insert_text(Pos::default(), "Hello world");
        assert_eq!(pos, Pos::new(0, 11));
        let pos = doc.split_block(Pos::new(0, 5));
        assert_eq!(pos, Pos::new(1, 0));
        assert_eq!(texts(&doc), ["Hello", " world"]);
    }

    #[test]
    fn insert_multiline_text_creates_blocks() {
        let mut doc = Document::from_plain_text("t", "ab");
        let pos = doc.insert_text(Pos::new(0, 1), "1\n2\n3");
        assert_eq!(texts(&doc), ["a1", "2", "3b"]);
        assert_eq!(pos, Pos::new(2, 1));
    }

    #[test]
    fn delete_across_blocks_joins_them() {
        let mut doc = Document::from_plain_text("t", "first line\nmiddle\nlast line");
        let pos = doc.delete(Pos::new(2, 5), Pos::new(0, 6));
        assert_eq!(pos, Pos::new(0, 6));
        assert_eq!(texts(&doc), ["first line"]);
    }

    #[test]
    fn heading_split_tail_is_paragraph() {
        let mut doc = Document { title: "t".into(), blocks: vec![Block::new(BlockKind::Heading(1), "Title")] };
        doc.split_block(Pos::new(0, 5));
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn toggle_mark_adds_then_removes() {
        let mut doc = Document::from_plain_text("t", "one two three");
        doc.toggle_mark(Pos::new(0, 4), Pos::new(0, 7), Marks::BOLD);
        let spans = &doc.blocks[0].spans;
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[1].text, "two");
        assert!(spans[1].marks.contains(Marks::BOLD));

        doc.toggle_mark(Pos::new(0, 7), Pos::new(0, 4), Marks::BOLD);
        assert_eq!(doc.blocks[0].spans.len(), 1, "spans merge back after unbolding");
    }

    #[test]
    fn toggle_mark_partially_marked_range_marks_all() {
        let mut doc = Document::from_plain_text("t", "abcdef");
        doc.toggle_mark(Pos::new(0, 0), Pos::new(0, 3), Marks::ITALIC);
        doc.toggle_mark(Pos::new(0, 0), Pos::new(0, 6), Marks::ITALIC);
        assert_eq!(doc.blocks[0].spans.len(), 1);
        assert!(doc.blocks[0].spans[0].marks.contains(Marks::ITALIC));
    }

    #[test]
    fn typing_inherits_marks_from_previous_char() {
        let mut doc = Document::from_plain_text("t", "bold");
        doc.toggle_mark(Pos::new(0, 0), Pos::new(0, 4), Marks::BOLD);
        doc.insert_text(Pos::new(0, 4), "er");
        assert_eq!(doc.blocks[0].spans.len(), 1);
        assert_eq!(doc.blocks[0].text(), "bolder");
    }

    #[test]
    fn grapheme_movement() {
        let doc = Document::from_plain_text("t", "e\u{301}👍🏽x");
        let after_accent = doc.next_pos(Pos::new(0, 0));
        assert_eq!(after_accent.offset, "e\u{301}".len());
        let after_thumb = doc.next_pos(after_accent);
        assert_eq!(doc.prev_pos(after_thumb), after_accent);
    }

    #[test]
    fn movement_crosses_blocks() {
        let doc = Document::from_plain_text("t", "ab\ncd");
        assert_eq!(doc.next_pos(Pos::new(0, 2)), Pos::new(1, 0));
        assert_eq!(doc.prev_pos(Pos::new(1, 0)), Pos::new(0, 2));
    }

    #[test]
    fn markdown_export() {
        let mut doc = Document {
            title: "t".into(),
            blocks: vec![
                Block::new(BlockKind::Heading(1), "Title"),
                Block::new(BlockKind::Paragraph, "Some bold words *literally*"),
                Block::new(BlockKind::Bullet, "a"),
                Block::new(BlockKind::Bullet, "b"),
                Block::new(BlockKind::Numbered, "one"),
                Block::new(BlockKind::Numbered, "two"),
                Block::new(BlockKind::Quote, "q"),
            ],
        };
        doc.toggle_mark(Pos::new(1, 4), Pos::new(1, 10), Marks::BOLD);
        doc.toggle_mark(Pos::new(1, 10), Pos::new(1, 15), Marks::ITALIC);
        assert_eq!(
            doc.to_markdown(),
            "# Title\n\nSome **bold** _words_ \\*literally\\*\n\n- a\n- b\n1. one\n2. two\n\n> q\n"
        );
    }

    #[test]
    fn flat_offsets_round_trip() {
        let doc = Document::from_plain_text("t", "ab\n\ncde");
        for (pos, flat) in [(Pos::new(0, 2), 2), (Pos::new(1, 0), 3), (Pos::new(2, 1), 5), (Pos::new(2, 3), 7)] {
            assert_eq!(doc.flat_offset(pos), flat);
            assert_eq!(doc.pos_at_flat(flat), pos);
        }
        assert_eq!(doc.pos_at_flat(99), doc.end());
    }

    #[test]
    fn word_count_and_serde_round_trip() {
        let mut doc = Document::from_plain_text("t", "It's a dark-and-stormy night.\nTwo words");
        doc.toggle_mark(Pos::new(0, 0), Pos::new(0, 4), Marks::BOLD);
        assert_eq!(doc.word_count(), 8, "UAX #29 splits hyphenated words");
        let json = serde_json::to_string(&doc).unwrap();
        assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), doc);
    }
}
