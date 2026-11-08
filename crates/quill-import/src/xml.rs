//! A tiny XML DOM over quick-xml, enough for walking DOCX and ODT bodies.
//! Names are kept as local names (namespace prefixes dropped).

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::Error;

#[derive(Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    /// All descendants named `name`, depth-first, not descending into matches.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for e in self.elements() {
            if e.name == name {
                out.push(e);
            } else {
                e.find_all(name, out);
            }
        }
    }

    /// Concatenated text of every descendant text node.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for c in &self.children {
            match c {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => e.collect_text(out),
            }
        }
    }
}

pub fn parse(xml: &str) -> Result<Element, Error> {
    let mut reader = Reader::from_str(xml);
    let mut stack = vec![Element::default()];
    loop {
        let event = reader.read_event().map_err(|e| Error::Xml(e.to_string()))?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                let mut attrs = Vec::new();
                for attr in start.attributes().flatten() {
                    let key = attr.key.local_name().as_ref().to_owned();
                    let value = attr.normalized_value(Default::default()).map(|v| v.into_owned()).unwrap_or_default();
                    attrs.push((key, value));
                }
                let element = Element { name: start.local_name().as_ref().to_owned(), attrs, children: Vec::new() };
                if empty {
                    push(&mut stack, Node::Element(element));
                } else {
                    stack.push(element);
                }
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    let done = stack.pop().expect("non-empty");
                    push(&mut stack, Node::Element(done));
                }
            }
            Event::Text(text) => push_text(&mut stack, &text.into_inner()),
            Event::CData(data) => push_text(&mut stack, &data.into_inner()),
            Event::GeneralRef(r) => {
                let ch = match r.resolve_char_ref() {
                    Ok(Some(ch)) => Some(ch),
                    _ => match r.into_inner().as_ref() {
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "amp" => Some('&'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                };
                if let Some(ch) = ch {
                    push_text(&mut stack, ch.encode_utf8(&mut [0; 4]));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    while stack.len() > 1 {
        let done = stack.pop().expect("non-empty");
        push(&mut stack, Node::Element(done));
    }
    Ok(stack.pop().expect("root"))
}

fn push(stack: &mut [Element], node: Node) {
    stack.last_mut().expect("root").children.push(node);
}

fn push_text(stack: &mut [Element], text: &str) {
    let top = stack.last_mut().expect("root");
    if let Some(Node::Text(prev)) = top.children.last_mut() {
        prev.push_str(text);
    } else {
        top.children.push(Node::Text(text.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_names_attrs_text_and_entities() {
        let root = super::parse(r#"<w:doc xmlns:w="x"><w:p w:val="1">a &amp; b&#x21;<w:br/></w:p></w:doc>"#).unwrap();
        let doc = root.child("doc").unwrap();
        let p = doc.child("p").unwrap();
        assert_eq!(p.attr("val"), Some("1"));
        assert_eq!(p.text(), "a & b!");
        assert!(p.child("br").is_some());
    }
}
