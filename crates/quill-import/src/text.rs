//! Plain text, Markdown and HTML.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use quill_doc::{BlockKind, Document, Marks};
use scraper::{ElementRef, Html, Node};

use crate::builder::Builder;

/// Decodes text files: UTF-8 (with or without BOM), UTF-16 with BOM, or
/// Windows-1252 for old files that are neither.
pub fn decode(bytes: &[u8]) -> String {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return encoding.decode_without_bom_handling(&bytes[bom..]).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

pub fn plain(title: &str, bytes: &[u8]) -> Document {
    let text = decode(bytes).replace("\r\n", "\n");
    let mut b = Builder::default();
    for line in text.split('\n') {
        b.start(BlockKind::Paragraph);
        b.text(line, Marks::NONE);
    }
    b.finish(title)
}

pub fn markdown(title: &str, bytes: &[u8]) -> Document {
    let text = decode(bytes);
    let mut b = Builder::default();
    let mut marks = Marks::NONE;
    let mut lists: Vec<BlockKind> = Vec::new();
    let mut quote = 0usize;
    let paragraph_kind = |quote: usize| if quote > 0 { BlockKind::Quote } else { BlockKind::Paragraph };
    for event in Parser::new_ext(&text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                b.start(BlockKind::Heading(if level == HeadingLevel::H1 { 1 } else { 2 }))
            }
            Event::Start(Tag::Paragraph) => {
                // A loose list item wraps its text in a paragraph: keep the item.
                let in_fresh_item = !lists.is_empty()
                    && b.current_is_empty()
                    && matches!(b.kind(), Some(BlockKind::Bullet | BlockKind::Numbered));
                if !in_fresh_item {
                    b.start(paragraph_kind(quote));
                }
            }
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote = quote.saturating_sub(1),
            Event::Start(Tag::List(first)) => {
                lists.push(if first.is_some() { BlockKind::Numbered } else { BlockKind::Bullet })
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                b.end();
            }
            Event::Start(Tag::Item) => b.start(*lists.last().unwrap_or(&BlockKind::Bullet)),
            Event::Start(Tag::CodeBlock(_)) | Event::Start(Tag::TableRow) | Event::Start(Tag::TableHead) => {
                b.start(BlockKind::Paragraph)
            }
            Event::End(TagEnd::TableCell) => b.text("  ", marks),
            Event::Start(Tag::Strong) => marks = marks.with(Marks::BOLD),
            Event::End(TagEnd::Strong) => marks = marks.without(Marks::BOLD),
            Event::Start(Tag::Emphasis) => marks = marks.with(Marks::ITALIC),
            Event::End(TagEnd::Emphasis) => marks = marks.without(Marks::ITALIC),
            Event::Start(Tag::Strikethrough) => marks = marks.with(Marks::STRIKE),
            Event::End(TagEnd::Strikethrough) => marks = marks.without(Marks::STRIKE),
            Event::End(TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::Item | TagEnd::CodeBlock) => b.end(),
            Event::Text(t) => {
                // Code blocks keep their line structure.
                let mut lines = t.split('\n').peekable();
                while let Some(line) = lines.next() {
                    b.text(line, marks);
                    if lines.peek().is_some_and(|next| !next.is_empty()) {
                        b.start(BlockKind::Paragraph);
                    }
                }
            }
            Event::Code(t) => b.text(&t, marks),
            Event::SoftBreak | Event::HardBreak => b.text(" ", marks),
            _ => {}
        }
    }
    b.finish(title)
}

pub fn html(title: &str, bytes: &[u8]) -> Document {
    let html = Html::parse_document(&decode(bytes));
    let mut b = Builder::default();
    walk_html(html.root_element(), &mut b, Marks::NONE, &mut Vec::new(), false);
    b.finish(title)
}

fn walk_html(el: ElementRef<'_>, b: &mut Builder, marks: Marks, lists: &mut Vec<BlockKind>, quote: bool) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if collapsed.is_empty() {
                    continue;
                }
                let lead = if t.starts_with(char::is_whitespace) && !b.current_is_empty() { " " } else { "" };
                let trail = if t.ends_with(char::is_whitespace) { " " } else { "" };
                b.text(&format!("{lead}{collapsed}{trail}"), marks);
            }
            Node::Element(e) => {
                let Some(child_el) = ElementRef::wrap(child) else { continue };
                let name = e.name();
                let mut m = marks;
                if let Some(style) = e.attr("style") {
                    m = style_marks(style, m);
                }
                match name {
                    "script" | "style" | "head" | "title" | "noscript" => {}
                    "b" | "strong" => walk_html(child_el, b, m.with(Marks::BOLD), lists, quote),
                    "i" | "em" => walk_html(child_el, b, m.with(Marks::ITALIC), lists, quote),
                    "u" | "ins" => walk_html(child_el, b, m.with(Marks::UNDERLINE), lists, quote),
                    "s" | "strike" | "del" => walk_html(child_el, b, m.with(Marks::STRIKE), lists, quote),
                    "mark" => walk_html(child_el, b, m.with(Marks::HIGHLIGHT), lists, quote),
                    "br" => b.text(" ", m),
                    "ul" | "ol" => {
                        lists.push(if name == "ol" { BlockKind::Numbered } else { BlockKind::Bullet });
                        walk_html(child_el, b, m, lists, quote);
                        lists.pop();
                        b.end();
                    }
                    "li" => {
                        b.start(*lists.last().unwrap_or(&BlockKind::Bullet));
                        walk_html(child_el, b, m, lists, quote);
                        b.end();
                    }
                    "blockquote" => walk_html(child_el, b, m, lists, true),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        b.start(BlockKind::Heading(if name == "h1" { 1 } else { 2 }));
                        walk_html(child_el, b, m, lists, quote);
                        b.end();
                    }
                    "p" | "div" | "pre" | "tr" | "section" | "article" => {
                        let in_item = !lists.is_empty() && b.current_is_empty() && name == "p";
                        if !in_item {
                            b.start(if quote { BlockKind::Quote } else { BlockKind::Paragraph });
                        }
                        walk_html(child_el, b, m, lists, quote);
                        if !in_item {
                            b.end();
                        }
                    }
                    _ => walk_html(child_el, b, m, lists, quote),
                }
            }
            _ => {}
        }
    }
}

/// Inline CSS exported by word processors.
fn style_marks(style: &str, mut marks: Marks) -> Marks {
    for decl in style.split(';') {
        let Some((prop, value)) = decl.split_once(':') else { continue };
        let (prop, value) = (prop.trim().to_ascii_lowercase(), value.trim().to_ascii_lowercase());
        match prop.as_str() {
            "font-weight" if value == "bold" || value.parse::<u32>().is_ok_and(|w| w >= 600) => {
                marks = marks.with(Marks::BOLD)
            }
            "font-style" if value == "italic" => marks = marks.with(Marks::ITALIC),
            "text-decoration" | "text-decoration-line" => {
                if value.contains("underline") {
                    marks = marks.with(Marks::UNDERLINE);
                }
                if value.contains("line-through") {
                    marks = marks.with(Marks::STRIKE);
                }
            }
            "background-color" if value != "transparent" && value != "#ffffff" && value != "white" => {
                marks = marks.with(Marks::HIGHLIGHT)
            }
            _ => {}
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(doc: &Document) -> Vec<BlockKind> {
        doc.blocks.iter().map(|b| b.kind).collect()
    }

    #[test]
    fn decodes_legacy_and_utf16_text() {
        assert_eq!(decode(b"caf\xe9"), "café");
        assert_eq!(decode(&[0xff, 0xfe, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode("naïve".as_bytes()), "naïve");
    }

    #[test]
    fn plain_text_lines_become_paragraphs() {
        let doc = plain("t", b"one\r\n\r\ntwo\n");
        assert_eq!(doc.plain_text(), "one\n\ntwo");
    }

    #[test]
    fn markdown_structure_and_marks() {
        let md = "# Title\n\nSome **bold** and *it*.\n\n- a\n- b\n\n1. first\n\n> quoted\n\n## Sub\n";
        let doc = markdown("t", md.as_bytes());
        assert_eq!(
            kinds(&doc),
            [
                BlockKind::Heading(1),
                BlockKind::Paragraph,
                BlockKind::Bullet,
                BlockKind::Bullet,
                BlockKind::Numbered,
                BlockKind::Quote,
                BlockKind::Heading(2)
            ]
        );
        let spans = &doc.blocks[1].spans;
        assert!(spans.iter().any(|s| s.text == "bold" && s.marks.contains(Marks::BOLD)));
        assert!(spans.iter().any(|s| s.text == "it" && s.marks.contains(Marks::ITALIC)));
    }

    #[test]
    fn loose_markdown_list_items_stay_items() {
        let doc = markdown("t", b"- one\n\n- two\n");
        assert_eq!(kinds(&doc), [BlockKind::Bullet, BlockKind::Bullet]);
    }

    #[test]
    fn html_with_inline_styles() {
        let src = r#"<html><head><style>p{}</style></head><body>
            <h1>Story</h1>
            <p><span style="font-weight:700">Bold</span> then <span style="font-style:italic">slanted</span></p>
            <ul><li><p>item</p></li></ul>
            <blockquote><p>q</p></blockquote>
            </body></html>"#;
        let doc = html("t", src.as_bytes());
        assert_eq!(kinds(&doc), [BlockKind::Heading(1), BlockKind::Paragraph, BlockKind::Bullet, BlockKind::Quote]);
        assert_eq!(doc.blocks[1].text(), "Bold then slanted");
        assert!(doc.blocks[1].spans[0].marks.contains(Marks::BOLD));
        assert!(doc.blocks[1].spans[2].marks.contains(Marks::ITALIC));
    }
}
