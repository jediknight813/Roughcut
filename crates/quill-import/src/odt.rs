//! OpenDocument text (LibreOffice, Pages export, older Google Docs exports).

use std::collections::HashMap;
use std::io::{Cursor, Read};

use quill_doc::{BlockKind, Marks};

use crate::builder::Builder;
use crate::xml::{self, Element, Node};
use crate::{Error, Imported};

pub fn import(title: &str, bytes: &[u8]) -> Result<Imported, Error> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut content = String::new();
    zip.by_name("content.xml")?.read_to_string(&mut content)?;
    let root = xml::parse(&content)?;

    let mut styles = Styles::default();
    let mut automatic = Vec::new();
    root.find_all("automatic-styles", &mut automatic);
    for block in automatic {
        styles.read(block);
    }

    let mut texts = Vec::new();
    root.find_all("text", &mut texts);
    let body = texts.into_iter().find(|t| t.name == "text").ok_or_else(|| Error::Invalid("no office:text".into()))?;

    let mut walker = Walker { styles, b: Builder::default(), notes: Vec::new() };
    walker.blocks(body, None);
    Ok(Imported { doc: walker.b.finish(title), notes: walker.notes })
}

#[derive(Default)]
struct Styles {
    marks: HashMap<String, Marks>,
    /// Paragraph style -> kind, via its parent (e.g. "Title", "Heading_20_1").
    kinds: HashMap<String, BlockKind>,
    numbered_lists: HashMap<String, bool>,
}

impl Styles {
    fn read(&mut self, block: &Element) {
        for style in block.elements() {
            let Some(name) = style.attr("name") else { continue };
            match style.name.as_str() {
                "style" => {
                    if let Some(props) = style.child("text-properties") {
                        self.marks.insert(name.to_owned(), text_marks(props));
                    }
                    if let Some(parent) = style.attr("parent-style-name") {
                        self.kinds.insert(name.to_owned(), kind_for(parent));
                    }
                }
                "list-style" => {
                    let numbered = style.elements().next().is_some_and(|l| l.name == "list-level-style-number");
                    self.numbered_lists.insert(name.to_owned(), numbered);
                }
                _ => {}
            }
        }
    }

    fn paragraph_kind(&self, style: Option<&str>) -> BlockKind {
        let Some(style) = style else { return BlockKind::Paragraph };
        self.kinds.get(style).copied().unwrap_or_else(|| kind_for(style))
    }
}

fn kind_for(style: &str) -> BlockKind {
    match style.to_ascii_lowercase().replace("_20_", "").as_str() {
        "title" | "heading1" => BlockKind::Heading(1),
        s if s.starts_with("heading") || s == "subtitle" => BlockKind::Heading(2),
        "quotations" | "quote" => BlockKind::Quote,
        _ => BlockKind::Paragraph,
    }
}

fn text_marks(props: &Element) -> Marks {
    let mut marks = Marks::NONE;
    if props.attr("font-weight").is_some_and(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)) {
        marks = marks.with(Marks::BOLD);
    }
    if props.attr("font-style") == Some("italic") {
        marks = marks.with(Marks::ITALIC);
    }
    if props.attr("text-underline-style").is_some_and(|s| s != "none") {
        marks = marks.with(Marks::UNDERLINE);
    }
    if props.attr("text-line-through-style").is_some_and(|s| s != "none") {
        marks = marks.with(Marks::STRIKE);
    }
    if props.attr("background-color").is_some_and(|c| c != "transparent" && c != "#ffffff") {
        marks = marks.with(Marks::HIGHLIGHT);
    }
    marks
}

struct Walker {
    styles: Styles,
    b: Builder,
    notes: Vec<String>,
}

impl Walker {
    /// Block-level content: paragraphs, headings, lists, tables, sections.
    fn blocks(&mut self, el: &Element, list: Option<BlockKind>) {
        for child in el.elements() {
            match child.name.as_str() {
                "h" => {
                    let level = child.attr("outline-level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(1);
                    self.b.start(BlockKind::Heading(if level <= 1 { 1 } else { 2 }));
                    self.inline(child, Marks::NONE);
                    self.b.end();
                }
                "p" => {
                    let kind = list.unwrap_or_else(|| self.styles.paragraph_kind(child.attr("style-name")));
                    self.b.start(kind);
                    self.inline(child, Marks::NONE);
                    self.b.end();
                }
                "list" => {
                    let numbered = child.attr("style-name").and_then(|s| self.styles.numbered_lists.get(s)).copied();
                    let kind = match (numbered, list) {
                        (Some(true), _) => BlockKind::Numbered,
                        (Some(false), _) => BlockKind::Bullet,
                        (None, Some(outer)) => outer,
                        (None, None) => BlockKind::Bullet,
                    };
                    self.blocks(child, Some(kind));
                }
                "list-item" | "list-header" | "section" | "table" | "table-row" | "table-cell"
                | "table-header-rows" => self.blocks(child, list),
                _ => {}
            }
        }
    }

    fn inline(&mut self, el: &Element, marks: Marks) {
        for child in &el.children {
            match child {
                Node::Text(t) => self.b.text(t, marks),
                Node::Element(e) => match e.name.as_str() {
                    "span" | "a" => {
                        let m =
                            e.attr("style-name").and_then(|s| self.styles.marks.get(s)).copied().unwrap_or_default();
                        self.inline(e, marks.with(m));
                    }
                    "s" => {
                        let n = e.attr("c").and_then(|c| c.parse().ok()).unwrap_or(1);
                        self.b.text(&" ".repeat(n), marks);
                    }
                    "tab" | "line-break" => self.b.text(" ", marks),
                    "annotation" => {
                        let mut paras = Vec::new();
                        e.find_all("p", &mut paras);
                        let text = paras.iter().map(|p| p.text()).collect::<Vec<_>>().join("\n");
                        let author = e.child("creator").map(|c| c.text()).filter(|a| !a.is_empty());
                        if !text.trim().is_empty() {
                            self.notes.push(match author {
                                Some(a) => format!("{a}: {}", text.trim()),
                                None => text.trim().to_owned(),
                            });
                        }
                    }
                    "note" | "annotation-end" | "bookmark" | "bookmark-start" | "bookmark-end" => {}
                    _ => self.inline(e, marks),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::tests::docx as zip_of;

    #[test]
    fn libreoffice_document() {
        let content = r#"<office:document-content xmlns:office="o" xmlns:style="s" xmlns:text="t" xmlns:fo="f" xmlns:dc="d">
          <office:automatic-styles>
            <style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
            <style:style style:name="P1" style:family="paragraph" style:parent-style-name="Title"/>
            <text:list-style style:name="L1"><text:list-level-style-number text:level="1"/></text:list-style>
          </office:automatic-styles>
          <office:body><office:text>
            <text:p text:style-name="P1">On Quiet</text:p>
            <text:h text:outline-level="2">Part one</text:h>
            <text:p>There is a <text:span text:style-name="T1">kind</text:span><text:s text:c="2"/>of silence.<office:annotation><dc:creator>Theo</dc:creator><text:p>Nice line</text:p></office:annotation></text:p>
            <text:list text:style-name="L1"><text:list-item><text:p>first</text:p></text:list-item></text:list>
            <text:list><text:list-item><text:p>dot</text:p></text:list-item></text:list>
          </office:text></office:body></office:document-content>"#;
        let imported = import("On Quiet", &zip_of(&[("content.xml", content)])).unwrap();
        let doc = &imported.doc;
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                BlockKind::Heading(1),
                BlockKind::Heading(2),
                BlockKind::Paragraph,
                BlockKind::Numbered,
                BlockKind::Bullet
            ]
        );
        assert_eq!(doc.blocks[2].text(), "There is a kind  of silence.");
        assert!(doc.blocks[2].spans[1].marks.contains(Marks::BOLD));
        assert_eq!(imported.notes, ["Theo: Nice line"]);
    }
}
