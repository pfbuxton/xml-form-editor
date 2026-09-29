//! Turns a change of value into edits of the XML text that touch only what changes, so that
//! formatting and comments survive, undo works as usual, and diffs stay small.

use std::fmt;
use std::ops::Range;

use roxmltree::Node;

use crate::text::{Utf16Offsets, escape_attribute, escape_text};
use crate::xml::{Path, StartTag, XSI_NS, content_range, is_xsi_nil, parse, scan_start_tag};

/// A change the form makes. The form only changes values: it never adds or removes elements or
/// attributes (though making an element nil adds `xsi:nil`, and giving it a value removes it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOp {
    /// Sets an element's content, removing `xsi:nil` if the element was nil.
    SetValue { path: Path, value: String },
    /// Makes an element nil: adds `xsi:nil="true"` and removes its content.
    SetNil { path: Path },
    /// Sets the value of an attribute the element already has, named as written.
    SetAttribute {
        path: Path,
        name: String,
        value: String,
    },
}

/// Replaces `start..end` with `text`. Offsets are in UTF-16 units, as VS Code counts them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: u32,
    pub end: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditError(pub String);

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EditError {}

type ByteEdit = (Range<usize>, String);

/// The edits that make a change to `xml`: none when it's already that way.
pub fn apply_edit(xml: &str, op: &EditOp) -> Result<Vec<TextEdit>, EditError> {
    let doc = parse(xml).map_err(|e| EditError(format!("The XML is not well-formed: {e}")))?;
    let path = match op {
        EditOp::SetValue { path, .. }
        | EditOp::SetNil { path }
        | EditOp::SetAttribute { path, .. } => path,
    };
    let node = path
        .find(&doc)
        .ok_or_else(|| EditError(format!("{path} is no longer in the document")))?;
    let tag = scan_start_tag(xml, node.range().start)
        .ok_or_else(|| EditError(format!("Could not read the start tag of {path}")))?;
    let edits = match op {
        EditOp::SetValue { value, .. } => set_value(xml, node, &tag, value),
        EditOp::SetNil { .. } => set_nil(xml, node, &tag),
        EditOp::SetAttribute { name, value, .. } => {
            let attribute = tag
                .attributes
                .iter()
                .find(|a| &xml[a.name.clone()] == name)
                .ok_or_else(|| EditError(format!("{path} has no attribute {name}")))?;
            vec![(
                attribute.value.clone(),
                escape_attribute(value, attribute.quote),
            )]
        }
    };
    let offsets = Utf16Offsets::new(xml);
    Ok(merge(xml, edits)
        .into_iter()
        .map(|(range, text)| TextEdit {
            start: offsets.of(range.start),
            end: offsets.of(range.end),
            text,
        })
        .collect())
}

fn set_value(src: &str, node: Node, tag: &StartTag, value: &str) -> Vec<ByteEdit> {
    let mut edits = Vec::new();
    if let Some(nil) = tag.attributes.iter().find(|a| is_xsi_nil(src, node, a)) {
        edits.push((nil.lead..nil.end, String::new()));
    }
    let text = escape_text(value);
    match content_range(src, node, tag) {
        Some(content) => edits.push((content, text)),
        // `<x/>` becomes `<x>value</x>`
        None => edits.push((
            tag.tail..tag.end,
            format!(">{text}</{}>", &src[tag.name.clone()]),
        )),
    }
    edits
}

fn set_nil(src: &str, node: Node, tag: &StartTag) -> Vec<ByteEdit> {
    let end = node.range().end;
    let mut edits = Vec::new();
    match tag.attributes.iter().find(|a| is_xsi_nil(src, node, a)) {
        Some(nil) => {
            if !matches!(src[nil.value.clone()].trim(), "true" | "1") {
                edits.push((nil.value.clone(), "true".to_string()));
            }
            if !tag.self_closing {
                edits.push((tag.tail..end, "/>".to_string()));
            }
        }
        None => {
            let attribute = match node
                .lookup_prefix(XSI_NS)
                .filter(|prefix| !prefix.is_empty())
            {
                Some(prefix) => format!(" {prefix}:nil=\"true\""),
                None => format!(" xmlns:xsi=\"{XSI_NS}\" xsi:nil=\"true\""),
            };
            if tag.self_closing {
                edits.push((tag.tail..tag.tail, attribute));
            } else {
                edits.push((tag.tail..end, format!("{attribute}/>")));
            }
        }
    }
    edits
}

/// Sorts edits, drops those that change nothing, and joins edits that touch, which VS Code could
/// otherwise reject as overlapping.
fn merge(src: &str, mut edits: Vec<ByteEdit>) -> Vec<ByteEdit> {
    edits.retain(|(range, text)| &src[range.clone()] != text.as_str());
    edits.sort_by_key(|(range, _)| range.start);
    let mut merged: Vec<ByteEdit> = Vec::with_capacity(edits.len());
    for (range, text) in edits {
        match merged.last_mut() {
            Some((last, last_text)) if range.start <= last.end => {
                last_text.push_str(&text);
                last.end = last.end.max(range.end);
            }
            _ => merged.push((range, text)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies edits as VS Code would, counting in UTF-16 units.
    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut out = Vec::new();
        let mut pos = 0;
        for edit in edits {
            assert!(
                edit.start as usize >= pos,
                "edits overlap or are out of order"
            );
            out.extend_from_slice(&units[pos..edit.start as usize]);
            out.extend(edit.text.encode_utf16());
            pos = edit.end as usize;
        }
        out.extend_from_slice(&units[pos..]);
        String::from_utf16(&out).unwrap()
    }

    fn edit(xml: &str, op: EditOp) -> String {
        apply(xml, &apply_edit(xml, &op).unwrap())
    }

    fn path(steps: &[&str]) -> Path {
        let mut path = Path::root(steps[0]);
        for step in &steps[1..] {
            path = path.child(step, 0);
        }
        path
    }

    const XSI: &str = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;

    #[test]
    fn sets_values() {
        let xml = "<r>\n  <!-- µ -->\n  <a>1</a>\n  <b/>\n  <c></c>\n</r>";
        let set = |steps: &[&str], value: &str| {
            edit(
                xml,
                EditOp::SetValue {
                    path: path(steps),
                    value: value.into(),
                },
            )
        };
        assert_eq!(
            set(&["r", "a"], "2"),
            "<r>\n  <!-- µ -->\n  <a>2</a>\n  <b/>\n  <c></c>\n</r>"
        );
        assert_eq!(
            set(&["r", "b"], "x < y"),
            "<r>\n  <!-- µ -->\n  <a>1</a>\n  <b>x &lt; y</b>\n  <c></c>\n</r>"
        );
        assert_eq!(
            set(&["r", "c"], "3"),
            "<r>\n  <!-- µ -->\n  <a>1</a>\n  <b/>\n  <c>3</c>\n</r>"
        );
        assert!(
            apply_edit(
                xml,
                &EditOp::SetValue {
                    path: path(&["r", "a"]),
                    value: "1".into()
                }
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn sets_and_clears_nil() {
        let xml = format!("<r {XSI}>\r\n    <pulseNo xsi:nil=\"true\"/>\r\n</r>");
        let pulse = path(&["r", "pulseNo"]);
        let valued = edit(
            &xml,
            EditOp::SetValue {
                path: pulse.clone(),
                value: "12345".into(),
            },
        );
        assert_eq!(
            valued,
            format!("<r {XSI}>\r\n    <pulseNo>12345</pulseNo>\r\n</r>")
        );
        assert_eq!(
            edit(
                &valued,
                EditOp::SetNil {
                    path: pulse.clone()
                }
            ),
            xml
        );
        assert!(
            apply_edit(&xml, &EditOp::SetNil { path: pulse })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn declares_xsi_when_needed() {
        let xml = "<r><a>1</a></r>";
        assert_eq!(
            edit(
                xml,
                EditOp::SetNil {
                    path: path(&["r", "a"])
                }
            ),
            format!("<r><a {XSI} xsi:nil=\"true\"/></r>")
        );
    }

    #[test]
    fn keeps_other_attributes() {
        let xml = format!("<r {XSI}><a unit='m' xsi:nil=\"true\" /></r>");
        let valued = edit(
            &xml,
            EditOp::SetValue {
                path: path(&["r", "a"]),
                value: "1.5".into(),
            },
        );
        assert_eq!(valued, format!("<r {XSI}><a unit='m'>1.5</a></r>"));
        let renamed = edit(
            &valued,
            EditOp::SetAttribute {
                path: path(&["r", "a"]),
                name: "unit".into(),
                value: "it's".into(),
            },
        );
        assert_eq!(renamed, format!("<r {XSI}><a unit='it&apos;s'>1.5</a></r>"));
    }
}
