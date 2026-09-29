//! Turns a change of value into edits of the XML text that touch only what changes, so that
//! formatting and comments survive, undo works as usual, and diffs stay small.

use std::fmt;
use std::ops::Range;

use roxmltree::Node;

use crate::text::{Utf16Offsets, escape_attribute, escape_text};
use crate::xml::{
    Path, StartTag, XSI_NS, content_range, is_nil, is_space, is_xsi_nil, parse, scan_start_tag,
};

/// A change the form makes. The form changes values: it never adds or removes elements or
/// attributes (though making an element nil adds `xsi:nil`, and giving it a value removes it),
/// except the rows of a matrix (see [`crate::Matrix`]).
///
/// A matrix is an element whose elements called `name` are its rows, each holding a list of
/// values; its columns are the values at each position in the lists.
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
    /// Sets the value in column `column`, counting from 0, of the matrix row at `path`. A shorter
    /// row is first padded with `fill`.
    SetCell {
        path: Path,
        column: usize,
        value: String,
        fill: String,
    },
    /// Adds a row holding `value` to the matrix at `path`, before its row `index`, or after its
    /// last row when it has no more.
    InsertRow {
        path: Path,
        name: String,
        index: usize,
        value: String,
    },
    /// Removes the matrix row at `path`. Removing the last row leaves the matrix empty: `<m/>`.
    RemoveRow { path: Path },
    /// Adds `value` to each row of the matrix at `path`, before its column `index`. Rows too short
    /// to have that column are padded with `value`.
    InsertColumn {
        path: Path,
        name: String,
        index: usize,
        value: String,
    },
    /// Removes column `index` from each row of the matrix at `path` that has it.
    RemoveColumn {
        path: Path,
        name: String,
        index: usize,
    },
}

impl EditOp {
    /// The element the change is made to: the one whose value or attribute it sets, the matrix
    /// whose rows or columns it adds or removes, or the row whose value it sets or that it removes.
    pub fn path(&self) -> &Path {
        match self {
            EditOp::SetValue { path, .. }
            | EditOp::SetNil { path }
            | EditOp::SetAttribute { path, .. }
            | EditOp::SetCell { path, .. }
            | EditOp::InsertRow { path, .. }
            | EditOp::RemoveRow { path }
            | EditOp::InsertColumn { path, .. }
            | EditOp::RemoveColumn { path, .. } => path,
        }
    }
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
    let path = op.path();
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
        EditOp::SetCell {
            column,
            value,
            fill,
            ..
        } => set_cell(xml, node, &tag, *column, value, fill),
        EditOp::InsertRow {
            name, index, value, ..
        } => insert_row(xml, node, &tag, name, *index, value),
        EditOp::RemoveRow { .. } => remove_row(xml, node)
            .ok_or_else(|| EditError(format!("{path} is not a row, so it can't be removed")))?,
        EditOp::InsertColumn {
            name, index, value, ..
        } => each_row(xml, node, name, |row, tag| {
            insert_item(xml, row, tag, *index, value)
        }),
        EditOp::RemoveColumn { name, index, .. } => each_row(xml, node, name, |row, tag| {
            remove_item(xml, row, tag, *index)
        }),
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

/// Where the values of a list are written in an element's content: the content, and each value in
/// it. `None` when the content isn't plain text, as in `<row/>`, a nil element, or content with
/// comments or entities, which the edits write anew rather than pick apart.
fn written_items(
    src: &str,
    node: Node,
    tag: &StartTag,
) -> Option<(Range<usize>, Vec<Range<usize>>)> {
    let content = content_range(src, node, tag)?;
    if is_nil(node) || src[content.clone()].contains(['<', '&']) {
        return None;
    }
    let b = src.as_bytes();
    let mut items = Vec::new();
    let mut i = content.start;
    while i < content.end {
        if is_space(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < content.end && !is_space(b[i]) {
            i += 1;
        }
        items.push(start..i);
    }
    Some((content, items))
}

/// The values of a list as the parser reads them, for writing the list anew.
fn items(node: Node) -> Vec<String> {
    if is_nil(node) {
        return Vec::new();
    }
    let text: String = node
        .children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect();
    text.split_whitespace().map(str::to_string).collect()
}

fn set_cell(
    src: &str,
    node: Node,
    tag: &StartTag,
    column: usize,
    value: &str,
    fill: &str,
) -> Vec<ByteEdit> {
    let Some((content, written)) = written_items(src, node, tag) else {
        let mut values = items(node);
        if values.len() <= column {
            values.resize(column + 1, fill.to_string());
        }
        values[column] = value.to_string();
        return set_value(src, node, tag, &values.join(" "));
    };
    if let Some(item) = written.get(column) {
        return vec![(item.clone(), escape_text(value))];
    }
    let mut added = vec![fill; column - written.len()];
    added.push(value);
    let added = escape_text(&added.join(" "));
    match written.last() {
        Some(last) => vec![(last.end..last.end, format!(" {added}"))],
        // Only whitespace, which the values replace
        None => vec![(content, added)],
    }
}

fn insert_item(src: &str, node: Node, tag: &StartTag, index: usize, value: &str) -> Vec<ByteEdit> {
    let Some((content, written)) = written_items(src, node, tag) else {
        let mut values = items(node);
        if values.len() < index {
            values.resize(index, value.to_string());
        }
        values.insert(index, value.to_string());
        return set_value(src, node, tag, &values.join(" "));
    };
    let value = escape_text(value);
    if let Some(item) = written.get(index) {
        return vec![(item.start..item.start, format!("{value} "))];
    }
    let added = vec![value.as_str(); index + 1 - written.len()].join(" ");
    match written.last() {
        Some(last) => vec![(last.end..last.end, format!(" {added}"))],
        None => vec![(content, added)],
    }
}

fn remove_item(src: &str, node: Node, tag: &StartTag, index: usize) -> Vec<ByteEdit> {
    let Some((_, written)) = written_items(src, node, tag) else {
        let mut values = items(node);
        if index >= values.len() {
            return Vec::new();
        }
        values.remove(index);
        return set_value(src, node, tag, &values.join(" "));
    };
    let Some(item) = written.get(index) else {
        return Vec::new();
    };
    // With the space that separates it from the value before, or after when it's the first
    let range = match (index.checked_sub(1), written.get(index + 1)) {
        (Some(before), _) => written[before].end..item.end,
        (None, Some(after)) => item.start..after.start,
        (None, None) => item.clone(),
    };
    vec![(range, String::new())]
}

/// The edits `edit` makes to each row of the matrix `matrix`: its elements called `name`.
fn each_row(
    src: &str,
    matrix: Node,
    name: &str,
    edit: impl Fn(Node, &StartTag) -> Vec<ByteEdit>,
) -> Vec<ByteEdit> {
    matrix
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == name)
        .filter_map(|row| scan_start_tag(src, row.range().start).map(|tag| edit(row, &tag)))
        .flatten()
        .collect()
}

/// Adds a row, written as the rows around it are: named as they're written, with any prefix, and
/// on a line of its own when they are. The first row goes on a line of its own when the matrix
/// is on one, indented one step further.
fn insert_row(
    src: &str,
    matrix: Node,
    tag: &StartTag,
    name: &str,
    index: usize,
    value: &str,
) -> Vec<ByteEdit> {
    let rows: Vec<Node> = matrix
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == name)
        .collect();
    let value = escape_text(value);
    let element = |like: Option<Node>| {
        let name = like
            .and_then(|row| scan_start_tag(src, row.range().start))
            .map_or(name, |tag| &src[tag.name]);
        format!("<{name}>{value}</{name}>")
    };
    if let Some(&next) = rows.get(index) {
        let start = next.range().start;
        let lead = lead(src, start);
        return vec![(start..start, format!("{}{lead}", element(Some(next))))];
    }
    if let Some(&last) = rows.last() {
        let (start, end) = (last.range().start, last.range().end);
        let lead = lead(src, start);
        return vec![(end..end, format!("{lead}{}", element(Some(last))))];
    }
    let element = element(None);
    // Where the row starts, and the matrix's end tag
    let (before, after) = match line_lead(src, matrix.range().start) {
        Some((newline, indent)) => {
            let step = indent_step(src, matrix, indent);
            (
                format!("{newline}{indent}{step}"),
                format!("{newline}{indent}"),
            )
        }
        None => (String::new(), String::new()),
    };
    if tag.self_closing {
        let matrix_name = &src[tag.name.clone()];
        return vec![(
            tag.tail..tag.end,
            format!(">{before}{element}{after}</{matrix_name}>"),
        )];
    }
    let Some(content) = content_range(src, matrix, tag) else {
        return Vec::new();
    };
    if src[content.clone()].trim().is_empty() {
        return vec![(content, format!("{before}{element}{after}"))];
    }
    // After whatever else it holds, such as a comment
    let end = content.start + src[content].trim_end().len();
    vec![(end..end, format!("{before}{element}"))]
}

/// Removes a row with the line break and indentation before it, so that its line goes. `None` for
/// the root element, which isn't a row.
fn remove_row(src: &str, row: Node) -> Option<Vec<ByteEdit>> {
    let matrix = row.parent_element()?;
    let only = matrix
        .children()
        .all(|c| c == row || (c.is_text() && c.text().is_some_and(|t| t.trim().is_empty())));
    if only {
        let tag = scan_start_tag(src, matrix.range().start)?;
        return Some(vec![(tag.tail..matrix.range().end, "/>".to_string())]);
    }
    let (start, end) = (row.range().start, row.range().end);
    Some(vec![(start - lead(src, start).len()..end, String::new())])
}

/// The line break and indentation before `pos`, when only spaces and tabs come before it on its
/// line: `("\n", "    ")`, or `("\r\n", "    ")` in a file with Windows line endings.
fn line_lead(src: &str, pos: usize) -> Option<(&'static str, &str)> {
    let b = src.as_bytes();
    let mut start = pos;
    while start > 0 && matches!(b[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    let newline = if src[..start].ends_with("\r\n") {
        "\r\n"
    } else if src[..start].ends_with('\n') {
        "\n"
    } else {
        return None;
    };
    Some((newline, &src[start..pos]))
}

/// What comes before an element at `pos` and separates it from what's before: its line break and
/// indentation, when it's on a line of its own, else the whitespace before it.
fn lead(src: &str, pos: usize) -> &str {
    match line_lead(src, pos) {
        Some((newline, indent)) => &src[pos - indent.len() - newline.len()..pos],
        None => {
            let b = src.as_bytes();
            let mut start = pos;
            while start > 0 && is_space(b[start - 1]) {
                start -= 1;
            }
            &src[start..pos]
        }
    }
}

/// How much further `node`'s elements are indented than it is, which is indented by `indent`: as
/// much as it is indented from its parent, or four spaces.
fn indent_step<'a>(src: &'a str, node: Node, indent: &'a str) -> &'a str {
    let outer = node.parent_element().and_then(|parent| {
        let start = parent.range().start;
        match line_lead(src, start) {
            Some((_, outer)) => Some(outer),
            // First in the file
            None => src[..start].trim().is_empty().then_some(""),
        }
    });
    match outer {
        Some(outer) if indent.len() > outer.len() && indent.starts_with(outer) => {
            &indent[outer.len()..]
        }
        _ if indent.contains('\t') => "\t",
        _ => "    ",
    }
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

    const MATRIX: &str = "<r>\n    <m>\n        <row>0.0 177.7   0.0</row>\n        <row>0.0   0.0 1e+10</row>\n    </m>\n</r>";

    fn row(i: usize) -> Path {
        path(&["r", "m"]).child("row", i)
    }

    fn set_cell(xml: &str, row: Path, column: usize, value: &str) -> String {
        edit(
            xml,
            EditOp::SetCell {
                path: row,
                column,
                value: value.into(),
                fill: "0.0".into(),
            },
        )
    }

    fn insert_row(xml: &str, index: usize, value: &str) -> String {
        edit(
            xml,
            EditOp::InsertRow {
                path: path(&["r", "m"]),
                name: "row".into(),
                index,
                value: value.into(),
            },
        )
    }

    #[test]
    fn sets_the_values_of_a_matrix_keeping_its_layout() {
        assert_eq!(
            set_cell(MATRIX, row(1), 2, "5"),
            MATRIX.replace("<row>0.0   0.0 1e+10</row>", "<row>0.0   0.0 5</row>")
        );
        // A row too short for the column is padded
        assert_eq!(
            set_cell(MATRIX, row(0), 4, "9"),
            MATRIX.replace("177.7   0.0<", "177.7   0.0 0.0 9<")
        );
        assert_eq!(
            set_cell("<r><m><row/></m></r>", row(0), 1, "7"),
            "<r><m><row>0.0 7</row></m></r>"
        );
        // Content that isn't plain text is written anew
        assert_eq!(
            set_cell(
                "<r><m><row>1&#32;2 <!-- c --></row></m></r>",
                row(0),
                0,
                "3"
            ),
            "<r><m><row>3 2</row></m></r>"
        );
    }

    #[test]
    fn adds_rows_as_the_others_are_written() {
        assert_eq!(
            insert_row(MATRIX, 2, "1 2 3"),
            MATRIX.replace("1e+10</row>", "1e+10</row>\n        <row>1 2 3</row>")
        );
        assert_eq!(
            insert_row(MATRIX, 0, "1 2 3"),
            MATRIX.replace("<m>\n", "<m>\n        <row>1 2 3</row>\n")
        );
        let crlf = "<r xmlns:p=\"urn:p\">\r\n\t<m>\r\n\t\t<p:row>1</p:row>\r\n\t</m>\r\n</r>";
        assert_eq!(
            insert_row(crlf, 1, "2"),
            crlf.replace("</p:row>", "</p:row>\r\n\t\t<p:row>2</p:row>")
        );
    }

    #[test]
    fn adds_the_first_row_indented_one_step_further() {
        assert_eq!(
            insert_row("<r>\n    <m/>\n</r>", 0, "1"),
            "<r>\n    <m>\n        <row>1</row>\n    </m>\n</r>"
        );
        assert_eq!(
            insert_row("<r>\r\n\t<m>\r\n\t</m>\r\n</r>", 0, "1"),
            "<r>\r\n\t<m>\r\n\t\t<row>1</row>\r\n\t</m>\r\n</r>"
        );
        assert_eq!(
            insert_row(
                "<?xml version=\"1.0\"?>\n<r>\n  <m unit=\"1\" />\n</r>",
                0,
                "1"
            ),
            "<?xml version=\"1.0\"?>\n<r>\n  <m unit=\"1\">\n    <row>1</row>\n  </m>\n</r>"
        );
        assert_eq!(
            insert_row("<r><m/></r>", 0, "1"),
            "<r><m><row>1</row></m></r>"
        );
    }

    #[test]
    fn removes_rows_with_their_lines() {
        let remove = |xml: &str, i| edit(xml, EditOp::RemoveRow { path: row(i) });
        assert_eq!(
            remove(MATRIX, 0),
            MATRIX.replace("\n        <row>0.0 177.7   0.0</row>", "")
        );
        assert_eq!(
            remove(MATRIX, 1),
            MATRIX.replace("\n        <row>0.0   0.0 1e+10</row>", "")
        );
        // The last row leaves the matrix empty
        assert_eq!(remove(&remove(MATRIX, 1), 0), "<r>\n    <m/>\n</r>");
        assert!(apply_edit(MATRIX, &EditOp::RemoveRow { path: path(&["r"]) }).is_err());
    }

    #[test]
    fn adds_and_removes_columns() {
        let insert = |xml: &str, index| {
            edit(
                xml,
                EditOp::InsertColumn {
                    path: path(&["r", "m"]),
                    name: "row".into(),
                    index,
                    value: "0.0".into(),
                },
            )
        };
        let remove = |xml: &str, index| {
            edit(
                xml,
                EditOp::RemoveColumn {
                    path: path(&["r", "m"]),
                    name: "row".into(),
                    index,
                },
            )
        };
        let rows = |xml: String| -> Vec<String> {
            xml.lines()
                .filter_map(|line| line.trim().strip_prefix("<row>")?.strip_suffix("</row>"))
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            rows(insert(MATRIX, 3)),
            ["0.0 177.7   0.0 0.0", "0.0   0.0 1e+10 0.0"]
        );
        assert_eq!(
            rows(insert(MATRIX, 1)),
            ["0.0 0.0 177.7   0.0", "0.0   0.0 0.0 1e+10"]
        );
        // A short row is padded to the new column
        let short = MATRIX.replace("0.0   0.0 1e+10", "1");
        assert_eq!(
            rows(insert(&short, 3)),
            ["0.0 177.7   0.0 0.0", "1 0.0 0.0 0.0"]
        );
        assert_eq!(rows(remove(MATRIX, 1)), ["0.0   0.0", "0.0 1e+10"]);
        assert_eq!(rows(remove(MATRIX, 0)), ["177.7   0.0", "0.0 1e+10"]);
        assert_eq!(rows(remove(&short, 2)), ["0.0 177.7", "1"]);
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
