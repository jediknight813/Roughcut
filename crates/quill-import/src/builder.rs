//! Accumulates blocks while a format's parser walks its source.

use quill_doc::{Block, BlockKind, Document, Marks};

#[derive(Default)]
pub struct Builder {
    blocks: Vec<Block>,
    current: Option<Block>,
}

impl Builder {
    /// Ends the current block (if any) and starts a new one.
    pub fn start(&mut self, kind: BlockKind) {
        self.end();
        self.current = Some(Block { kind, spans: Vec::new() });
    }

    /// Changes the kind of the current block without ending it.
    pub fn set_kind(&mut self, kind: BlockKind) {
        self.current.get_or_insert_with(Block::default).kind = kind;
    }

    pub fn kind(&self) -> Option<BlockKind> {
        self.current.as_ref().map(|b| b.kind)
    }

    pub fn current_is_empty(&self) -> bool {
        self.current.as_ref().is_none_or(Block::is_empty)
    }

    pub fn text(&mut self, text: &str, marks: Marks) {
        let text: String = text.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
        if text.is_empty() {
            return;
        }
        let block = self.current.get_or_insert_with(Block::default);
        let end = block.len();
        block.insert(end, &text, marks);
    }

    /// Ends the current block. Empty paragraphs are kept (they are blank
    /// lines the author typed); empty list items and headings are dropped.
    pub fn end(&mut self) {
        if let Some(block) = self.current.take()
            && (!block.is_empty() || block.kind == BlockKind::Paragraph)
        {
            self.blocks.push(block);
        }
    }

    pub fn finish(mut self, title: &str) -> Document {
        self.end();
        // Leading and trailing blank lines are noise from conversion.
        while self.blocks.last().is_some_and(Block::is_empty) {
            self.blocks.pop();
        }
        let first = self.blocks.iter().position(|b| !b.is_empty()).unwrap_or(self.blocks.len());
        self.blocks.drain(..first);
        if self.blocks.is_empty() {
            self.blocks.push(Block::default());
        }
        Document { title: title.to_owned(), blocks: self.blocks }
    }
}
