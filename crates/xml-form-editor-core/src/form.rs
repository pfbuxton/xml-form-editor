//! Builds the form: the document's elements matched with their declarations in the schema.
//!
//! The form shows only what the document contains. Optional elements it leaves out don't appear,
//! and required ones it leaves out are reported, because the form edits values and never adds or
//! removes elements, other than the rows of a matrix (see [`Matrix`]).

use std::collections::HashMap;

use roxmltree::Node;

use crate::schema::{AttributeDecl, ChildDecl, ElementContent, ElementDecl, Schema};
use crate::simple::SimpleInfo;
use crate::text::Utf16Offsets;
use crate::xml::{
    Path, XSI_NS, XmlError, attribute_namespace, is_nil, local_name, parse, scan_start_tag,
    schema_location_of,
};

/// A document as the form shows it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Form {
    pub root: FormNode,
    /// The schema the document names, as written in it
    pub schema_location: Option<String>,
}

/// One element of the document.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FormNode {
    pub path: Path,
    /// The element's name as written, with any prefix
    pub name: String,
    /// The element's position among its siblings of the same name, counting from 0, when it's an
    /// item of a list: the schema allows more than one of it, or there is more than one
    pub index: Option<usize>,
    pub doc: Option<String>,
    pub kind: NodeKind,
    pub attributes: Vec<AttributeField>,
    /// Problems with the element's structure, such as a missing required child
    pub notes: Vec<Note>,
    pub variant: Variant,
    /// Whether this is an enumeration that chooses which of its siblings is used (see [`Variant`])
    pub selector: bool,
    /// Where the element starts, in UTF-16 units, for showing it in the text editor
    pub offset: u32,
    /// The value's units, from the schema's `xs:appinfo`
    pub units: Option<String>,
    /// The XML file a section links to, as its `href` attribute gives it
    pub link: Option<String>,
    /// The contents of that file, to show in the section. [`build_form`] leaves them out, as it
    /// reads one document: they come from the linked file's own form.
    pub linked: Option<Box<Linked>>,
    /// Set when the element is a section holding a matrix, whose elements are its rows
    pub matrix: Option<Matrix>,
}

/// What makes a section a matrix: the one element its type allows may repeat, and holds a list of
/// numbers, so that each of those elements is a row, as in
/// `<regularisations><row>1.0 0.0</row><row>0.0 1.0</row></regularisations>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Matrix {
    /// The name of the rows' elements, as the schema declares it
    pub row_name: String,
    /// The rows' documentation
    pub row_doc: Option<String>,
    /// The type of a row: a list, of the values
    pub row_type: SimpleInfo,
    /// How many rows the schema allows
    pub min_rows: u32,
    /// `None` for unbounded
    pub max_rows: Option<u32>,
}

impl Matrix {
    /// The type of the values.
    pub fn value_type(&self) -> Option<&SimpleInfo> {
        self.row_type.item.as_deref()
    }

    /// The value new cells start with (see [`SimpleInfo::default_value`]).
    pub fn fill(&self) -> Option<String> {
        self.value_type()?.default_value()
    }

    /// A row of `columns` values to add, when the type of a row allows that many.
    pub fn new_row(&self, columns: usize) -> Option<String> {
        let row = vec![self.fill()?; columns].join(" ");
        self.row_type.validate(&row).is_ok().then_some(row)
    }

    /// Whether the schema allows this many rows.
    pub fn allows_rows(&self, rows: usize) -> bool {
        rows >= self.min_rows as usize && self.max_rows.is_none_or(|max| rows <= max as usize)
    }
}

/// The contents of the file a section links to (see [`FormNode::link`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Linked {
    /// Identifies the file, such as by its URI
    pub document: String,
    /// The root of the file's form
    pub root: FormNode,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(
    clippy::large_enum_variant,
    reason = "most nodes are values, so boxing them would cost more than it saves"
)]
pub enum NodeKind {
    /// An element holding other elements
    Section { children: Vec<FormNode> },
    /// An element holding a value
    Value(Field),
}

/// A value the form can edit: an element's content or an attribute's value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    /// The value as written, with entities decoded; empty when nil
    pub value: String,
    /// Whether the element has `xsi:nil="true"`
    pub nil: bool,
    /// Whether the element may be nil
    pub nillable: bool,
    /// The value's type; `None` when there is no schema, or the schema doesn't describe it
    pub ty: Option<SimpleInfo>,
    /// A value the schema fixes
    pub fixed: Option<String>,
    /// Why the current value isn't valid
    pub error: Option<String>,
}

/// An attribute the element has.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AttributeField {
    /// The attribute's name as written, with any prefix
    pub name: String,
    pub doc: Option<String>,
    pub field: Field,
    /// The value's units, from the schema's `xs:appinfo`
    pub units: Option<String>,
    /// Set when the schema doesn't declare the attribute
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Note {
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

/// Whether an element is the one its siblings' selector chooses.
///
/// When an element has an enumeration child, `method` say, and some of the enumeration's values
/// name sibling elements, then the value of `method` picks which of those siblings is used. The
/// one it picks is [`Variant::Selected`] and the others are [`Variant::Unselected`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Variant {
    Plain,
    Selected { selector: String },
    Unselected { selector: String, value: String },
}

/// Counts of the problems in a form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Problems {
    pub errors: usize,
    pub warnings: usize,
}

impl Note {
    pub fn error(message: impl Into<String>) -> Note {
        Note {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Note {
        Note {
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

impl Field {
    fn new(
        value: String,
        nil: bool,
        nillable: bool,
        ty: Option<SimpleInfo>,
        fixed: Option<String>,
    ) -> Field {
        let mut field = Field {
            value,
            nil,
            nillable,
            ty,
            fixed,
            error: None,
        };
        field.error = if field.nil {
            (!field.nillable).then(|| {
                "xsi:nil is set, but the schema doesn't allow this element to be nil".to_string()
            })
        } else {
            field.check(&field.value)
        };
        field
    }

    /// The value as written in the file, or `None` when nil, for comparing versions of a file.
    pub fn as_written(&self) -> Option<String> {
        (!self.nil).then(|| self.value.clone())
    }

    /// The value as the form shows it: whitespace handled as its type specifies.
    pub fn display_value(&self) -> String {
        self.normalize(&self.value)
    }

    /// The value as it would be written: whitespace handled as its type specifies.
    pub fn normalize(&self, value: &str) -> String {
        self.ty
            .as_ref()
            .map_or_else(|| value.to_string(), |ty| ty.normalize(value))
    }

    /// Why `value` isn't valid for this field, or `None` when it is.
    pub fn check(&self, value: &str) -> Option<String> {
        if let Some(fixed) = &self.fixed
            && self.normalize(value) != self.normalize(fixed)
        {
            return Some(format!("Must be {fixed}, which the schema fixes"));
        }
        self.ty.as_ref()?.validate(value).err()
    }
}

impl Form {
    /// The problems in the form, with those of the linked files it shows.
    pub fn problems(&self) -> Problems {
        let mut problems = Problems::default();
        count_problems(&self.root, &mut problems);
        problems
    }

    /// The files the document links to, as its `href` attributes give them (see [`FormNode::link`]).
    pub fn links(&self) -> Vec<String> {
        fn collect(node: &FormNode, out: &mut Vec<String>) {
            if let Some(link) = &node.link
                && !out.contains(link)
            {
                out.push(link.clone());
            }
            if let NodeKind::Section { children } = &node.kind {
                children.iter().for_each(|c| collect(c, out));
            }
        }
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }
}

impl FormNode {
    /// Whether the element, its value or its attributes have an error.
    pub fn has_error(&self) -> bool {
        self.notes.iter().any(|n| n.severity == Severity::Error)
            || matches!(&self.kind, NodeKind::Value(field) if field.error.is_some())
            || self.attributes.iter().any(|a| a.field.error.is_some())
    }
}

fn count_problems(node: &FormNode, problems: &mut Problems) {
    for note in &node.notes {
        match note.severity {
            Severity::Error => problems.errors += 1,
            Severity::Warning => problems.warnings += 1,
        }
    }
    for attribute in &node.attributes {
        problems.errors += usize::from(attribute.field.error.is_some());
        problems.warnings += usize::from(attribute.warning.is_some());
    }
    match &node.kind {
        NodeKind::Value(field) => problems.errors += usize::from(field.error.is_some()),
        NodeKind::Section { children } => children.iter().for_each(|c| count_problems(c, problems)),
    }
    if let Some(linked) = &node.linked {
        count_problems(&linked.root, problems);
    }
}

/// Every value in a document, as [`Field::as_written`] gives it, keyed by the path of its element
/// (`/a/b[2]`) or attribute (`/a/b/@unit`). Comparing these between versions of a document shows
/// which values changed.
pub fn values(xml: &str) -> Result<HashMap<String, Option<String>>, XmlError> {
    fn collect(node: &FormNode, out: &mut HashMap<String, Option<String>>) {
        for attribute in &node.attributes {
            out.insert(
                format!("{}/@{}", node.path, attribute.name),
                attribute.field.as_written(),
            );
        }
        match &node.kind {
            NodeKind::Value(field) => {
                out.insert(node.path.to_string(), field.as_written());
            }
            NodeKind::Section { children } => children.iter().for_each(|c| collect(c, out)),
        }
    }
    let mut out = HashMap::new();
    collect(&build_form(xml, None)?.root, &mut out);
    Ok(out)
}

/// Builds the form for a document, using its schema when there is one.
pub fn build_form(xml: &str, schema: Option<&Schema>) -> Result<Form, XmlError> {
    let doc = parse(xml)?;
    let builder = Builder {
        src: xml,
        schema,
        offsets: Utf16Offsets::new(xml),
    };
    let root = doc.root_element();
    let name = root.tag_name().name();
    let path = Path::root(name);
    let root = match schema {
        Some(schema) => match schema.global_element(name) {
            Some(decl) => builder.typed(root, path, decl),
            None => {
                let mut node = builder.untyped(root, path);
                node.notes.push(Note::error(format!(
                    "The schema doesn't declare a <{name}> element"
                )));
                node
            }
        },
        None => builder.untyped(root, path),
    };
    Ok(Form {
        root,
        schema_location: schema_location_of(&doc),
    })
}

struct Builder<'a> {
    src: &'a str,
    schema: Option<&'a Schema>,
    offsets: Utf16Offsets,
}

impl Builder<'_> {
    fn blank(&self, node: Node, path: Path, doc: Option<String>) -> FormNode {
        let start = node.range().start;
        let name = scan_start_tag(self.src, start).map_or_else(
            || node.tag_name().name().to_string(),
            |tag| self.src[tag.name].to_string(),
        );
        FormNode {
            path,
            name,
            index: None,
            doc,
            kind: NodeKind::Section {
                children: Vec::new(),
            },
            attributes: Vec::new(),
            notes: Vec::new(),
            variant: Variant::Plain,
            selector: false,
            offset: self.offsets.of(start),
            units: None,
            link: None,
            linked: None,
            matrix: None,
        }
    }

    /// An element the schema declares.
    fn typed(&self, node: Node, path: Path, decl: &ElementDecl) -> FormNode {
        let schema = self.schema.expect("typed elements come with a schema");
        let doc = decl.doc.clone().or_else(|| schema.type_doc(&decl.ty));
        let mut out = self.blank(node, path, doc);
        out.units = decl.units.clone();
        match schema.content(&decl.ty) {
            ElementContent::Text(info) => {
                out.kind = NodeKind::Value(self.value(
                    node,
                    Some(info),
                    decl.nillable,
                    decl.fixed.clone(),
                ));
                if node.children().any(|c| c.is_element()) {
                    out.notes
                        .push(Note::error("Should hold a value, but holds elements"));
                }
            }
            ElementContent::Elements {
                children: decls,
                open,
            } => {
                let children = self.children(node, &out.path, &decls, open, &mut out.notes);
                out.matrix = matrix(schema, &decls, open, &children);
                if out.matrix.is_some() {
                    out.notes.extend(uneven_rows(&children));
                    out.units = out.units.or_else(|| decls[0].decl.units.clone());
                }
                out.kind = NodeKind::Section { children };
            }
            ElementContent::Empty => {
                let children = self.children(node, &out.path, &[], false, &mut out.notes);
                out.kind = NodeKind::Section { children };
                if node
                    .children()
                    .any(|c| c.is_text() && c.text().is_some_and(|t| !t.trim().is_empty()))
                {
                    out.notes
                        .push(Note::warning("Should be empty, but holds text"));
                }
            }
            ElementContent::Untyped => {
                let untyped = self.untyped(node, out.path.clone());
                out.kind = untyped.kind;
            }
        }
        let declared = schema.attributes(&decl.ty);
        out.attributes = self.attributes(node, Some(&declared), &mut out.notes);
        if matches!(out.kind, NodeKind::Section { .. }) {
            out.link = link(&out.attributes);
        }
        out
    }

    /// An element with no type information: a section if it holds elements or links to a file,
    /// else a text value.
    fn untyped(&self, node: Node, path: Path) -> FormNode {
        let mut out = self.blank(node, path, None);
        out.attributes = self.attributes(node, None, &mut out.notes);
        out.link = link(&out.attributes);
        if out.link.is_some() || node.children().any(|c| c.is_element()) {
            let mut counts: HashMap<&str, usize> = HashMap::new();
            let mut children: Vec<FormNode> = node
                .children()
                .filter(Node::is_element)
                .map(|child| {
                    let name = child.tag_name().name();
                    let index = next_index(&mut counts, name);
                    self.untyped(child, out.path.child(name, index))
                })
                .collect();
            number_items(&mut children, |name| counts[name] > 1);
            out.kind = NodeKind::Section { children };
        } else {
            // Nillable when it's nil already, so that it can be given a value
            let nil = is_nil(node);
            out.kind = NodeKind::Value(self.value(node, None, nil, None));
        }
        out
    }

    /// The children of an element whose content model is `decls`, in document order.
    fn children(
        &self,
        node: Node,
        path: &Path,
        decls: &[ChildDecl],
        open: bool,
        notes: &mut Vec<Note>,
    ) -> Vec<FormNode> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut children = Vec::new();
        for child in node.children().filter(Node::is_element) {
            let name = child.tag_name().name();
            let index = next_index(&mut counts, name);
            let child_path = path.child(name, index);
            match decls.iter().find(|d| d.decl.name == name) {
                Some(d) => children.push(self.typed(child, child_path, d.decl)),
                None => {
                    let mut unknown = self.untyped(child, child_path);
                    if !open {
                        unknown.notes.push(Note::warning("Not in the schema"));
                    }
                    children.push(unknown);
                }
            }
        }
        for d in decls {
            let name = &d.decl.name;
            let count = counts.get(name.as_str()).copied().unwrap_or(0);
            if count < d.min as usize {
                notes.push(Note::error(if count == 0 {
                    format!(
                        "Missing <{name}>, which the schema requires. Add it in the text editor."
                    )
                } else {
                    format!(
                        "Needs at least {} <{name}> elements, but has {count}",
                        d.min
                    )
                }));
            }
            if let Some(max) = d.max.filter(|&max| count > max as usize) {
                notes.push(Note::warning(format!(
                    "Allows at most {max} <{name}> elements, but has {count}"
                )));
            }
        }
        number_items(&mut children, |name| {
            counts[name] > 1
                || decls
                    .iter()
                    .find(|d| d.decl.name == name)
                    .is_some_and(|d| d.max.is_none_or(|max| max > 1))
        });
        mark_variants(&mut children);
        children
    }

    fn value(
        &self,
        node: Node,
        ty: Option<SimpleInfo>,
        nillable: bool,
        fixed: Option<String>,
    ) -> Field {
        let nil = is_nil(node);
        let value = if nil {
            String::new()
        } else {
            node.children()
                .filter(|c| c.is_text())
                .filter_map(|c| c.text())
                .collect()
        };
        Field::new(value, nil, nillable, ty, fixed)
    }

    /// The attributes an element has, except namespace declarations and `xsi:` attributes. With
    /// the declarations from the schema, missing required attributes are added to `notes`.
    fn attributes(
        &self,
        node: Node,
        declared: Option<&[(&AttributeDecl, bool)]>,
        notes: &mut Vec<Note>,
    ) -> Vec<AttributeField> {
        let Some(tag) = scan_start_tag(self.src, node.range().start) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for raw in &tag.attributes {
            let qname = &self.src[raw.name.clone()];
            if qname == "xmlns" || qname.starts_with("xmlns:") {
                continue;
            }
            let namespace = attribute_namespace(node, qname);
            if namespace == Some(XSI_NS) {
                continue;
            }
            let local = local_name(qname);
            let value = match namespace {
                Some(namespace) => node.attribute((namespace, local)),
                None => node.attribute(local),
            };
            // Declarations here are for unqualified attributes
            let decl = declared.and_then(|declared| {
                declared
                    .iter()
                    .find(|(a, _)| a.name == local && namespace.is_none())
            });
            let ty = decl
                .zip(self.schema)
                .map(|((a, _), schema)| schema.simple_info(&a.ty));
            out.push(AttributeField {
                name: qname.to_string(),
                doc: decl.and_then(|(a, _)| a.doc.clone()),
                field: Field::new(
                    value.unwrap_or_default().to_string(),
                    false,
                    false,
                    ty,
                    decl.and_then(|(a, _)| a.fixed.clone()),
                ),
                units: decl.and_then(|(a, _)| a.units.clone()),
                warning: (declared.is_some() && decl.is_none())
                    .then(|| "Not in the schema".to_string()),
            });
        }
        for (decl, required) in declared.unwrap_or_default() {
            if *required && !out.iter().any(|a| local_name(&a.name) == decl.name) {
                notes.push(Note::error(format!(
                    "Missing the {} attribute, which the schema requires. Add it in the text editor.",
                    decl.name
                )));
            }
        }
        out
    }
}

fn next_index<'a>(counts: &mut HashMap<&'a str, usize>, name: &'a str) -> usize {
    let count = counts.entry(name).or_default();
    *count += 1;
    *count - 1
}

/// The XML file an element links to: its `href` attribute, in any namespace, when that names a
/// file ending in `.xml`.
fn link(attributes: &[AttributeField]) -> Option<String> {
    let href = attributes.iter().find(|a| local_name(&a.name) == "href")?;
    let href = href.field.value.trim();
    href.to_ascii_lowercase()
        .ends_with(".xml")
        .then(|| href.to_string())
}

/// The matrix a section holds (see [`Matrix`]), when `decls`, its type's content model, allows one
/// element, which may repeat and holds a list of numbers, and `children` are all such elements,
/// with no attributes, which a grid couldn't show.
fn matrix(
    schema: &Schema,
    decls: &[ChildDecl],
    open: bool,
    children: &[FormNode],
) -> Option<Matrix> {
    let [row] = decls else {
        return None;
    };
    let decl = row.decl;
    if open
        || row.max.is_some_and(|max| max < 2)
        || decl.nillable
        || decl.fixed.is_some()
        || !schema.attributes(&decl.ty).is_empty()
    {
        return None;
    }
    let ElementContent::Text(row_type) = schema.content(&decl.ty) else {
        return None;
    };
    let numbers = row_type
        .item
        .as_ref()
        .is_some_and(|item| item.builtin.is_numeric() && !item.is_list() && !item.is_union());
    let rows = children.iter().all(|child| {
        child.path.name() == decl.name
            && child.attributes.is_empty()
            && matches!(child.kind, NodeKind::Value(_))
    });
    (numbers && rows).then(|| Matrix {
        row_name: decl.name.clone(),
        row_doc: decl.doc.clone(),
        row_type,
        min_rows: row.min,
        max_rows: row.max,
    })
}

/// A warning when the rows of a matrix don't all have the same number of values. The rows are
/// numbered from 1, as the form's grid numbers them.
fn uneven_rows(rows: &[FormNode]) -> Option<Note> {
    let counts: Vec<usize> = rows
        .iter()
        .map(|row| match &row.kind {
            NodeKind::Value(field) => field.value.split_whitespace().count(),
            NodeKind::Section { .. } => 0,
        })
        .collect();
    // What most rows have; of equally common counts, the first
    let mut usual = *counts.first()?;
    let mut most = 0;
    for &count in &counts {
        let times = counts.iter().filter(|&&c| c == count).count();
        if times > most {
            (usual, most) = (count, times);
        }
    }
    let odd: Vec<String> = counts
        .iter()
        .enumerate()
        .filter(|&(_, &count)| count != usual)
        .map(|(i, count)| format!("row {} has {count}", i + 1))
        .collect();
    if odd.is_empty() {
        return None;
    }
    const SHOWN: usize = 5;
    let listed = if odd.len() > SHOWN {
        format!(
            "{}, and {} more rows differ",
            odd[..SHOWN].join(", "),
            odd.len() - SHOWN
        )
    } else {
        odd.join(", ")
    };
    Some(Note::warning(format!(
        "The rows don't all have the same number of values: {listed}, where the others have {usual}"
    )))
}

/// Gives the children that `is_list` says, by name, are items of a list their [`FormNode::index`].
fn number_items(children: &mut [FormNode], is_list: impl Fn(&str) -> bool) {
    for child in children {
        if is_list(child.path.name()) {
            child.index = child.path.steps().last().map(|step| step.index);
        }
    }
}

/// Finds a selector among a section's children (see [`Variant`]) and marks the siblings it chooses between.
fn mark_variants(children: &mut [FormNode]) {
    let names: Vec<String> = children.iter().map(|c| c.path.name().to_string()).collect();
    let selector = children.iter().enumerate().find_map(|(i, child)| {
        let NodeKind::Value(field) = &child.kind else {
            return None;
        };
        let ty = field.ty.as_ref()?;
        let chooses = ty.enumeration.iter().any(|e| {
            names
                .iter()
                .enumerate()
                .any(|(j, name)| j != i && *name == e.value)
        });
        chooses.then(|| {
            let options: Vec<String> = ty.enumeration.iter().map(|e| e.value.clone()).collect();
            (
                i,
                child.path.name().to_string(),
                field.display_value(),
                options,
            )
        })
    });
    let Some((index, selector, value, options)) = selector else {
        return;
    };
    children[index].selector = true;
    for (j, child) in children.iter_mut().enumerate() {
        let name = child.path.name();
        if j == index || !options.iter().any(|o| o == name) {
            continue;
        }
        child.variant = if name == value {
            Variant::Selected {
                selector: selector.clone(),
            }
        } else {
            Variant::Unselected {
                selector: selector.clone(),
                value: value.clone(),
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simple::Builtin;

    fn indexes(form: &Form) -> Vec<(&str, Option<usize>)> {
        let NodeKind::Section { children } = &form.root.kind else {
            panic!("the root holds elements");
        };
        children
            .iter()
            .map(|c| (c.name.as_str(), c.index))
            .collect()
    }

    #[test]
    fn finds_links_to_xml_files() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:complexType name="Link">
                <xs:attribute name="href" type="xs:anyURI" use="required"/>
            </xs:complexType>
            <xs:element name="settings">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="grid" type="Link"/>
                        <xs:element name="notes" type="Link"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;
        let xml = r#"<settings><grid href=" ../default/grid.xml "/><notes href="notes.html"/></settings>"#;
        let schema = Schema::parse(xsd).unwrap();
        for form in [
            build_form(xml, Some(&schema)).unwrap(),
            build_form(xml, None).unwrap(),
        ] {
            let NodeKind::Section { children } = &form.root.kind else {
                panic!("the root holds elements");
            };
            // A section even without a schema, to show the linked file in
            assert!(matches!(children[0].kind, NodeKind::Section { .. }));
            assert_eq!(children[0].link.as_deref(), Some("../default/grid.xml"));
            assert_eq!(children[1].link, None);
            assert_eq!(form.links(), ["../default/grid.xml"]);
        }
    }

    #[test]
    fn counts_the_problems_of_linked_files() {
        let mut form = build_form(r#"<settings><grid href="grid.xml"/></settings>"#, None).unwrap();
        let mut grid = build_form("<grid><n_r>81</n_r></grid>", None).unwrap().root;
        grid.notes.push(Note::error("Missing <n_z>"));
        let NodeKind::Section { children } = &mut form.root.kind else {
            panic!("the root holds elements");
        };
        children[0].linked = Some(Box::new(Linked {
            document: "grid.xml".into(),
            root: grid,
        }));
        assert_eq!(form.problems().errors, 1);
    }

    const MATRIX_XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
        <xs:simpleType name="DoubleList">
            <xs:list itemType="xs:double"/>
        </xs:simpleType>
        <xs:complexType name="Matrix">
            <xs:sequence>
                <xs:element name="row" type="DoubleList" minOccurs="0" maxOccurs="unbounded">
                    <xs:annotation><xs:documentation>One row</xs:documentation></xs:annotation>
                </xs:element>
            </xs:sequence>
        </xs:complexType>
        <xs:complexType name="Pair">
            <xs:sequence>
                <xs:element name="row" type="DoubleList" maxOccurs="2"/>
            </xs:sequence>
        </xs:complexType>
        <xs:element name="r">
            <xs:complexType>
                <xs:all>
                    <xs:element name="m" type="Matrix"/>
                    <xs:element name="empty" type="Matrix" minOccurs="0"/>
                    <xs:element name="pair" type="Pair" minOccurs="0"/>
                    <xs:element name="odd" type="Matrix" minOccurs="0"/>
                    <xs:element name="list" type="DoubleList" minOccurs="0"/>
                </xs:all>
            </xs:complexType>
        </xs:element>
    </xs:schema>"#;

    fn child<'a>(form: &'a Form, name: &str) -> &'a FormNode {
        let NodeKind::Section { children } = &form.root.kind else {
            panic!("the root holds elements");
        };
        children.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn finds_matrices() {
        let xml = "<r>
            <m><row>1 2 3</row><row>4 5</row><row>6 7 8</row></m>
            <empty/>
            <pair><row>1</row></pair>
            <odd><row>1</row><other/></odd>
            <list>1 2</list>
        </r>";
        let schema = Schema::parse(MATRIX_XSD).unwrap();
        let form = build_form(xml, Some(&schema)).unwrap();

        let m = child(&form, "m");
        let matrix = m.matrix.as_ref().expect("m is a matrix");
        assert_eq!(matrix.row_name, "row");
        assert_eq!(matrix.row_doc.as_deref(), Some("One row"));
        assert_eq!(matrix.value_type().unwrap().builtin, Builtin::Double);
        assert_eq!((matrix.min_rows, matrix.max_rows), (0, None));
        assert_eq!(matrix.new_row(3).as_deref(), Some("0.0 0.0 0.0"));
        assert_eq!(
            m.notes,
            [Note::warning(
                "The rows don't all have the same number of values: row 2 has 2, where the others have 3"
            )]
        );
        assert_eq!(form.problems().warnings, 2, "m's rows, and odd's <other>");

        let empty = child(&form, "empty");
        assert!(empty.matrix.is_some() && empty.notes.is_empty());
        let pair = child(&form, "pair").matrix.as_ref().unwrap();
        assert!(pair.allows_rows(2) && !pair.allows_rows(3) && !pair.allows_rows(0));
        // Not a matrix: it holds an element that isn't a row, or it's a value, or there's no schema
        assert_eq!(child(&form, "odd").matrix, None);
        assert_eq!(child(&form, "list").matrix, None);
        assert_eq!(child(&build_form(xml, None).unwrap(), "m").matrix, None);
    }

    #[test]
    fn numbers_the_items_of_lists() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:element name="r">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="single" type="xs:string"/>
                        <xs:element name="item" type="xs:string" maxOccurs="unbounded"/>
                        <xs:element name="pair" type="xs:string" minOccurs="0" maxOccurs="2"/>
                        <xs:element name="once" type="xs:string" minOccurs="0"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;
        let xml = "<r><single/><item/><pair/><pair/><once/><once/></r>";
        let schema = Schema::parse(xsd).unwrap();
        // A list by the schema, even with one item; and anything that repeats
        assert_eq!(
            indexes(&build_form(xml, Some(&schema)).unwrap()),
            [
                ("single", None),
                ("item", Some(0)),
                ("pair", Some(0)),
                ("pair", Some(1)),
                ("once", Some(0)),
                ("once", Some(1)),
            ]
        );
        assert_eq!(
            indexes(&build_form(xml, None).unwrap()),
            [
                ("single", None),
                ("item", None),
                ("pair", Some(0)),
                ("pair", Some(1)),
                ("once", Some(0)),
                ("once", Some(1)),
            ]
        );
    }
}
