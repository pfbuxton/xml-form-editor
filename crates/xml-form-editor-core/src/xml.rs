//! Parsing, element paths, and reading a start tag straight from the source text.

use std::fmt;
use std::ops::Range;

use roxmltree::{Document, Node, ParsingOptions};

/// The namespace of `xsi:nil`, `xsi:noNamespaceSchemaLocation` and `xsi:schemaLocation`.
pub(crate) const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";

pub(crate) fn parse(text: &str) -> Result<Document<'_>, XmlError> {
    let options = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, options).map_err(XmlError::from)
}

/// Why a document couldn't be parsed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct XmlError {
    pub message: String,
    /// 1-based
    pub line: u32,
    /// 1-based
    pub column: u32,
}

impl From<roxmltree::Error> for XmlError {
    fn from(error: roxmltree::Error) -> Self {
        let pos = error.pos();
        XmlError {
            message: error.to_string(),
            line: pos.row,
            column: pos.col,
        }
    }
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for XmlError {}

/// One step of a [`Path`]: an element's local name, and how many earlier siblings have that name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Step {
    pub name: String,
    pub index: usize,
}

/// Where an element is in a document, like an XPath: `/gsfit_code_settings/grid/n_r`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Path(Vec<Step>);

impl Path {
    pub fn root(name: &str) -> Path {
        Path(vec![Step {
            name: name.to_string(),
            index: 0,
        }])
    }

    pub fn child(&self, name: &str, index: usize) -> Path {
        let mut steps = self.0.clone();
        steps.push(Step {
            name: name.to_string(),
            index,
        });
        Path(steps)
    }

    pub fn steps(&self) -> &[Step] {
        &self.0
    }

    /// The element's local name.
    pub fn name(&self) -> &str {
        self.0.last().map_or("", |step| step.name.as_str())
    }

    pub(crate) fn find<'a, 'input>(&self, doc: &'a Document<'input>) -> Option<Node<'a, 'input>> {
        let (first, rest) = self.0.split_first()?;
        let root = doc.root_element();
        if root.tag_name().name() != first.name || first.index != 0 {
            return None;
        }
        rest.iter().try_fold(root, |node, step| {
            node.children()
                .filter(|c| c.is_element() && c.tag_name().name() == step.name)
                .nth(step.index)
        })
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for step in &self.0 {
            write!(f, "/{}", step.name)?;
            if step.index > 0 {
                // 1-based, as in XPath
                write!(f, "[{}]", step.index + 1)?;
            }
        }
        Ok(())
    }
}

/// The schema a document names with `xsi:noNamespaceSchemaLocation` or `xsi:schemaLocation` on
/// its root element, as written there (usually a path relative to the document).
pub fn schema_location(xml: &str) -> Result<Option<String>, XmlError> {
    Ok(schema_location_of(&parse(xml)?))
}

pub(crate) fn schema_location_of(doc: &Document) -> Option<String> {
    let root = doc.root_element();
    if let Some(location) = root
        .attribute((XSI_NS, "noNamespaceSchemaLocation"))
        .map(str::trim)
        && !location.is_empty()
    {
        return Some(location.to_string());
    }
    // Pairs of namespace and location: prefer the pair for the root element's namespace
    let words: Vec<&str> = root
        .attribute((XSI_NS, "schemaLocation"))?
        .split_whitespace()
        .collect();
    let namespace = root.tag_name().namespace();
    let (pairs, _) = words.as_chunks::<2>();
    let [_, location] = pairs
        .iter()
        .find(|[pair_namespace, _]| Some(*pair_namespace) == namespace)
        .or(pairs.first())?;
    Some(location.to_string())
}

/// An attribute as written in a start tag, as byte ranges into the source.
pub(crate) struct RawAttribute {
    /// Where the whitespace before the attribute starts
    pub lead: usize,
    pub name: Range<usize>,
    /// The value, without its quotes
    pub value: Range<usize>,
    pub quote: u8,
    /// Just after the closing quote
    pub end: usize,
}

/// A start tag as written, as byte ranges into the source.
pub(crate) struct StartTag {
    pub name: Range<usize>,
    pub attributes: Vec<RawAttribute>,
    /// The end of the last attribute, or of the name: where the whitespace and `>` or `/>` begin
    pub tail: usize,
    /// Just after the `>`
    pub end: usize,
    pub self_closing: bool,
}

/// Reads the start tag that begins at `start`, the `<` of an element in a well-formed document.
pub(crate) fn scan_start_tag(src: &str, start: usize) -> Option<StartTag> {
    let b = src.as_bytes();
    if b.get(start) != Some(&b'<') {
        return None;
    }
    let skip_space = |mut i: usize| {
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        i
    };
    let mut i = start + 1;
    while i < b.len() && !is_space(b[i]) && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let name = start + 1..i;
    let mut attributes = Vec::new();
    let mut tail = i;
    loop {
        let lead = i;
        i = skip_space(i);
        match *b.get(i)? {
            b'/' => {
                let closed = b.get(i + 1) == Some(&b'>');
                return closed.then_some(StartTag {
                    name,
                    attributes,
                    tail,
                    end: i + 2,
                    self_closing: true,
                });
            }
            b'>' => {
                return Some(StartTag {
                    name,
                    attributes,
                    tail,
                    end: i + 1,
                    self_closing: false,
                });
            }
            _ => {
                let name_start = i;
                while i < b.len() && !is_space(b[i]) && b[i] != b'=' {
                    i += 1;
                }
                let attribute_name = name_start..i;
                i = skip_space(i);
                if b.get(i) != Some(&b'=') {
                    return None;
                }
                i = skip_space(i + 1);
                let quote = *b.get(i)?;
                if quote != b'"' && quote != b'\'' {
                    return None;
                }
                let value_start = i + 1;
                let value_end = value_start + src[value_start..].find(quote as char)?;
                i = value_end + 1;
                attributes.push(RawAttribute {
                    lead,
                    name: attribute_name,
                    value: value_start..value_end,
                    quote,
                    end: i,
                });
                tail = i;
            }
        }
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// The bytes between an element's start and end tags; `None` for an empty-element tag, `<x/>`.
pub(crate) fn content_range(src: &str, node: Node, tag: &StartTag) -> Option<Range<usize>> {
    if tag.self_closing {
        return None;
    }
    let range = node.range();
    let close = range.start + src[range].rfind("</")?;
    Some(tag.end..close)
}

/// The part of a qualified name after the prefix.
pub(crate) fn local_name(qname: &str) -> &str {
    qname.split_once(':').map_or(qname, |(_, local)| local)
}

/// The namespace of an attribute named `qname` on `node`; `None` when it has no prefix.
pub(crate) fn attribute_namespace<'a>(node: Node<'a, '_>, qname: &str) -> Option<&'a str> {
    let (prefix, _) = qname.split_once(':')?;
    node.lookup_namespace_uri(Some(prefix))
}

pub(crate) fn is_xsi_nil(src: &str, node: Node, attribute: &RawAttribute) -> bool {
    let qname = &src[attribute.name.clone()];
    local_name(qname) == "nil" && attribute_namespace(node, qname) == Some(XSI_NS)
}

pub(crate) fn is_nil(node: Node) -> bool {
    matches!(
        node.attribute((XSI_NS, "nil")).map(str::trim),
        Some("true" | "1")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_start_tags() {
        let src = r#"<a x="1" y = 'two' >text</a>"#;
        let tag = scan_start_tag(src, 0).unwrap();
        assert_eq!(&src[tag.name.clone()], "a");
        let names: Vec<&str> = tag
            .attributes
            .iter()
            .map(|a| &src[a.name.clone()])
            .collect();
        assert_eq!(names, ["x", "y"]);
        assert_eq!(&src[tag.attributes[1].value.clone()], "two");
        assert_eq!(&src[tag.tail..tag.end], " >");
        assert!(!tag.self_closing);

        let src = r#"<p:b xsi:nil="true"/>"#;
        let tag = scan_start_tag(src, 0).unwrap();
        assert_eq!(&src[tag.name.clone()], "p:b");
        assert_eq!(&src[tag.tail..tag.end], "/>");
        assert!(tag.self_closing);
    }

    #[test]
    fn finds_paths_and_schema_locations() {
        let xml = r#"<r xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="urn:a a.xsd">
            <x>1</x><y/><x>2</x></r>"#;
        let doc = parse(xml).unwrap();
        let second = Path::root("r").child("x", 1);
        assert_eq!(second.to_string(), "/r/x[2]");
        assert_eq!(second.find(&doc).unwrap().text(), Some("2"));
        assert!(Path::root("r").child("x", 2).find(&doc).is_none());
        assert_eq!(schema_location(xml).unwrap().as_deref(), Some("a.xsd"));
    }
}
