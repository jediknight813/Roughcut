//! Word documents, including exports from other word processors.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use quill_doc::{BlockKind, Marks};

use crate::builder::Builder;
use crate::xml::{self, Element};
use crate::{Error, Imported};

pub fn import(title: &str, bytes: &[u8]) -> Result<Imported, Error> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut read = |name: &str| -> Result<Option<String>, Error> {
        match zip.by_name(name) {
            Ok(mut file) => {
                let mut s = String::new();
                file.read_to_string(&mut s)?;
                Ok(Some(s))
            }
            Err(zip::result::ZipError::FileNotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    };
    let body = read("word/document.xml")?.ok_or_else(|| Error::Invalid("no word/document.xml".into()))?;
    let styles = read("word/styles.xml")?.map(|s| xml::parse(&s)).transpose()?;
    let numbering = read("word/numbering.xml")?.map(|s| xml::parse(&s)).transpose()?;
    let comments = read("word/comments.xml")?.map(|s| xml::parse(&s)).transpose()?;

    let ctx = Context {
        style_kinds: styles.as_ref().map(style_kinds).unwrap_or_default(),
        numbered: numbering.as_ref().map(numbered_lists).unwrap_or_default(),
    };
    let root = xml::parse(&body)?;
    let mut paragraphs = Vec::new();
    root.find_all("p", &mut paragraphs);
    let mut b = Builder::default();
    for p in paragraphs {
        b.start(ctx.paragraph_kind(p));
        runs(p, &mut b);
        b.end();
    }

    let notes = comments.map(|c| comment_notes(&c)).unwrap_or_default();
    Ok(Imported { doc: b.finish(title), notes })
}

struct Context {
    /// Paragraph style id -> block kind, from styles.xml names.
    style_kinds: HashMap<String, BlockKind>,
    /// numId -> whether level 0 is numbered (vs. bulleted).
    numbered: HashMap<String, bool>,
}

impl Context {
    fn paragraph_kind(&self, p: &Element) -> BlockKind {
        let Some(ppr) = p.child("pPr") else { return BlockKind::Paragraph };
        if let Some(num) = ppr.child("numPr")
            && let Some(id) = num.child("numId").and_then(|n| n.attr("val"))
            && id != "0"
        {
            return if self.numbered.get(id).copied().unwrap_or(false) {
                BlockKind::Numbered
            } else {
                BlockKind::Bullet
            };
        }
        let style = ppr.child("pStyle").and_then(|s| s.attr("val")).unwrap_or("");
        self.style_kinds.get(style).copied().unwrap_or_else(|| kind_for_style_name(style))
    }
}

fn kind_for_style_name(name: &str) -> BlockKind {
    let name = name.to_ascii_lowercase().replace(' ', "");
    match name.as_str() {
        "title" | "heading1" => BlockKind::Heading(1),
        n if n.starts_with("heading") => BlockKind::Heading(2),
        "subtitle" => BlockKind::Heading(2),
        "quote" | "intensequote" => BlockKind::Quote,
        "listbullet" | "listparagraph" => BlockKind::Bullet,
        "listnumber" => BlockKind::Numbered,
        _ => BlockKind::Paragraph,
    }
}

fn style_kinds(styles: &Element) -> HashMap<String, BlockKind> {
    let mut all = Vec::new();
    styles.find_all("style", &mut all);
    all.into_iter()
        .filter_map(|s| {
            let id = s.attr("styleId")?;
            let name = s.child("name").and_then(|n| n.attr("val")).unwrap_or(id);
            Some((id.to_owned(), kind_for_style_name(name)))
        })
        .collect()
}

fn numbered_lists(numbering: &Element) -> HashMap<String, bool> {
    let mut abstracts = Vec::new();
    numbering.find_all("abstractNum", &mut abstracts);
    let abstract_numbered: HashMap<&str, bool> = abstracts
        .iter()
        .filter_map(|a| {
            let id = a.attr("abstractNumId")?;
            let level0 = a.elements().find(|l| l.name == "lvl" && l.attr("ilvl") == Some("0"))?;
            let fmt = level0.child("numFmt").and_then(|f| f.attr("val")).unwrap_or("bullet");
            Some((id, fmt != "bullet" && fmt != "none"))
        })
        .collect();
    let mut nums = Vec::new();
    numbering.find_all("num", &mut nums);
    nums.iter()
        .filter_map(|n| {
            let id = n.attr("numId")?;
            let abs = n.child("abstractNumId")?.attr("val")?;
            Some((id.to_owned(), abstract_numbered.get(abs).copied().unwrap_or(false)))
        })
        .collect()
}

/// Walks a paragraph's runs, descending through hyperlinks, insertions and
/// content controls but skipping deleted text and nested paragraphs.
fn runs(el: &Element, b: &mut Builder) {
    for child in el.elements() {
        match child.name.as_str() {
            "r" => run(child, b),
            "hyperlink" | "ins" | "smartTag" | "sdt" | "sdtContent" | "fldSimple" | "customXml" => runs(child, b),
            _ => {}
        }
    }
}

fn run(r: &Element, b: &mut Builder) {
    let marks = r.child("rPr").map(run_marks).unwrap_or_default();
    for part in r.elements() {
        match part.name.as_str() {
            "t" => b.text(&part.text(), marks),
            "tab" | "br" | "cr" => b.text(" ", marks),
            "noBreakHyphen" => b.text("-", marks),
            _ => {}
        }
    }
}

fn run_marks(rpr: &Element) -> Marks {
    let on =
        |name: &str| rpr.child(name).is_some_and(|e| !matches!(e.attr("val"), Some("0" | "false" | "none" | "off")));
    let mut marks = Marks::NONE;
    if on("b") {
        marks = marks.with(Marks::BOLD);
    }
    if on("i") {
        marks = marks.with(Marks::ITALIC);
    }
    if on("u") {
        marks = marks.with(Marks::UNDERLINE);
    }
    if on("strike") || on("dstrike") {
        marks = marks.with(Marks::STRIKE);
    }
    let shaded = rpr
        .child("shd")
        .and_then(|s| s.attr("fill"))
        .is_some_and(|fill| !matches!(fill.to_ascii_lowercase().as_str(), "auto" | "ffffff" | ""));
    if on("highlight") || shaded {
        marks = marks.with(Marks::HIGHLIGHT);
    }
    marks
}

fn comment_notes(comments: &Element) -> Vec<String> {
    let mut all = Vec::new();
    comments.find_all("comment", &mut all);
    all.into_iter()
        .filter_map(|c| {
            let mut paras = Vec::new();
            c.find_all("p", &mut paras);
            let text = paras.iter().map(|p| p.text()).collect::<Vec<_>>().join("\n");
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            Some(match c.attr("author").filter(|a| !a.is_empty()) {
                Some(author) => format!("{author}: {text}"),
                None => text.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write;

    use super::*;

    pub fn docx(files: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    #[test]
    fn exported_document_with_styles_and_comments() {
        let body = format!(
            r#"<w:document {W}><w:body>
            <w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Winter Orchard</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">Nobody </w:t></w:r><w:r><w:rPr><w:b w:val="0"/><w:i/></w:rPr><w:t>picked</w:t></w:r><w:r><w:t xml:space="preserve"> the apples.</w:t></w:r></w:p>
            <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>bullet</w:t></w:r></w:p>
            <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>numbered</w:t></w:r></w:p>
            <w:p><w:r><w:t>kept</w:t></w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:hyperlink><w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t> link</w:t></w:r></w:hyperlink></w:p>
            </w:body></w:document>"#
        );
        let numbering = format!(
            r#"<w:numbering {W}>
            <w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
            <w:abstractNum w:abstractNumId="11"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>
            <w:num w:numId="2"><w:abstractNumId w:val="11"/></w:num>
            </w:numbering>"#
        );
        let comments = format!(
            r#"<w:comments {W}><w:comment w:id="0" w:author="Maya"><w:p><w:r><w:t>Plant this earlier?</w:t></w:r></w:p></w:comment></w:comments>"#
        );
        let bytes =
            docx(&[("word/document.xml", &body), ("word/numbering.xml", &numbering), ("word/comments.xml", &comments)]);
        let imported = import("Winter Orchard", &bytes).unwrap();
        let doc = &imported.doc;
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [BlockKind::Heading(1), BlockKind::Paragraph, BlockKind::Bullet, BlockKind::Numbered, BlockKind::Paragraph]
        );
        assert_eq!(doc.blocks[1].text(), "Nobody picked the apples.");
        assert_eq!(doc.blocks[1].spans[0].marks, Marks::BOLD);
        assert_eq!(doc.blocks[1].spans[1].marks, Marks::ITALIC);
        assert_eq!(doc.blocks[4].text(), "kept link");
        assert!(doc.blocks[4].spans[1].marks.contains(Marks::UNDERLINE));
        assert_eq!(imported.notes, ["Maya: Plant this earlier?"]);
    }

    #[test]
    fn missing_body_is_an_error() {
        assert!(import("x", &docx(&[("other.xml", "<a/>")])).is_err());
        assert!(import("x", b"not a zip").is_err());
    }
}
