//! Reads an XSD into the model the form is built from.
//!
//! This covers what describes a document's elements and values: global and local elements and
//! attributes, named and anonymous simple and complex types, restrictions with their facets,
//! lists, unions, sequences, choices, `xs:all`, model groups, attribute groups, simple content,
//! and complex content extending another type. Identity constraints, substitution groups and
//! `xs:include`/`xs:import` are not supported.

use std::collections::HashMap;
use std::fmt;

use roxmltree::Node;

use crate::simple::{Bound, Builtin, EnumValue, Facets, SimpleInfo};
use crate::xml::{local_name, parse};

const XS_NS: &str = "http://www.w3.org/2001/XMLSchema";

/// Deep enough for any real schema, while stopping circular type references.
const MAX_DEPTH: usize = 40;

/// Why a schema couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError(pub String);

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemaError {}

/// A parsed XSD.
#[derive(Debug, Default)]
pub struct Schema {
    elements: HashMap<String, ElementDecl>,
    simple_types: HashMap<String, SimpleTypeDef>,
    complex_types: HashMap<String, ComplexTypeDef>,
    groups: HashMap<String, Particle>,
    attribute_groups: HashMap<String, Vec<AttributeItem>>,
    attributes: HashMap<String, AttributeDecl>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ElementDecl {
    pub name: String,
    pub ty: TypeRef,
    pub nillable: bool,
    pub fixed: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum TypeRef {
    Builtin(Builtin),
    /// A type the schema defines by name
    Named(String),
    Simple(Box<SimpleTypeDef>),
    Complex(Box<ComplexTypeDef>),
    /// `xs:anyType`, or no type at all: anything goes
    Any,
}

#[derive(Clone, Debug)]
pub(crate) struct SimpleTypeDef {
    name: Option<String>,
    doc: Option<String>,
    variety: Variety,
}

#[derive(Clone, Debug)]
enum Variety {
    Restriction { base: TypeRef, facets: Facets },
    List { item: TypeRef },
    Union { members: Vec<TypeRef> },
}

#[derive(Clone, Debug)]
pub(crate) struct ComplexTypeDef {
    doc: Option<String>,
    content: Content,
    attributes: Vec<AttributeItem>,
}

#[derive(Clone, Debug)]
enum Content {
    Empty,
    /// Text of a simple type (`xs:simpleContent`)
    Simple(TypeRef),
    Elements(Particle),
    /// `xs:complexContent` extending a named type, perhaps with more elements
    Extension {
        base: String,
        extra: Option<Particle>,
    },
}

#[derive(Clone, Copy, Debug)]
struct Occurs {
    min: u32,
    /// `None` for unbounded
    max: Option<u32>,
}

#[derive(Clone, Debug)]
enum Particle {
    Element(ElementDecl, Occurs),
    ElementRef(String, Occurs),
    /// `xs:sequence` or `xs:all`
    Sequence(Vec<Particle>, Occurs),
    Choice(Vec<Particle>, Occurs),
    GroupRef(String, Occurs),
    Any,
}

#[derive(Clone, Debug)]
pub(crate) struct AttributeDecl {
    pub name: String,
    pub ty: TypeRef,
    pub required: bool,
    pub fixed: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
}

#[derive(Clone, Debug)]
enum AttributeItem {
    Decl(AttributeDecl),
    /// A reference to a global attribute, and whether it's required
    Ref(String, bool),
    Group(String),
}

/// A child element a content model allows, with how often it may occur there.
pub(crate) struct ChildDecl<'a> {
    pub decl: &'a ElementDecl,
    pub min: u32,
    pub max: Option<u32>,
}

/// What an element may contain.
#[allow(
    clippy::large_enum_variant,
    reason = "built once per element and used at once"
)]
pub(crate) enum ElementContent<'a> {
    /// A value of a simple type
    Text(SimpleInfo),
    /// Child elements; `open` when `xs:any` also allows elements the schema doesn't name
    Elements {
        children: Vec<ChildDecl<'a>>,
        open: bool,
    },
    Empty,
    /// No type information
    Untyped,
}

impl Schema {
    pub fn parse(xsd: &str) -> Result<Schema, SchemaError> {
        let doc = parse(xsd)
            .map_err(|e| SchemaError(format!("The schema is not well-formed XML: {e}")))?;
        let root = doc.root_element();
        if !is_xs(root, "schema") {
            return Err(SchemaError(
                "The schema's root element is not xs:schema".into(),
            ));
        }
        let mut schema = Schema::default();
        for child in xs_children(root) {
            match child.tag_name().name() {
                "element" => {
                    let decl = element_decl(child);
                    schema.elements.insert(decl.name.clone(), decl);
                }
                "simpleType" => {
                    if let Some(name) = child.attribute("name") {
                        schema
                            .simple_types
                            .insert(name.to_string(), simple_type(child));
                    }
                }
                "complexType" => {
                    if let Some(name) = child.attribute("name") {
                        schema
                            .complex_types
                            .insert(name.to_string(), complex_type(child));
                    }
                }
                "group" => {
                    if let (Some(name), Some(model)) = (
                        child.attribute("name"),
                        xs_children(child).find_map(particle),
                    ) {
                        schema.groups.insert(name.to_string(), model);
                    }
                }
                "attributeGroup" => {
                    if let Some(name) = child.attribute("name") {
                        schema
                            .attribute_groups
                            .insert(name.to_string(), attribute_items(child));
                    }
                }
                "attribute" => {
                    let decl = attribute_decl(child);
                    schema.attributes.insert(decl.name.clone(), decl);
                }
                kind @ ("include" | "import" | "redefine" | "override") => {
                    let location = child
                        .attribute("schemaLocation")
                        .unwrap_or("another schema");
                    schema.warnings.push(format!(
                        "xs:{kind} is not supported, so definitions from {location} are missing"
                    ));
                }
                _ => {}
            }
        }
        if schema.elements.is_empty() {
            return Err(SchemaError("The schema declares no global elements".into()));
        }
        Ok(schema)
    }

    /// Parts of the schema the form doesn't understand, which leave some values unchecked.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub(crate) fn global_element(&self, name: &str) -> Option<&ElementDecl> {
        self.elements.get(name)
    }

    /// The documentation of a complex type, for elements that have none of their own.
    pub(crate) fn type_doc(&self, ty: &TypeRef) -> Option<String> {
        self.complex(ty).and_then(|ct| ct.doc.clone())
    }

    pub(crate) fn content<'a>(&'a self, ty: &'a TypeRef) -> ElementContent<'a> {
        if let Some(ct) = self.complex(ty) {
            return self.complex_content(ct, 0);
        }
        match ty {
            TypeRef::Any => ElementContent::Untyped,
            TypeRef::Named(name) if !self.simple_types.contains_key(name) => {
                ElementContent::Untyped
            }
            _ => ElementContent::Text(self.simple_info(ty)),
        }
    }

    fn complex<'a>(&'a self, ty: &'a TypeRef) -> Option<&'a ComplexTypeDef> {
        match ty {
            TypeRef::Complex(ct) => Some(ct),
            TypeRef::Named(name) => self.complex_types.get(name),
            _ => None,
        }
    }

    fn complex_content<'a>(&'a self, ct: &'a ComplexTypeDef, depth: usize) -> ElementContent<'a> {
        match &ct.content {
            Content::Empty => ElementContent::Empty,
            Content::Simple(ty) => ElementContent::Text(self.simple_info(ty)),
            Content::Elements(particle) => {
                let (mut children, mut open) = (Vec::new(), false);
                self.flatten(particle, 1, Some(1), &mut children, &mut open, 0);
                ElementContent::Elements { children, open }
            }
            Content::Extension { base, extra } => {
                let base = match self.complex_types.get(base) {
                    Some(base) if depth < MAX_DEPTH => self.complex_content(base, depth + 1),
                    _ => ElementContent::Untyped,
                };
                let (mut children, mut open) = match base {
                    ElementContent::Elements { children, open } => (children, open),
                    ElementContent::Text(info) if extra.is_none() => {
                        return ElementContent::Text(info);
                    }
                    ElementContent::Untyped if extra.is_none() => return ElementContent::Untyped,
                    _ => (Vec::new(), false),
                };
                match extra {
                    Some(extra) => self.flatten(extra, 1, Some(1), &mut children, &mut open, 0),
                    None if children.is_empty() => return ElementContent::Empty,
                    None => {}
                }
                ElementContent::Elements { children, open }
            }
        }
    }

    /// Lists the elements a content model allows, with how often each may occur: the product of
    /// the occurrences of the particles around it, and 0 at least when it's one of a choice.
    fn flatten<'a>(
        &'a self,
        particle: &'a Particle,
        min: u32,
        max: Option<u32>,
        out: &mut Vec<ChildDecl<'a>>,
        open: &mut bool,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let scale = |o: &Occurs| {
            (
                min.saturating_mul(o.min),
                max.zip(o.max).map(|(a, b)| a.saturating_mul(b)),
            )
        };
        match particle {
            Particle::Element(decl, occurs) => push_child(out, decl, scale(occurs)),
            Particle::ElementRef(name, occurs) => {
                if let Some(decl) = self.elements.get(name) {
                    push_child(out, decl, scale(occurs));
                }
            }
            Particle::Sequence(items, occurs) => {
                let (min, max) = scale(occurs);
                for item in items {
                    self.flatten(item, min, max, out, open, depth + 1);
                }
            }
            Particle::Choice(items, occurs) => {
                let (min, max) = scale(occurs);
                let min = if items.len() > 1 { 0 } else { min };
                for item in items {
                    self.flatten(item, min, max, out, open, depth + 1);
                }
            }
            Particle::GroupRef(name, occurs) => {
                if let Some(group) = self.groups.get(name) {
                    let (min, max) = scale(occurs);
                    self.flatten(group, min, max, out, open, depth + 1);
                }
            }
            Particle::Any => *open = true,
        }
    }

    /// The attributes an element of this type may have, and whether each is required.
    pub(crate) fn attributes<'a>(&'a self, ty: &'a TypeRef) -> Vec<(&'a AttributeDecl, bool)> {
        let mut out = Vec::new();
        if let Some(ct) = self.complex(ty) {
            self.collect_attributes(ct, &mut out, 0);
        }
        out
    }

    fn collect_attributes<'a>(
        &'a self,
        ct: &'a ComplexTypeDef,
        out: &mut Vec<(&'a AttributeDecl, bool)>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let base = match &ct.content {
            Content::Extension { base, .. } | Content::Simple(TypeRef::Named(base)) => {
                self.complex_types.get(base)
            }
            _ => None,
        };
        if let Some(base) = base {
            self.collect_attributes(base, out, depth + 1);
        }
        self.collect_attribute_items(&ct.attributes, out, depth);
    }

    fn collect_attribute_items<'a>(
        &'a self,
        items: &'a [AttributeItem],
        out: &mut Vec<(&'a AttributeDecl, bool)>,
        depth: usize,
    ) {
        for item in items {
            match item {
                AttributeItem::Decl(decl) => out.push((decl, decl.required)),
                AttributeItem::Ref(name, required) => {
                    if let Some(decl) = self.attributes.get(name) {
                        out.push((decl, *required));
                    }
                }
                AttributeItem::Group(name) => {
                    if let Some(group) = self
                        .attribute_groups
                        .get(name)
                        .filter(|_| depth < MAX_DEPTH)
                    {
                        self.collect_attribute_items(group, out, depth + 1);
                    }
                }
            }
        }
    }

    /// Resolves a simple type through its derivation.
    pub(crate) fn simple_info(&self, ty: &TypeRef) -> SimpleInfo {
        self.simple_info_at(ty, 0)
    }

    fn simple_info_at(&self, ty: &TypeRef, depth: usize) -> SimpleInfo {
        if depth > MAX_DEPTH {
            return SimpleInfo::default();
        }
        let simple_content = |ct: &ComplexTypeDef| match &ct.content {
            Content::Simple(ty) => self.simple_info_at(ty, depth + 1),
            _ => SimpleInfo::default(),
        };
        match ty {
            TypeRef::Builtin(builtin) => SimpleInfo::builtin(*builtin),
            TypeRef::Simple(def) => self.simple_type_info(def, depth),
            TypeRef::Named(name) => {
                match (self.simple_types.get(name), self.complex_types.get(name)) {
                    (Some(def), _) => self.simple_type_info(def, depth),
                    (None, Some(ct)) => simple_content(ct),
                    (None, None) => SimpleInfo::default(),
                }
            }
            TypeRef::Complex(ct) => simple_content(ct),
            TypeRef::Any => SimpleInfo::default(),
        }
    }

    fn simple_type_info(&self, def: &SimpleTypeDef, depth: usize) -> SimpleInfo {
        let mut info = match &def.variety {
            Variety::Restriction { base, facets } => {
                let mut info = self.simple_info_at(base, depth + 1);
                info.restrict(facets);
                info
            }
            Variety::List { item } => SimpleInfo::list(self.simple_info_at(item, depth + 1)),
            Variety::Union { members } => SimpleInfo::union(
                members
                    .iter()
                    .map(|m| self.simple_info_at(m, depth + 1))
                    .collect(),
            ),
        };
        info.name = def.name.clone();
        if def.doc.is_some() {
            info.doc = def.doc.clone();
        }
        info
    }
}

fn push_child<'a>(
    out: &mut Vec<ChildDecl<'a>>,
    decl: &'a ElementDecl,
    (min, max): (u32, Option<u32>),
) {
    // The same name in two branches of a choice: the first declaration wins
    if !out.iter().any(|c| c.decl.name == decl.name) {
        out.push(ChildDecl { decl, min, max });
    }
}

fn is_xs(node: Node, name: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace() == Some(XS_NS)
        && node.tag_name().name() == name
}

fn xs_children<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(|c| c.is_element() && c.tag_name().namespace() == Some(XS_NS))
}

fn xs_child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    xs_children(node).find(|c| c.tag_name().name() == name)
}

/// The text of a node's `xs:annotation/xs:documentation`, with the line breaks and indentation
/// that come from the schema's layout removed, and paragraphs separated by a blank line.
fn documentation(node: Node) -> Option<String> {
    let paragraphs: Vec<String> = xs_children(node)
        .filter(|c| c.tag_name().name() == "annotation")
        .flat_map(|annotation| {
            xs_children(annotation).filter(|c| c.tag_name().name() == "documentation")
        })
        .map(|doc| {
            tidy(
                &doc.descendants()
                    .filter(|n| n.is_text())
                    .filter_map(|n| n.text())
                    .collect::<String>(),
            )
        })
        .filter(|text| !text.is_empty())
        .collect();
    (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n"))
}

/// The node's units, from `xs:annotation/xs:appinfo/units`: the convention of the IMAS Data
/// Dictionary, where they're SI symbols such as `m` or `A.m^-2`, and `1` for dimensionless.
fn units(node: Node) -> Option<String> {
    xs_children(node)
        .filter(|c| c.tag_name().name() == "annotation")
        .flat_map(|annotation| xs_children(annotation).filter(|c| c.tag_name().name() == "appinfo"))
        .flat_map(|appinfo| {
            appinfo
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "units")
        })
        .find_map(|units| {
            units
                .text()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        })
}

fn tidy(text: &str) -> String {
    let mut paragraphs = Vec::new();
    let mut lines = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            if !lines.is_empty() {
                paragraphs.push(lines.join(" "));
                lines.clear();
            }
        } else {
            lines.push(line);
        }
    }
    if !lines.is_empty() {
        paragraphs.push(lines.join(" "));
    }
    paragraphs.join("\n\n")
}

/// Resolves a type name such as `xs:double` or `tns:Settings`.
fn type_ref(node: Node, qname: &str) -> TypeRef {
    let (prefix, local) = match qname.trim().split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, qname.trim()),
    };
    let namespace = node.lookup_namespace_uri(prefix);
    let builtin =
        namespace == Some(XS_NS) || (namespace.is_none() && matches!(prefix, Some("xs" | "xsd")));
    if !builtin {
        return TypeRef::Named(local.to_string());
    }
    if local == "anyType" {
        return TypeRef::Any;
    }
    TypeRef::Builtin(Builtin::from_xs_name(local).unwrap_or_default())
}

/// The type of an element or attribute: named by `type`, or defined inside it.
fn declared_type(node: Node) -> TypeRef {
    if let Some(name) = node.attribute("type") {
        return type_ref(node, name);
    }
    if let Some(simple) = xs_child(node, "simpleType") {
        return TypeRef::Simple(Box::new(simple_type(simple)));
    }
    if let Some(complex) = xs_child(node, "complexType") {
        return TypeRef::Complex(Box::new(complex_type(complex)));
    }
    TypeRef::Any
}

/// The type an `xs:restriction` or `xs:list` builds on: named by `attribute`, or defined inside it.
fn inner_type(node: Node, attribute: &str) -> TypeRef {
    match node.attribute(attribute) {
        Some(name) => type_ref(node, name),
        None => xs_child(node, "simpleType")
            .map_or(TypeRef::Builtin(Builtin::AnySimpleType), |s| {
                TypeRef::Simple(Box::new(simple_type(s)))
            }),
    }
}

fn element_decl(node: Node) -> ElementDecl {
    ElementDecl {
        name: node.attribute("name").unwrap_or_default().to_string(),
        ty: declared_type(node),
        nillable: matches!(
            node.attribute("nillable").map(str::trim),
            Some("true" | "1")
        ),
        fixed: node.attribute("fixed").map(str::to_string),
        doc: documentation(node),
        units: units(node),
    }
}

fn occurs(node: Node) -> Occurs {
    let min = node
        .attribute("minOccurs")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(1);
    let max = match node.attribute("maxOccurs").map(str::trim) {
        Some("unbounded") => None,
        Some(v) => Some(v.parse().unwrap_or(1)),
        None => Some(1),
    };
    Occurs { min, max }
}

fn particle(node: Node) -> Option<Particle> {
    let occurs = occurs(node);
    Some(match node.tag_name().name() {
        "element" => match node.attribute("ref") {
            Some(name) => Particle::ElementRef(local_name(name).to_string(), occurs),
            None => Particle::Element(element_decl(node), occurs),
        },
        "sequence" | "all" => {
            Particle::Sequence(xs_children(node).filter_map(particle).collect(), occurs)
        }
        "choice" => Particle::Choice(xs_children(node).filter_map(particle).collect(), occurs),
        "group" => Particle::GroupRef(local_name(node.attribute("ref")?).to_string(), occurs),
        "any" => Particle::Any,
        _ => return None,
    })
}

fn simple_type(node: Node) -> SimpleTypeDef {
    let variety = if let Some(restriction) = xs_child(node, "restriction") {
        Variety::Restriction {
            base: inner_type(restriction, "base"),
            facets: facets(restriction),
        }
    } else if let Some(list) = xs_child(node, "list") {
        Variety::List {
            item: inner_type(list, "itemType"),
        }
    } else if let Some(union) = xs_child(node, "union") {
        let mut members: Vec<TypeRef> = union
            .attribute("memberTypes")
            .map(|names| {
                names
                    .split_whitespace()
                    .map(|name| type_ref(union, name))
                    .collect()
            })
            .unwrap_or_default();
        members.extend(
            xs_children(union)
                .filter(|c| c.tag_name().name() == "simpleType")
                .map(|s| TypeRef::Simple(Box::new(simple_type(s)))),
        );
        Variety::Union { members }
    } else {
        Variety::Restriction {
            base: TypeRef::Builtin(Builtin::AnySimpleType),
            facets: Facets::default(),
        }
    };
    SimpleTypeDef {
        name: node.attribute("name").map(str::to_string),
        doc: documentation(node),
        variety,
    }
}

fn facets(restriction: Node) -> Facets {
    let mut facets = Facets::default();
    for facet in xs_children(restriction) {
        let value = facet.attribute("value").unwrap_or_default();
        let bound = |inclusive| {
            Some(Bound {
                value: value.trim().to_string(),
                inclusive,
            })
        };
        match facet.tag_name().name() {
            "enumeration" => facets.enumeration.push(EnumValue {
                value: value.to_string(),
                doc: documentation(facet),
            }),
            "pattern" => facets.patterns.push(value.to_string()),
            "minInclusive" => facets.min = bound(true),
            "minExclusive" => facets.min = bound(false),
            "maxInclusive" => facets.max = bound(true),
            "maxExclusive" => facets.max = bound(false),
            "length" => facets.length = value.trim().parse().ok(),
            "minLength" => facets.min_length = value.trim().parse().ok(),
            "maxLength" => facets.max_length = value.trim().parse().ok(),
            _ => {}
        }
    }
    facets
}

fn complex_type(node: Node) -> ComplexTypeDef {
    let mut content = Content::Empty;
    let mut attributes = Vec::new();
    for child in xs_children(node) {
        match child.tag_name().name() {
            "sequence" | "all" | "choice" | "group" => {
                if let Some(model) = particle(child) {
                    content = Content::Elements(model);
                }
            }
            "attribute" | "attributeGroup" => attributes.extend(attribute_item(child)),
            "simpleContent" => {
                let Some(derivation) = derivation(child) else {
                    continue;
                };
                let base = inner_type(derivation, "base");
                let restricted = facets(derivation);
                content = Content::Simple(
                    if derivation.tag_name().name() == "restriction" && !restricted.is_empty() {
                        TypeRef::Simple(Box::new(SimpleTypeDef {
                            name: None,
                            doc: None,
                            variety: Variety::Restriction {
                                base,
                                facets: restricted,
                            },
                        }))
                    } else {
                        base
                    },
                );
                attributes.extend(attribute_items(derivation));
            }
            "complexContent" => {
                let Some(derivation) = derivation(child) else {
                    continue;
                };
                let extra = xs_children(derivation).find_map(particle);
                let base = derivation
                    .attribute("base")
                    .map(|name| type_ref(derivation, name));
                content = match (derivation.tag_name().name(), base) {
                    ("extension", Some(TypeRef::Named(base))) => Content::Extension { base, extra },
                    // A restriction restates the whole content model
                    _ => extra.map_or(Content::Empty, Content::Elements),
                };
                attributes.extend(attribute_items(derivation));
            }
            _ => {}
        }
    }
    ComplexTypeDef {
        doc: documentation(node),
        content,
        attributes,
    }
}

fn derivation<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    xs_children(node).find(|c| matches!(c.tag_name().name(), "extension" | "restriction"))
}

fn attribute_items(node: Node) -> Vec<AttributeItem> {
    xs_children(node).filter_map(attribute_item).collect()
}

fn attribute_item(node: Node) -> Option<AttributeItem> {
    let required = node.attribute("use") == Some("required");
    match node.tag_name().name() {
        "attribute" => Some(match node.attribute("ref") {
            Some(name) => AttributeItem::Ref(local_name(name).to_string(), required),
            None => AttributeItem::Decl(attribute_decl(node)),
        }),
        "attributeGroup" => node
            .attribute("ref")
            .map(|name| AttributeItem::Group(local_name(name).to_string())),
        _ => None,
    }
}

fn attribute_decl(node: Node) -> AttributeDecl {
    AttributeDecl {
        name: node.attribute("name").unwrap_or_default().to_string(),
        ty: match node.attribute("type") {
            Some(name) => type_ref(node, name),
            None => xs_child(node, "simpleType")
                .map_or(TypeRef::Builtin(Builtin::AnySimpleType), |s| {
                    TypeRef::Simple(Box::new(simple_type(s)))
                }),
        },
        required: node.attribute("use") == Some("required"),
        fixed: node.attribute("fixed").map(str::to_string),
        doc: documentation(node),
        units: units(node),
    }
}
