//! Flattens the form into the rows the page shows, applying the filter and the collapsed sections.
//!
//! A flat list lets the page render rows keyed by their content: when the document changes, only
//! rows whose content changed are rebuilt, so the field being edited keeps its focus.
//!
//! The form can show several documents: the one it was opened on, and, in each section that links
//! to a file, that file's contents (see [`FormNode::linked`]). The linked file's root element isn't
//! a row of its own: the linking section shows its attributes, elements and notes.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use xml_form_editor_core::{
    AttributeField, Field, Form, FormNode, Matrix, NodeKind, Note, Path, Variant,
};

use crate::Shared;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Row {
    Section(SectionRow),
    Value(ValueRow),
    Attribute(AttributeRow),
    Matrix(MatrixRow),
}

/// Where a row's element is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Place {
    /// The document the element is in, as the extension names it
    pub document: Arc<str>,
    /// The element's path in its document
    pub path: Path,
    /// Tells the element apart from those of the other documents: its path, after the key of the
    /// section linking to its document and `|` when it's in a linked file
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SectionRow {
    pub place: Place,
    pub depth: usize,
    pub name: String,
    /// Its index when it's an item of a list
    pub index: Option<usize>,
    /// The path its copy button copies: relative to the root element, with the index of each item
    /// of a list, as in `constraint[0]/fit_settings/weight`. `None` for the root, which has no button.
    pub copy_path: Option<String>,
    pub doc: Option<String>,
    pub notes: Vec<Note>,
    pub variant: Variant,
    /// Inside a section its selector doesn't choose
    pub unused: bool,
    pub expanded: bool,
    pub empty: bool,
    pub offset: u32,
    /// The file the section shows, when it links to one
    pub linked_file: Option<LinkedFile>,
}

/// The file a section links to and shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LinkedFile {
    /// The file as the link gives it
    pub href: String,
    pub document: Arc<str>,
    /// Where its root element starts
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ValueRow {
    pub place: Place,
    pub depth: usize,
    pub name: String,
    pub index: Option<usize>,
    pub copy_path: Option<String>,
    pub doc: Option<String>,
    pub field: Field,
    pub notes: Vec<Note>,
    pub variant: Variant,
    pub unused: bool,
    pub selector: bool,
    pub offset: u32,
    pub units: Option<String>,
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AttributeRow {
    /// Where the attribute's element is
    pub place: Place,
    pub depth: usize,
    pub copy_path: String,
    pub attribute: AttributeField,
    pub unused: bool,
    pub offset: u32,
    pub change: Change,
}

/// A section holding a matrix, shown as a grid of its values (see [`Matrix`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MatrixRow {
    pub place: Place,
    pub depth: usize,
    pub name: String,
    pub index: Option<usize>,
    pub copy_path: Option<String>,
    pub doc: Option<String>,
    pub notes: Vec<Note>,
    pub variant: Variant,
    pub unused: bool,
    pub offset: u32,
    pub units: Option<String>,
    pub matrix: Matrix,
    pub lines: Vec<MatrixLine>,
}

/// A row of a matrix.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MatrixLine {
    /// The row's element
    pub path: Path,
    pub values: Vec<String>,
    /// Why the row isn't valid
    pub error: Option<String>,
    /// How each value changed
    pub changes: Vec<Change>,
}

impl MatrixRow {
    /// The number of columns: the length of the longest row.
    pub(crate) fn columns(&self) -> usize {
        self.lines.iter().map(|l| l.values.len()).max().unwrap_or(0)
    }
}

/// How a value differs from the saved file, and from the file as it was when the form opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Change {
    None,
    /// Changed since the file was last saved
    Unsaved,
    /// Changed since the form opened, and saved
    Saved,
}

impl Change {
    /// How the value at `key` in a document (see [`xml_form_editor_core::values`]) changed.
    fn of(key: &str, value: Option<String>, versions: Option<&Versions>) -> Change {
        let differs = |values: &Option<Shared<Values>>| {
            values.as_ref().is_some_and(|v| v.get(key) != Some(&value))
        };
        match versions {
            Some(versions) if differs(&versions.saved) => Change::Unsaved,
            Some(versions) if differs(&versions.original) => Change::Saved,
            _ => Change::None,
        }
    }

    /// How each of the `values` of the list at `key` in a document changed: compared with the
    /// value in the same place in the list, in each version.
    fn of_values(key: &str, values: &[String], versions: Option<&Versions>) -> Vec<Change> {
        // A version's list; empty when that version doesn't have it, or it's nil
        let list = |values: &Option<Shared<Values>>| {
            values.as_ref().map(|v| {
                v.get(key)
                    .cloned()
                    .flatten()
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        };
        let (saved, original) = match versions {
            Some(versions) => (list(&versions.saved), list(&versions.original)),
            None => (None, None),
        };
        let differs = |old: &Option<Vec<String>>, i: usize, value: &String| {
            old.as_ref().is_some_and(|old| old.get(i) != Some(value))
        };
        values
            .iter()
            .enumerate()
            .map(|(i, value)| {
                if differs(&saved, i, value) {
                    Change::Unsaved
                } else if differs(&original, i, value) {
                    Change::Saved
                } else {
                    Change::None
                }
            })
            .collect()
    }
}

impl Row {
    /// Identifies the row for keyed rendering: its key, and a hash of everything it shows.
    ///
    /// A matrix's key leaves out what it shows, so that its grid stays in place as its values
    /// change, and the cell being edited keeps its focus. The grid follows the changes itself.
    pub(crate) fn key(&self) -> String {
        let key = match self {
            Row::Section(row) => row.place.key.clone(),
            Row::Value(row) => row.place.key.clone(),
            Row::Attribute(row) => format!("{}/@{}", row.place.key, row.attribute.name),
            Row::Matrix(row) => return format!("{}#matrix", row.place.key),
        };
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{key}#{:016x}", hasher.finish())
    }
}

/// The values of a version of a document, by path (see [`xml_form_editor_core::values`]).
pub(crate) type Values = HashMap<String, Option<String>>;

/// A document's values as last saved, and as they were when the form opened.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct Versions {
    pub saved: Option<Shared<Values>>,
    pub original: Option<Shared<Values>>,
}

/// How the rows are filtered and folded, and what they're compared with.
pub(crate) struct Folding<'a> {
    pub filter: &'a str,
    /// Sections the user expanded (`true`) or collapsed (`false`), by key
    pub expanded: &'a HashMap<String, bool>,
    pub hide_unused: bool,
    /// Each document's versions, to mark the values that changed
    pub versions: &'a HashMap<Arc<str>, Versions>,
}

/// The rows of `form`, the form of `document` with the linked files it shows.
pub(crate) fn rows(form: &Form, document: &Arc<str>, folding: &Folding) -> Vec<Row> {
    let filter = folding.filter.trim().to_lowercase();
    let mut out = Vec::new();
    let above = Above {
        unused: false,
        matched: false,
        copy_path: "",
        document,
        key_prefix: "",
    };
    walk(&form.root, 0, above, &filter, folding, &mut out);
    out
}

/// What a node takes from the sections it's in.
#[derive(Clone, Copy)]
struct Above<'a> {
    /// In a section its selector doesn't choose
    unused: bool,
    /// In a section whose name matches the filter
    matched: bool,
    /// The section's copy path; empty for the root
    copy_path: &'a str,
    /// The document the node is in
    document: &'a Arc<str>,
    /// What the node's key starts with (see [`Place::key`])
    key_prefix: &'a str,
}

impl Above<'_> {
    fn place(&self, node: &FormNode) -> Place {
        Place {
            document: self.document.clone(),
            path: node.path.clone(),
            key: format!("{}{}", self.key_prefix, node.path),
        }
    }
}

fn walk(
    node: &FormNode,
    depth: usize,
    above: Above,
    filter: &str,
    folding: &Folding,
    out: &mut Vec<Row>,
) {
    let unused = above.unused || matches!(node.variant, Variant::Unselected { .. });
    if unused && folding.hide_unused {
        return;
    }
    if !filter.is_empty() && !above.matched && !subtree_matches(node, filter) {
        return;
    }
    // A section whose name matches shows all it holds; one whose description matches shows only
    // what matches in it. The root's name doesn't count, or everything would match.
    let matched = above.matched
        || (!filter.is_empty() && depth > 0 && node.name.to_lowercase().contains(filter));
    let copy_path = (depth > 0).then(|| join(above.copy_path, &step(node)));
    let place = above.place(node);
    let inner = Above {
        unused,
        matched,
        copy_path: copy_path.as_deref().unwrap_or_default(),
        ..above
    };
    if let (NodeKind::Section { children }, Some(matrix)) = (&node.kind, &node.matrix) {
        out.push(Row::Matrix(MatrixRow {
            place,
            depth,
            name: node.name.clone(),
            index: node.index,
            copy_path: copy_path.clone(),
            doc: node.doc.clone(),
            notes: node.notes.clone(),
            variant: node.variant.clone(),
            unused,
            offset: node.offset,
            units: node.units.clone(),
            matrix: matrix.clone(),
            lines: matrix_lines(children, folding.versions.get(above.document)),
        }));
        // Its attributes follow, as a value's do
        push_attributes(node, depth + 1, inner, filter, folding, out);
        return;
    }
    match &node.kind {
        NodeKind::Section { children } => {
            // A linked file starts folded, so that a file linking to many reads as a list of them
            let collapsed_by_default =
                matches!(node.variant, Variant::Unselected { .. }) || node.linked.is_some();
            let expanded = !filter.is_empty()
                || folding
                    .expanded
                    .get(&place.key)
                    .copied()
                    .unwrap_or(!collapsed_by_default);
            let linked = node.linked.as_deref();
            let mut notes = node.notes.clone();
            let mut doc = node.doc.clone();
            if let Some(linked) = linked
                && matches!(linked.root.kind, NodeKind::Section { .. })
            {
                notes.extend(linked.root.notes.iter().cloned());
                doc = doc.or_else(|| linked.root.doc.clone());
            }
            out.push(Row::Section(SectionRow {
                place,
                depth,
                name: node.name.clone(),
                index: node.index,
                copy_path: copy_path.clone(),
                doc,
                notes,
                variant: node.variant.clone(),
                unused,
                expanded,
                empty: children.is_empty() && node.attributes.is_empty() && linked.is_none(),
                offset: node.offset,
                linked_file: linked
                    .zip(node.link.as_ref())
                    .map(|(linked, href)| LinkedFile {
                        href: href.clone(),
                        document: linked.document.as_str().into(),
                        offset: linked.root.offset,
                    }),
            }));
            if expanded {
                walk_contents(node, depth + 1, inner, filter, folding, out);
            }
        }
        NodeKind::Value(field) => {
            let versions = folding.versions.get(above.document);
            out.push(Row::Value(ValueRow {
                place,
                depth,
                name: node.name.clone(),
                index: node.index,
                copy_path: copy_path.clone(),
                doc: node.doc.clone(),
                field: field.clone(),
                notes: node.notes.clone(),
                variant: node.variant.clone(),
                unused,
                selector: node.selector,
                offset: node.offset,
                units: node.units.clone(),
                change: Change::of(&node.path.to_string(), field.as_written(), versions),
            }));
            walk_contents(node, depth + 1, inner, filter, folding, out);
        }
    }
}

/// The rows of a matrix, from its elements.
fn matrix_lines(rows: &[FormNode], versions: Option<&Versions>) -> Vec<MatrixLine> {
    rows.iter()
        .map(|row| {
            let (values, error) = match &row.kind {
                NodeKind::Value(field) => (
                    field.value.split_whitespace().map(str::to_string).collect(),
                    field.error.clone(),
                ),
                NodeKind::Section { .. } => (Vec::new(), None),
            };
            MatrixLine {
                changes: Change::of_values(&row.path.to_string(), &values, versions),
                path: row.path.clone(),
                values,
                error,
            }
        })
        .collect()
}

/// The rows of what an element holds: its attributes and elements, then the contents of the file
/// it links to. `above` is the element's.
fn walk_contents(
    node: &FormNode,
    depth: usize,
    above: Above,
    filter: &str,
    folding: &Folding,
    out: &mut Vec<Row>,
) {
    push_attributes(node, depth, above, filter, folding, out);
    if let NodeKind::Section { children } = &node.kind {
        for child in children {
            walk(child, depth, above, filter, folding, out);
        }
    }
    if let Some(linked) = node.linked.as_deref() {
        let document: Arc<str> = linked.document.as_str().into();
        let key_prefix = format!("{}{}|", above.key_prefix, node.path);
        let inside = Above {
            document: &document,
            key_prefix: &key_prefix,
            ..above
        };
        match &linked.root.kind {
            NodeKind::Section { .. } => {
                walk_contents(&linked.root, depth, inside, filter, folding, out)
            }
            // A root holding a value is a row of its own
            NodeKind::Value(_) => walk(&linked.root, depth, inside, filter, folding, out),
        }
    }
}

/// The rows of an element's attributes; `above` is the element's. With a filter, only those that
/// match show, unless the element's name matches.
///
/// A section showing the file it links to names the file in its title, so the attribute giving the
/// link doesn't show, unless it has a problem. Until the file shows, the attribute does, to fix.
fn push_attributes(
    node: &FormNode,
    depth: usize,
    above: Above,
    filter: &str,
    folding: &Folding,
    out: &mut Vec<Row>,
) {
    let versions = folding.versions.get(above.document);
    let shown_link = node.linked.as_ref().and(node.link.as_deref());
    let shown = node.attributes.iter().filter(|a| {
        (!gives_link(a, shown_link) || a.field.error.is_some() || a.warning.is_some())
            && (filter.is_empty() || above.matched || attribute_matches(a, filter))
    });
    out.extend(shown.map(|attribute| {
        let key = format!("{}/@{}", node.path, attribute.name);
        Row::Attribute(AttributeRow {
            place: above.place(node),
            depth,
            copy_path: join(above.copy_path, &format!("@{}", attribute.name)),
            attribute: attribute.clone(),
            unused: above.unused,
            offset: node.offset,
            change: Change::of(&key, attribute.field.as_written(), versions),
        })
    }));
}

/// Whether `attribute` is the `href` that gives `link` (see [`FormNode::link`]).
fn gives_link(attribute: &AttributeField, link: Option<&str>) -> bool {
    link.is_some_and(|href| {
        attribute.name.rsplit(':').next() == Some("href") && attribute.field.value.trim() == href
    })
}

/// Adds a step to a copy path, which is empty for the root.
fn join(path: &str, step: &str) -> String {
    if path.is_empty() {
        step.to_string()
    } else {
        format!("{path}/{step}")
    }
}

/// An element's step of a copy path: its name, with its index when it's an item of a list.
fn step(node: &FormNode) -> String {
    match node.index {
        Some(index) => format!("{}[{index}]", node.name),
        None => node.name.clone(),
    }
}

fn matches(node: &FormNode, filter: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(filter);
    has(&node.name)
        || node.doc.as_deref().is_some_and(has)
        || matches!(&node.kind, NodeKind::Value(field) if has(&field.value))
        || node.attributes.iter().any(|a| attribute_matches(a, filter))
}

fn attribute_matches(attribute: &AttributeField, filter: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(filter);
    has(&attribute.name) || attribute.doc.as_deref().is_some_and(has) || has(&attribute.field.value)
}

fn subtree_matches(node: &FormNode, filter: &str) -> bool {
    matches(node, filter) || contents_match(node, filter)
}

/// Whether anything an element holds matches: its elements, or those of the file it links to.
fn contents_match(node: &FormNode, filter: &str) -> bool {
    let own = matches!(&node.kind, NodeKind::Section { children } if children.iter().any(|c| subtree_matches(c, filter)));
    own || node
        .linked
        .as_deref()
        .is_some_and(|linked| match &linked.root.kind {
            // The root's attributes show in the linking section
            NodeKind::Section { .. } => {
                linked
                    .root
                    .attributes
                    .iter()
                    .any(|a| attribute_matches(a, filter))
                    || contents_match(&linked.root, filter)
            }
            NodeKind::Value(_) => subtree_matches(&linked.root, filter),
        })
}

/// The keys of all sections below the root, for expanding or collapsing them all.
pub(crate) fn section_keys(form: &Form) -> Vec<String> {
    /// `prefix` is what the keys of `node`'s elements start with
    fn collect(node: &FormNode, prefix: &str, out: &mut Vec<String>) {
        if let NodeKind::Section { children } = &node.kind {
            for child in children {
                if matches!(child.kind, NodeKind::Section { .. }) {
                    out.push(format!("{prefix}{}", child.path));
                    collect(child, prefix, out);
                }
            }
        }
        if let Some(linked) = &node.linked {
            collect(&linked.root, &format!("{prefix}{}|", node.path), out);
        }
    }
    let mut out = Vec::new();
    collect(&form.root, "", &mut out);
    out
}

/// The keys of the sections that hold a problem, to expand them before showing it.
pub(crate) fn sections_with_problems(form: &Form) -> Vec<String> {
    /// `prefix` is what `node`'s key starts with
    fn collect(node: &FormNode, prefix: &str, ancestors: &mut Vec<String>, out: &mut Vec<String>) {
        let key = format!("{prefix}{}", node.path);
        if has_problem(node) {
            out.extend(ancestors.iter().cloned());
            out.push(key.clone());
        }
        ancestors.push(key);
        collect_contents(node, prefix, ancestors, out);
        ancestors.pop();
    }
    /// The sections in what `node`, the last of the `ancestors`, holds
    fn collect_contents(
        node: &FormNode,
        prefix: &str,
        ancestors: &mut Vec<String>,
        out: &mut Vec<String>,
    ) {
        if let NodeKind::Section { children } = &node.kind {
            for child in children {
                collect(child, prefix, ancestors, out);
            }
        }
        if let Some(linked) = &node.linked {
            let prefix = format!("{prefix}{}|", node.path);
            match &linked.root.kind {
                // The linking section shows the root's problems
                NodeKind::Section { .. } => {
                    if has_problem(&linked.root) {
                        out.extend(ancestors.iter().cloned());
                    }
                    collect_contents(&linked.root, &prefix, ancestors, out);
                }
                NodeKind::Value(_) => collect(&linked.root, &prefix, ancestors, out),
            }
        }
    }
    let mut out = Vec::new();
    collect(&form.root, "", &mut Vec::new(), &mut out);
    out.sort();
    out.dedup();
    out
}

fn has_problem(node: &FormNode) -> bool {
    node.has_error()
        || !node.notes.is_empty()
        || node.attributes.iter().any(|a| a.warning.is_some())
}

#[cfg(test)]
mod tests {
    use xml_form_editor_core::{Linked, Schema, build_form};

    use super::*;

    fn folded<'a>(
        expanded: &'a HashMap<String, bool>,
        versions: &'a HashMap<Arc<str>, Versions>,
    ) -> Folding<'a> {
        Folding {
            filter: "",
            expanded,
            hide_unused: false,
            versions,
        }
    }

    fn copy_paths(rows: &[Row]) -> Vec<Option<&str>> {
        rows.iter()
            .map(|row| match row {
                Row::Section(row) => row.copy_path.as_deref(),
                Row::Value(row) => row.copy_path.as_deref(),
                Row::Attribute(row) => Some(row.copy_path.as_str()),
                Row::Matrix(row) => row.copy_path.as_deref(),
            })
            .collect()
    }

    #[test]
    fn copy_paths_start_below_the_root_and_index_the_items_of_lists() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:element name="settings">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="solver">
                            <xs:complexType>
                                <xs:sequence>
                                    <xs:element name="n_iter" type="xs:int"/>
                                </xs:sequence>
                            </xs:complexType>
                        </xs:element>
                        <xs:element name="constraint" maxOccurs="unbounded">
                            <xs:complexType>
                                <xs:sequence>
                                    <xs:element name="weight" type="xs:double"/>
                                </xs:sequence>
                                <xs:attribute name="unit" type="xs:string"/>
                            </xs:complexType>
                        </xs:element>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;
        let xml = r#"<settings>
            <solver><n_iter>1</n_iter></solver>
            <constraint unit="T"><weight>1</weight></constraint>
            <constraint><weight>0</weight></constraint>
        </settings>"#;
        let schema = Schema::parse(xsd).unwrap();
        let form = build_form(xml, Some(&schema)).unwrap();
        let (expanded, versions) = (HashMap::new(), HashMap::new());
        let shown = rows(&form, &"main".into(), &folded(&expanded, &versions));
        assert_eq!(
            copy_paths(&shown),
            [
                None,
                Some("solver"),
                Some("solver/n_iter"),
                Some("constraint[0]"),
                Some("constraint[0]/@unit"),
                Some("constraint[0]/weight"),
                Some("constraint[1]"),
                Some("constraint[1]/weight"),
            ]
        );
    }

    #[test]
    fn linked_files_show_in_the_section_linking_to_them() {
        let mut form = build_form(
            r#"<settings><numerics href="numerics.xml"/></settings>"#,
            None,
        )
        .unwrap();
        let numerics = build_form(
            r#"<numerics version="2"><picard><n_iter>1</n_iter></picard></numerics>"#,
            None,
        )
        .unwrap();
        // The file's values as saved differ from the form's in `n_iter`
        let saved = numerics_values("<numerics><picard><n_iter>5</n_iter></picard></numerics>");
        let versions = HashMap::from([(
            Arc::from("numerics.xml"),
            Versions {
                saved: Some(saved),
                original: None,
            },
        )]);
        let open = HashMap::from([("/settings/numerics".to_string(), true)]);
        let paths = |form: &Form, filter| {
            let folding = Folding {
                filter,
                ..folded(&open, &versions)
            };
            copy_paths(&rows(form, &"main".into(), &folding))
                .into_iter()
                .map(|path| path.map(str::to_string))
                .collect::<Vec<_>>()
        };

        // Until the file shows, the link does, to fix
        assert_eq!(
            paths(&form, ""),
            [None, Some("numerics".into()), Some("numerics/@href".into())]
        );

        let NodeKind::Section { children } = &mut form.root.kind else {
            panic!("the root holds elements");
        };
        children[0].linked = Some(Box::new(Linked {
            document: "numerics.xml".into(),
            root: numerics.root,
        }));

        // Folded at first
        let folded_at_first = HashMap::new();
        let shown = rows(&form, &"main".into(), &folded(&folded_at_first, &versions));
        assert_eq!(shown.len(), 2);

        // The title names the file, instead of a row for the link
        let shown = rows(&form, &"main".into(), &folded(&open, &versions));
        let places: Vec<(&str, String)> = shown
            .iter()
            .map(|row| match row {
                Row::Section(row) => (&*row.place.document, row.place.key.clone()),
                Row::Value(row) => (&*row.place.document, row.place.key.clone()),
                Row::Attribute(row) => (
                    &*row.place.document,
                    format!("{}/@{}", row.place.key, row.attribute.name),
                ),
                Row::Matrix(row) => (&*row.place.document, row.place.key.clone()),
            })
            .collect();
        assert_eq!(
            places,
            [
                ("main", "/settings".to_string()),
                ("main", "/settings/numerics".to_string()),
                (
                    "numerics.xml",
                    "/settings/numerics|/numerics/@version".to_string()
                ),
                (
                    "numerics.xml",
                    "/settings/numerics|/numerics/picard".to_string()
                ),
                (
                    "numerics.xml",
                    "/settings/numerics|/numerics/picard/n_iter".to_string()
                ),
            ]
        );
        assert_eq!(
            paths(&form, ""),
            [
                None,
                Some("numerics".into()),
                Some("numerics/@version".into()),
                Some("numerics/picard".into()),
                Some("numerics/picard/n_iter".into()),
            ]
        );
        let Row::Section(numerics) = &shown[1] else {
            panic!("numerics is a section");
        };
        let file = numerics
            .linked_file
            .as_ref()
            .expect("the title names the file");
        assert_eq!(
            (&*file.href, &*file.document),
            ("numerics.xml", "numerics.xml")
        );
        let Row::Value(n_iter) = &shown[4] else {
            panic!("n_iter is a value");
        };
        assert_eq!(n_iter.change, Change::Unsaved);
        assert_eq!(
            section_keys(&form),
            ["/settings/numerics", "/settings/numerics|/numerics/picard"]
        );

        // Searching finds what's in the linked file, without the attributes that don't match
        assert_eq!(
            paths(&form, "n_iter"),
            [
                None,
                Some("numerics".into()),
                Some("numerics/picard".into()),
                Some("numerics/picard/n_iter".into()),
            ]
        );
        assert_eq!(
            paths(&form, "version"),
            [
                None,
                Some("numerics".into()),
                Some("numerics/@version".into())
            ]
        );

        // A link with a problem shows, to show the problem
        let NodeKind::Section { children } = &mut form.root.kind else {
            panic!("the root holds elements");
        };
        children[0].attributes[0].warning = Some("Not in the schema".into());
        assert!(paths(&form, "").contains(&Some("numerics/@href".into())));
    }

    fn numerics_values(xml: &str) -> Shared<Values> {
        Shared(Arc::new(xml_form_editor_core::values(xml).unwrap()))
    }

    #[test]
    fn a_matrix_is_one_row_marking_each_value_that_changed() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:simpleType name="DoubleList"><xs:list itemType="xs:double"/></xs:simpleType>
            <xs:element name="r">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="m">
                            <xs:complexType>
                                <xs:sequence>
                                    <xs:element name="row" type="DoubleList" maxOccurs="unbounded"/>
                                </xs:sequence>
                            </xs:complexType>
                        </xs:element>
                        <xs:element name="n" type="xs:int"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;
        let saved = "<r><m><row>1 2</row><row>3 4</row></m><n>1</n></r>";
        let xml = "<r><m><row>1   5</row><row>3 4</row><row>0 0</row></m><n>1</n></r>";
        let schema = Schema::parse(xsd).unwrap();
        let form = build_form(xml, Some(&schema)).unwrap();
        let versions = HashMap::from([(
            Arc::from("main"),
            Versions {
                saved: Some(numerics_values(saved)),
                original: None,
            },
        )]);
        let expanded = HashMap::new();
        let shown = rows(&form, &"main".into(), &folded(&expanded, &versions));
        assert_eq!(copy_paths(&shown), [None, Some("m"), Some("n")]);
        let Row::Matrix(m) = &shown[1] else {
            panic!("m is a matrix");
        };
        let lines: Vec<(Vec<&str>, &[Change])> = m
            .lines
            .iter()
            .map(|l| (l.values.iter().map(String::as_str).collect(), &*l.changes))
            .collect();
        use Change::{None as Same, Unsaved};
        assert_eq!(
            lines,
            [
                (vec!["1", "5"], &[Same, Unsaved][..]),
                (vec!["3", "4"], &[Same, Same][..]),
                (vec!["0", "0"], &[Unsaved, Unsaved][..]),
            ]
        );
        assert_eq!(m.columns(), 2);
        // Its key stays as its values change, so that its grid stays in place
        let edited = build_form(&xml.replace("5", "6"), Some(&schema)).unwrap();
        let again = rows(&edited, &"main".into(), &folded(&expanded, &versions));
        assert_eq!(shown[1].key(), again[1].key());
        assert_ne!(shown[1], again[1]);
    }
}
