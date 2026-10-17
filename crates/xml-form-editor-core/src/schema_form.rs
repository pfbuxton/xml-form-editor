//! Builds the form of a schema: the elements and attributes it declares, as the form of a document
//! holding each of them would show them, but without values.
//!
//! Its nodes are the schema's declarations: each element appears once, with how many of it the
//! schema allows there (see [`FormNode::occurs`]), so it has no lists to add to, or matrices. The
//! sections that a selector chooses between are each a [`Variant::Alternative`].

use crate::form::{AttributeField, Field, Form, FormNode, NodeKind, Note, mark_variants};
use crate::schema::{ElementContent, ElementDecl, Occurs, Schema, TypeRef, XS_NS};
use crate::simple::SimpleInfo;
use crate::xml::{Path, XmlError, parse, scan_start_tag};

/// How many elements the form of a schema shows at most: plenty for any real schema, while
/// stopping one that would take too long to show.
const MAX_ELEMENTS: usize = 5000;

/// Whether a document is an XSD: its root is `xs:schema`.
pub fn is_schema(xml: &str) -> Result<bool, XmlError> {
    let doc = parse(xml)?;
    let root = doc.root_element().tag_name();
    Ok(root.namespace() == Some(XS_NS) && root.name() == "schema")
}

/// Builds the form of a schema (see the module's documentation). A schema that can't be read is a
/// form holding why, as the schema's root.
pub fn build_schema_form(xsd: &str) -> Result<Form, XmlError> {
    let doc = parse(xsd)?;
    let element = doc.root_element();
    // The schema's root element as written, such as `xs:schema`
    let name = scan_start_tag(xsd, element.range().start).map_or_else(|| element.tag_name().name().to_string(), |tag| xsd[tag.name].to_string());
    let path = Path::root(element.tag_name().name());
    let schema = match Schema::parse(xsd) {
        Ok(schema) => schema,
        Err(error) => {
            let mut root = FormNode::new(path, name, None);
            root.notes.push(Note::error(error.0));
            return Ok(Form { root, schema_location: None });
        }
    };
    let mut builder = Builder {
        schema: &schema,
        budget: MAX_ELEMENTS,
        ancestors: Vec::new(),
    };
    let roots: Vec<&ElementDecl> = schema.roots().collect();
    let mut root = match roots[..] {
        [decl] => {
            let mut root = builder.element(decl, Path::root(&decl.name), Occurs::ONCE);
            root.doc = root.doc.or_else(|| schema.doc().map(str::to_string));
            root
        }
        // A document can start with any of them, so they're shown in a section for the schema
        _ => {
            let mut root = FormNode::new(path, name, schema.doc().map(str::to_string));
            let children = roots
                .iter()
                .map(|decl| builder.element(decl, root.path.child(&decl.name, 0), Occurs::ONCE))
                .collect();
            root.kind = NodeKind::Section { children };
            root.occurs = Some(Occurs::ONCE);
            root
        }
    };
    root.notes.extend(schema.warnings().iter().map(|warning| Note::warning(warning.clone())));
    if builder.budget == 0 {
        root.notes.push(Note::warning(format!(
            "The schema allows more elements than the {MAX_ELEMENTS} the form shows, so some aren't shown"
        )));
    }
    Ok(Form { root, schema_location: None })
}

struct Builder<'a> {
    schema: &'a Schema,
    /// How many more elements to show
    budget: usize,
    /// The declarations of the sections the element being built is in
    ancestors: Vec<&'a ElementDecl>,
}

impl<'a> Builder<'a> {
    /// The element `decl` declares, at `path`, where the schema allows `occurs` of it.
    fn element(&mut self, decl: &'a ElementDecl, path: Path, occurs: Occurs) -> FormNode {
        self.budget = self.budget.saturating_sub(1);
        let schema = self.schema;
        let mut node = FormNode::new(path, decl.name.clone(), decl.doc.clone().or_else(|| schema.type_doc(&decl.ty)));
        node.occurs = Some(occurs);
        node.units = decl.units.clone();
        node.kind = match schema.content(&decl.ty) {
            ElementContent::Text(info) => NodeKind::Value(declared_field(Some(info), decl.nillable, decl.fixed.clone())),
            ElementContent::Untyped => NodeKind::Value(declared_field(None, decl.nillable, decl.fixed.clone())),
            ElementContent::Empty => NodeKind::Section { children: Vec::new() },
            // A type that holds itself would go on forever
            ElementContent::Elements { .. } if self.ancestors.iter().any(|above| same_type(above, decl)) => {
                node.notes.push(Note::warning(format!(
                    "Holds the same elements as the <{}> it's in, which aren't shown again",
                    decl.name
                )));
                NodeKind::Section { children: Vec::new() }
            }
            ElementContent::Elements { children: decls, .. } => {
                self.ancestors.push(decl);
                let mut children = Vec::new();
                for child in &decls {
                    if self.budget == 0 {
                        break;
                    }
                    let path = node.path.child(&child.decl.name, 0);
                    children.push(self.element(
                        child.decl,
                        path,
                        Occurs {
                            min: child.min,
                            max: child.max,
                        },
                    ));
                }
                self.ancestors.pop();
                mark_variants(&mut children, true);
                NodeKind::Section { children }
            }
        };
        for (attribute, required) in schema.attributes(&decl.ty) {
            // A type can declare an attribute again, as a derived type may
            if node.attributes.iter().any(|a| a.name == attribute.name) {
                continue;
            }
            node.attributes.push(AttributeField {
                name: attribute.name.clone(),
                doc: attribute.doc.clone(),
                field: declared_field(Some(schema.simple_info(&attribute.ty)), false, attribute.fixed.clone()),
                units: attribute.units.clone(),
                warning: None,
                occurs: Some(Occurs {
                    min: u32::from(required),
                    max: Some(1),
                }),
            });
        }
        node
    }
}

/// A value the schema declares, which has no value but the one the schema fixes, if any.
fn declared_field(ty: Option<SimpleInfo>, nillable: bool, fixed: Option<String>) -> Field {
    Field {
        value: fixed.clone().unwrap_or_default(),
        nil: false,
        nillable,
        ty,
        fixed,
        error: None,
    }
}

/// Whether elements that two declarations declare hold the same elements, so that one inside the
/// other holds another, and so on forever.
fn same_type(a: &ElementDecl, b: &ElementDecl) -> bool {
    std::ptr::eq(a, b) || matches!((&a.ty, &b.ty), (TypeRef::Named(a), TypeRef::Named(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::Variant;

    const XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
        <xs:annotation><xs:documentation>Settings</xs:documentation></xs:annotation>
        <xs:simpleType name="Method">
            <xs:restriction base="xs:string">
                <xs:enumeration value="fast"/>
                <xs:enumeration value="exact"/>
            </xs:restriction>
        </xs:simpleType>
        <xs:complexType name="Tree">
            <xs:sequence>
                <xs:element name="branch" type="Tree" minOccurs="0" maxOccurs="2"/>
            </xs:sequence>
        </xs:complexType>
        <xs:element name="part" type="xs:int"/>
        <xs:element name="settings">
            <xs:complexType>
                <xs:sequence>
                    <xs:element name="method" type="Method"/>
                    <xs:element name="fast">
                        <xs:complexType>
                            <xs:sequence>
                                <xs:element name="step" type="xs:double" minOccurs="0"/>
                            </xs:sequence>
                            <xs:attribute name="unit" type="xs:string" fixed="s" use="required"/>
                            <xs:attribute name="note" type="xs:string"/>
                        </xs:complexType>
                    </xs:element>
                    <xs:element name="exact"><xs:complexType/></xs:element>
                    <xs:element name="time" type="xs:double" maxOccurs="unbounded"/>
                    <xs:element ref="part" minOccurs="2" maxOccurs="5"/>
                    <xs:element name="tree" type="Tree"/>
                </xs:sequence>
            </xs:complexType>
        </xs:element>
    </xs:schema>"#;

    fn children(node: &FormNode) -> &[FormNode] {
        match &node.kind {
            NodeKind::Section { children } => children,
            NodeKind::Value(_) => panic!("{} holds elements", node.path),
        }
    }

    #[test]
    fn shows_what_a_schema_declares_without_values() {
        assert_eq!(is_schema(XSD), Ok(true));
        assert_eq!(is_schema("<settings/>"), Ok(false));

        // `part`, which `settings` refers to, isn't a root
        let form = build_schema_form(XSD).unwrap();
        let root = &form.root;
        assert_eq!((root.path.to_string().as_str(), root.doc.as_deref()), ("/settings", Some("Settings")));
        assert_eq!(form.problems().errors + form.problems().warnings, 1, "only the tree's note");
        let shown: Vec<(&str, Option<String>)> = children(root).iter().map(|c| (c.name.as_str(), c.occurs.and_then(Occurs::label))).collect();
        assert_eq!(
            shown,
            [
                ("method", None),
                ("fast", None),
                ("exact", None),
                ("time", Some("1 or more".into())),
                ("part", Some("2 to 5".into())),
                ("tree", None),
            ]
        );

        // Values are empty and valid, but for those the schema fixes
        let [method, fast, exact, time, ..] = children(root) else {
            panic!("six children");
        };
        let NodeKind::Value(field) = &method.kind else {
            panic!("method is a value");
        };
        assert_eq!((field.value.as_str(), &field.error), ("", &None));
        assert!(method.selector);
        assert!(matches!(time.kind, NodeKind::Value(_)) && time.index.is_none());
        assert!(root.lists.is_empty());
        let alternative = |value: &str| Variant::Alternative {
            selector: "method".into(),
            value: value.into(),
        };
        assert_eq!((&fast.variant, &exact.variant), (&alternative("fast"), &alternative("exact")));

        assert_eq!(children(fast)[0].occurs.and_then(Occurs::label).as_deref(), Some("optional"));
        let attributes: Vec<(&str, &str, Option<String>)> = fast
            .attributes
            .iter()
            .map(|a| (a.name.as_str(), a.field.value.as_str(), a.occurs.and_then(Occurs::label)))
            .collect();
        assert_eq!(attributes, [("unit", "s", None), ("note", "", Some("optional".into()))]);

        // A type that holds itself stops where it starts again
        let tree = &children(root)[5];
        let branch = &children(tree)[0];
        assert_eq!(branch.occurs.and_then(Occurs::label).as_deref(), Some("up to 2"));
        assert!(children(branch).is_empty() && branch.notes.len() == 1);
    }

    #[test]
    fn a_schema_with_several_roots_shows_them_all() {
        let xsd = r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema">
            <xsd:element name="a" type="xsd:string"/>
            <xsd:element name="b"><xsd:complexType/></xsd:element>
            <xsd:include schemaLocation="other.xsd"/>
        </xsd:schema>"#;
        let form = build_schema_form(xsd).unwrap();
        assert_eq!((form.root.name.as_str(), form.root.path.to_string().as_str()), ("xsd:schema", "/schema"));
        let names: Vec<String> = children(&form.root).iter().map(|c| c.path.to_string()).collect();
        assert_eq!(names, ["/schema/a", "/schema/b"]);
        assert_eq!(form.problems().warnings, 1, "include isn't supported");

        let form = build_schema_form(r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"/>"#).unwrap();
        assert_eq!(form.root.notes, [Note::error("The schema declares no global elements")]);
        assert!(build_schema_form("<xs:schema").is_err());
    }
}
