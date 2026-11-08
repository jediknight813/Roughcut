//! Rich Text Format: enough of the spec for documents from TextEdit, Word
//! and older word processors — paragraphs, headings by outline level, list
//! paragraphs, character formatting, and Unicode/code-page text.

use quill_doc::{BlockKind, Marks};

use crate::builder::Builder;
use crate::{Error, Imported};

/// Destinations whose content is not document text.
const SKIP: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "fldinst",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "xmlnstbl",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "mmathPr",
    "listtext",
    "pntext",
    "pntxta",
    "pntxtb",
    "object",
    "filetbl",
    "revtbl",
    "expandedcolortbl",
    "footnote",
    "annotation",
    "atnid",
    "atnauthor",
    "bkmkstart",
    "bkmkend",
];

#[derive(Clone, Copy)]
struct State {
    marks: Marks,
    skip: bool,
    /// Characters to skip after a \u escape.
    uc: usize,
}

pub fn import(title: &str, bytes: &[u8]) -> Result<Imported, Error> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(Error::Invalid("not an RTF file".into()));
    }
    let mut b = Builder::default();
    let mut stack: Vec<State> = Vec::new();
    let mut st = State { marks: Marks::NONE, skip: false, uc: 1 };
    let mut kind = BlockKind::Paragraph;
    let mut pending_skip = 0usize;
    let mut text = String::new();
    let mut i = 0;

    let flush = |b: &mut Builder, text: &mut String, marks: Marks, kind: BlockKind| {
        if !text.is_empty() {
            if b.current_is_empty() {
                b.set_kind(kind);
            }
            b.text(text, marks);
            text.clear();
        }
    };

    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'{' => {
                flush(&mut b, &mut text, st.marks, kind);
                stack.push(st);
                i += 1;
            }
            b'}' => {
                flush(&mut b, &mut text, st.marks, kind);
                st = stack.pop().unwrap_or(st);
                i += 1;
            }
            b'\\' => {
                i += 1;
                let Some(&next) = bytes.get(i) else { break };
                if next.is_ascii_alphabetic() {
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                    let num_start = i;
                    if i < bytes.len() && bytes[i] == b'-' {
                        i += 1;
                    }
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    let param: Option<i32> =
                        std::str::from_utf8(&bytes[num_start..i]).ok().and_then(|s| s.parse().ok());
                    if i < bytes.len() && bytes[i] == b' ' {
                        i += 1;
                    }
                    if st.skip {
                        continue;
                    }
                    if SKIP.contains(&word) {
                        flush(&mut b, &mut text, st.marks, kind);
                        st.skip = true;
                        if word == "listtext" || word == "pntext" {
                            kind = BlockKind::Bullet;
                        }
                        continue;
                    }
                    if pending_skip > 0 && word != "u" {
                        pending_skip -= 1;
                        continue;
                    }
                    let on = param != Some(0);
                    let set = |mark: Marks, on: bool, st: &mut State, text: &mut String, b: &mut Builder| {
                        flush(b, text, st.marks, kind);
                        st.marks = if on { st.marks.with(mark) } else { st.marks.without(mark) };
                    };
                    match word {
                        "par" | "sect" | "page" => {
                            flush(&mut b, &mut text, st.marks, kind);
                            if b.current_is_empty() {
                                b.set_kind(kind);
                            }
                            b.end();
                            b.start(BlockKind::Paragraph);
                        }
                        "pard" => kind = BlockKind::Paragraph,
                        "outlinelevel" => kind = BlockKind::Heading(if param.unwrap_or(0) == 0 { 1 } else { 2 }),
                        "ls" | "ilvl" => {
                            if kind == BlockKind::Paragraph {
                                kind = BlockKind::Bullet;
                            }
                        }
                        "line" | "tab" | "cell" => text.push(' '),
                        "b" => set(Marks::BOLD, on, &mut st, &mut text, &mut b),
                        "i" => set(Marks::ITALIC, on, &mut st, &mut text, &mut b),
                        "ul" => set(Marks::UNDERLINE, on, &mut st, &mut text, &mut b),
                        "ulnone" => set(Marks::UNDERLINE, false, &mut st, &mut text, &mut b),
                        "strike" | "striked" => set(Marks::STRIKE, on, &mut st, &mut text, &mut b),
                        "highlight" => set(Marks::HIGHLIGHT, on, &mut st, &mut text, &mut b),
                        "plain" => {
                            flush(&mut b, &mut text, st.marks, kind);
                            st.marks = Marks::NONE;
                        }
                        "uc" => st.uc = param.unwrap_or(1).max(0) as usize,
                        "u" => {
                            if let Some(n) = param {
                                let code = if n < 0 { n + 65536 } else { n } as u32;
                                if let Some(ch) = char::from_u32(code) {
                                    text.push(ch);
                                }
                                pending_skip = st.uc;
                            }
                        }
                        "emdash" => text.push('—'),
                        "endash" => text.push('–'),
                        "bullet" => text.push('•'),
                        "lquote" => text.push('‘'),
                        "rquote" => text.push('’'),
                        "ldblquote" => text.push('“'),
                        "rdblquote" => text.push('”'),
                        _ => {}
                    }
                } else {
                    i += 1;
                    if st.skip && next != b'\'' {
                        continue;
                    }
                    match next {
                        b'*' => {
                            flush(&mut b, &mut text, st.marks, kind);
                            st.skip = true;
                        }
                        b'\'' => {
                            let hex = bytes.get(i..i + 2).and_then(|h| std::str::from_utf8(h).ok());
                            i += 2;
                            if st.skip {
                                continue;
                            }
                            if pending_skip > 0 {
                                pending_skip -= 1;
                                continue;
                            }
                            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                                text.push_str(&encoding_rs::WINDOWS_1252.decode_without_bom_handling(&[byte]).0);
                            }
                        }
                        b'~' => text.push('\u{a0}'),
                        b'_' => text.push('-'),
                        b'-' => {}
                        b'\n' | b'\r' => {
                            flush(&mut b, &mut text, st.marks, kind);
                            b.end();
                            b.start(BlockKind::Paragraph);
                        }
                        other => text.push(other as char),
                    }
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                i += 1;
                if st.skip {
                    continue;
                }
                if pending_skip > 0 {
                    pending_skip -= 1;
                    continue;
                }
                // Raw bytes above ASCII are in the document's code page.
                if c.is_ascii() {
                    text.push(c as char);
                } else {
                    text.push_str(&encoding_rs::WINDOWS_1252.decode_without_bom_handling(&[c]).0);
                }
            }
        }
    }
    flush(&mut b, &mut text, st.marks, kind);
    Ok(Imported { doc: b.finish(title), notes: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textedit_style_rtf() {
        let src = concat!(
            "{\\rtf1\\ansi\\ansicpg1252\\cocoartf2761\n",
            "{\\fonttbl\\f0\\fswiss\\fcharset0 Helvetica;}\n",
            "{\\colortbl;\\red255\\green255\\blue255;}\n",
            "\\pard\\outlinelevel0\\f0\\b\\fs36 Notes 2019\\b0\\fs24 \\par\n",
            "\\pard Caf\\'e9 and \\i tea\\i0 \\u8212?ok\\par\n",
            "{\\listtext\\'95 }\\pard\\ls1 first\\par\n",
            "\\pard \\ul under\\ulnone  done}"
        )
        .as_bytes();
        let doc = import("Notes", src).unwrap().doc;
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [BlockKind::Heading(1), BlockKind::Paragraph, BlockKind::Bullet, BlockKind::Paragraph]);
        assert_eq!(doc.blocks[0].text(), "Notes 2019");
        assert_eq!(doc.blocks[1].text(), "Café and tea—ok");
        assert!(doc.blocks[1].spans.iter().any(|s| s.text == "tea" && s.marks == Marks::ITALIC));
        assert_eq!(doc.blocks[2].text(), "first");
        assert!(doc.blocks[3].spans[0].marks.contains(Marks::UNDERLINE));
        assert_eq!(doc.blocks[3].text(), "under done");
    }

    #[test]
    fn rejects_non_rtf() {
        assert!(import("x", b"hello").is_err());
    }
}
