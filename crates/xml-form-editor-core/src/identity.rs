//! Identity constraints: `xs:key`, `xs:unique` and `xs:keyref`.
//!
//! A key or unique constraint picks out elements with its selector, such as each `<probe>` in a
//! list, and values of each with its fields, such as the probe's `<name>`. No two elements may have
//! the same values, and under a key, each must have them all. A keyref's values must be those of
//! a key. Each constraint is checked within each element that declares it, as the schema scopes it.
//!
//! Selectors and fields are written in the subset of XPath that XSD allows: paths of child steps,
//! each a name, `*` or `.`, perhaps starting with `.//`, joined by `|`, and for a field, perhaps
//! ending with an attribute, `@name`. Namespaces are ignored. Values are compared as text with its
//! whitespace collapsed, rather than as values of their types, so that `1.0` and `1` differ.

use std::collections::{HashMap, HashSet};

use roxmltree::Node;

use crate::form::Note;
use crate::schema::{Schema, XS_NS};
use crate::xml::{is_nil, local_name};

#[derive(Clone, Debug)]
pub(crate) struct IdentityConstraint {
    pub name: String,
    pub kind: Kind,
    /// The selector and fields, or why they can't be read
    paths: Result<Paths, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Key,
    Unique,
    /// Refers to the key or unique constraint with this name
    Keyref {
        refer: String,
    },
}

impl Kind {
    fn word(&self) -> &'static str {
        match self {
            Kind::Key => "key",
            Kind::Unique => "unique constraint",
            Kind::Keyref { .. } => "keyref",
        }
    }
}

#[derive(Clone, Debug)]
struct Paths {
    selector: Vec<XPath>,
    /// The selector as written
    selector_text: String,
    /// Each field, as written too
    fields: Vec<(Vec<XPath>, String)>,
}

/// A path of a selector or a field. A union of them is a list.
#[derive(Clone, Debug)]
struct XPath {
    /// Starts with `.//`: from the element and everything it holds
    descendants: bool,
    /// Child steps; `None` for `.`
    steps: Vec<Option<NameTest>>,
    /// A field's last step, when it's an attribute
    attribute: Option<NameTest>,
}

#[derive(Clone, Debug)]
enum NameTest {
    Any,
    Name(String),
}

impl NameTest {
    fn parse(text: &str) -> Option<NameTest> {
        if text == "*" || text.ends_with(":*") {
            return Some(NameTest::Any);
        }
        let name = local_name(text);
        let valid = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
        valid.then(|| NameTest::Name(name.to_string()))
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            NameTest::Any => true,
            NameTest::Name(wanted) => wanted == name,
        }
    }
}

impl IdentityConstraint {
    /// Reads an `xs:key`, `xs:unique` or `xs:keyref`; `None` for anything else.
    pub(crate) fn parse(node: Node) -> Option<IdentityConstraint> {
        let kind = match node.tag_name().name() {
            "key" => Kind::Key,
            "unique" => Kind::Unique,
            "keyref" => Kind::Keyref {
                refer: local_name(node.attribute("refer")?).to_string(),
            },
            _ => return None,
        };
        let xpaths = |name: &str| -> Vec<String> {
            node.children()
                .filter(|c| c.is_element() && c.tag_name().namespace() == Some(XS_NS) && c.tag_name().name() == name)
                .map(|c| c.attribute("xpath").unwrap_or_default().trim().to_string())
                .collect()
        };
        let paths = (|| {
            let selector_text = xpaths("selector").into_iter().next().ok_or("it has no selector")?;
            let fields = xpaths("field")
                .into_iter()
                .map(|text| Ok((parse_union(&text, true)?, text)))
                .collect::<Result<Vec<_>, String>>()?;
            if fields.is_empty() {
                return Err("it has no fields".to_string());
            }
            Ok(Paths {
                selector: parse_union(&selector_text, false)?,
                selector_text,
                fields,
            })
        })();
        Some(IdentityConstraint {
            name: node.attribute("name").unwrap_or_default().to_string(),
            kind,
            paths,
        })
    }
}

fn parse_union(text: &str, field: bool) -> Result<Vec<XPath>, String> {
    text.split('|')
        .map(|path| parse_path(path.trim(), field).ok_or_else(|| format!("the form can't read its XPath, {text}")))
        .collect()
}

fn parse_path(path: &str, field: bool) -> Option<XPath> {
    let (descendants, rest) = match path.strip_prefix(".//") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, path),
    };
    let parts: Vec<&str> = rest.split('/').map(str::trim).collect();
    let mut steps = Vec::new();
    let mut attribute = None;
    for (i, part) in parts.iter().enumerate() {
        if let Some(name) = part.strip_prefix('@').or_else(|| part.strip_prefix("attribute::")) {
            // Only a field's last step may be an attribute
            if !field || i + 1 != parts.len() {
                return None;
            }
            attribute = Some(NameTest::parse(name.trim())?);
        } else if *part == "." {
            steps.push(None);
        } else {
            steps.push(Some(NameTest::parse(part.strip_prefix("child::").unwrap_or(part).trim())?));
        }
    }
    Some(XPath { descendants, steps, attribute })
}

impl XPath {
    /// The elements the path's steps select from `from`, before any attribute.
    fn elements<'a, 'input>(&self, from: Node<'a, 'input>) -> Vec<Node<'a, 'input>> {
        let mut nodes: Vec<Node> = if self.descendants {
            from.descendants().filter(Node::is_element).collect()
        } else {
            vec![from]
        };
        for step in self.steps.iter().flatten() {
            nodes = nodes
                .iter()
                .flat_map(|node| node.children().filter(|c| c.is_element() && step.matches(c.tag_name().name())))
                .collect();
        }
        nodes
    }
}

/// The elements a union of paths selects, in document order, each once.
fn select<'a, 'input>(paths: &[XPath], from: Node<'a, 'input>) -> Vec<Node<'a, 'input>> {
    let mut nodes: Vec<Node> = paths.iter().flat_map(|path| path.elements(from)).collect();
    nodes.sort_by_key(|node| node.range().start);
    nodes.dedup();
    nodes
}

/// A field's value in an element the selector picked.
enum Field<'a, 'input> {
    /// With the element it's in, or whose attribute it is
    Value(String, Node<'a, 'input>),
    Missing,
    /// It matches more than one element or attribute
    Several,
}

fn field<'a, 'input>(paths: &[XPath], target: Node<'a, 'input>) -> Field<'a, 'input> {
    let mut found = Vec::new();
    for path in paths {
        for element in path.elements(target) {
            match &path.attribute {
                Some(test) => found.extend(element.attributes().filter(|a| test.matches(a.name())).map(|a| (collapse(a.value()), element))),
                // A nil element has no value
                None if is_nil(element) => {}
                None => {
                    let text: String = element.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
                    found.push((collapse(&text), element));
                }
            }
        }
    }
    // Paths of a union may select the same element
    let mut seen = HashSet::new();
    found.retain(|(value, node)| seen.insert((node.id(), value.clone())));
    match found.len() {
        0 => Field::Missing,
        1 => {
            let (value, node) = found.remove(0);
            Field::Value(value, node)
        }
        _ => Field::Several,
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An element a constraint's selector picked, with its fields' values.
struct Row<'a, 'input> {
    target: Node<'a, 'input>,
    fields: Vec<Field<'a, 'input>>,
}

impl<'a, 'input> Row<'a, 'input> {
    /// Its values, when it has every field once.
    fn values(&self) -> Option<Vec<String>> {
        self.fields
            .iter()
            .map(|f| match f {
                Field::Value(value, _) => Some(value.clone()),
                _ => None,
            })
            .collect()
    }

    /// Where to show a problem with its values: the element holding its only field, or itself.
    fn place(&self) -> Node<'a, 'input> {
        match self.fields.as_slice() {
            [Field::Value(_, node)] => *node,
            _ => self.target,
        }
    }
}

impl Paths {
    fn rows<'a, 'input>(&self, element: Node<'a, 'input>) -> Vec<Row<'a, 'input>> {
        select(&self.selector, element)
            .into_iter()
            .map(|target| Row {
                target,
                fields: self.fields.iter().map(|(paths, _)| field(paths, target)).collect(),
            })
            .collect()
    }

    /// What the selector picks, for messages: the name its first path ends with, as in `probe`.
    fn target(&self) -> String {
        match self.selector.first().and_then(|path| path.steps.iter().flatten().last()) {
            Some(NameTest::Name(name)) => name.clone(),
            _ => format!("element picked by {}", self.selector_text),
        }
    }

    /// What the fields are, for messages: `name`, or `name and kind`; `value` for `.`.
    fn describe(&self) -> String {
        let names: Vec<&str> = self
            .fields
            .iter()
            .map(|(_, text)| match text.trim_start_matches("./").trim_start_matches('@') {
                "." | "" => "value",
                name => name,
            })
            .collect();
        and_list(&names)
    }
}

/// A problem with an identity constraint, and the element it's about.
pub(crate) struct Problem<'a, 'input> {
    pub node: Node<'a, 'input>,
    pub note: Note,
    /// For a missing field that's a child element, its name: the element may be missing already
    /// for the schema's content model, which says so
    pub missing: Option<String>,
}

/// Checks the identity constraints that `element`'s declaration has.
pub(crate) fn check<'a, 'input>(element: Node<'a, 'input>, constraints: &[IdentityConstraint], schema: &Schema) -> Vec<Problem<'a, 'input>> {
    let mut problems = Vec::new();
    for constraint in constraints {
        let paths = match &constraint.paths {
            Ok(paths) => paths,
            Err(why) => {
                problems.push(Problem {
                    node: element,
                    note: Note::warning(format!("The {} {} isn't checked, as {why}", constraint.kind.word(), constraint.name)),
                    missing: None,
                });
                continue;
            }
        };
        let rows = paths.rows(element);
        for row in &rows {
            for (value, (_, text)) in row.fields.iter().zip(&paths.fields) {
                match value {
                    Field::Several => problems.push(Problem {
                        node: row.target,
                        note: Note::error(format!(
                            "Has more than one {text}, but the {} {} needs one",
                            constraint.kind.word(),
                            constraint.name
                        )),
                        missing: None,
                    }),
                    Field::Missing if constraint.kind == Kind::Key => {
                        let child = child_name(text);
                        let what = match (&child, text.strip_prefix('@')) {
                            (Some(name), _) => format!("<{name}>"),
                            (None, Some(attribute)) => format!("the {attribute} attribute"),
                            (None, None) => text.clone(),
                        };
                        problems.push(Problem {
                            node: row.target,
                            note: Note::error(format!("Missing {what}, which the key {} needs", constraint.name)),
                            missing: child,
                        });
                    }
                    _ => {}
                }
            }
        }
        match &constraint.kind {
            Kind::Key | Kind::Unique => problems.extend(duplicates(element, &rows, paths)),
            Kind::Keyref { refer } => problems.extend(dangling(element, &rows, constraint, refer, schema)),
        }
    }
    problems
}

/// The name of the child element a field selects, when that's all it does, as in `name`.
fn child_name(text: &str) -> Option<String> {
    match parse_path(text, true)? {
        XPath {
            descendants: false,
            steps,
            attribute: None,
        } => match steps.as_slice() {
            [Some(NameTest::Name(name))] => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The elements whose values another's repeat.
fn duplicates<'a, 'input>(scope: Node<'a, 'input>, rows: &[Row<'a, 'input>], paths: &Paths) -> Vec<Problem<'a, 'input>> {
    let values: Vec<Option<Vec<String>>> = rows.iter().map(Row::values).collect();
    let mut by_value: HashMap<&Vec<String>, Vec<usize>> = HashMap::new();
    for (i, value) in values.iter().enumerate() {
        if let Some(value) = value {
            by_value.entry(value).or_default().push(i);
        }
    }
    let mut problems = Vec::new();
    for (i, value) in values.iter().enumerate() {
        let Some(same) = value.as_ref().and_then(|value| by_value.get(value)).filter(|same| same.len() > 1) else {
            continue;
        };
        let others: Vec<String> = same.iter().filter(|&&j| j != i).map(|&j| label(scope, rows[j].target)).collect();
        let verb = if others.len() == 1 { "has" } else { "have" };
        problems.push(Problem {
            node: rows[i].place(),
            note: Note::error(format!(
                "Must be unique, but {} {verb} the same {} too",
                and_list(&others.iter().map(String::as_str).collect::<Vec<_>>()),
                paths.describe()
            )),
            missing: None,
        });
    }
    problems
}

/// The elements whose values match none of the key a keyref refers to.
fn dangling<'a, 'input>(
    element: Node<'a, 'input>,
    rows: &[Row<'a, 'input>],
    constraint: &IdentityConstraint,
    refer: &str,
    schema: &Schema,
) -> Vec<Problem<'a, 'input>> {
    let unchecked = |why: String| Problem {
        node: element,
        note: Note::warning(format!("The keyref {} isn't checked, as {why}", constraint.name)),
        missing: None,
    };
    let Some((holder, key)) = schema.identity_constraint(refer) else {
        return vec![unchecked(format!("the schema doesn't declare the key {refer} it refers to"))];
    };
    let Ok(key_paths) = &key.paths else {
        return vec![unchecked(format!("the form can't read the key {refer}"))];
    };
    // The key's values, from where it's declared in this element, or in what this element holds
    let keys: HashSet<Vec<String>> = element
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == holder)
        .flat_map(|node| key_paths.rows(node))
        .filter_map(|row| row.values())
        .collect();
    rows.iter()
        .filter_map(|row| {
            let values = row.values()?;
            (!keys.contains(&values)).then(|| Problem {
                node: row.place(),
                note: Note::error(format!(
                    "There's no {} with the {} {}",
                    key_paths.target(),
                    key_paths.describe(),
                    values.join(", ")
                )),
                missing: None,
            })
        })
        .collect()
}

/// An element in `scope` as the form labels it, as in `probe [2]`, from `scope` down when it's
/// deeper, as in `group [1] / coil [0]`.
fn label(scope: Node, node: Node) -> String {
    let mut steps: Vec<String> = node.ancestors().take_while(|n| *n != scope).map(step_label).collect();
    steps.reverse();
    steps.join(" / ")
}

/// An element's name, and its position among its siblings of that name when it has any.
fn step_label(node: Node) -> String {
    let name = node.tag_name().name();
    let same = |n: &Node| n.is_element() && n.tag_name().name() == name;
    // The sibling axes start at the node itself
    let index = node.prev_siblings().skip(1).filter(same).count();
    let others = index + node.next_siblings().skip(1).filter(same).count();
    if others == 0 { name.to_string() } else { format!("{name} [{index}]") }
}

/// `a`, `a and b`, or `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => only.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Form, FormNode, NodeKind, Schema, build_form};

    const XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
        <xs:element name="plant">
            <xs:complexType>
                <xs:sequence>
                    <xs:element name="group" maxOccurs="unbounded">
                        <xs:complexType>
                            <xs:sequence>
                                <xs:element name="coil" minOccurs="0" maxOccurs="unbounded">
                                    <xs:complexType><xs:attribute name="id" type="xs:string"/></xs:complexType>
                                </xs:element>
                            </xs:sequence>
                        </xs:complexType>
                    </xs:element>
                    <xs:element name="circuit" minOccurs="0" maxOccurs="unbounded">
                        <xs:complexType><xs:attribute name="coil" type="xs:string"/></xs:complexType>
                    </xs:element>
                </xs:sequence>
            </xs:complexType>
            <xs:key name="coil_id">
                <xs:selector xpath=".//coil"/>
                <xs:field xpath="@id"/>
            </xs:key>
            <xs:keyref name="circuit_coil" refer="coil_id">
                <xs:selector xpath="circuit"/>
                <xs:field xpath="@coil"/>
            </xs:keyref>
            <xs:unique name="unreadable">
                <xs:selector xpath="group[1]"/>
                <xs:field xpath="."/>
            </xs:unique>
        </xs:element>
    </xs:schema>"#;

    fn notes(xml: &str) -> Vec<String> {
        fn walk(node: &FormNode, out: &mut Vec<String>) {
            out.extend(node.notes.iter().map(|n| format!("{}: {}", node.path, n.message)));
            if let NodeKind::Section { children } = &node.kind {
                children.iter().for_each(|c| walk(c, out));
            }
        }
        let form: Form = build_form(xml, Some(&Schema::parse(XSD).unwrap())).unwrap();
        let mut out = Vec::new();
        walk(&form.root, &mut out);
        out
    }

    #[test]
    fn keys_are_found_anywhere_below_and_keyrefs_must_match_one() {
        let xml = r#"<plant>
            <group><coil id="a"/><coil id="b"/></group>
            <group><coil id="a"/><coil/></group>
            <circuit coil="b"/>
            <circuit coil="z"/>
        </plant>"#;
        assert_eq!(
            notes(xml),
            [
                "/plant: The unique constraint unreadable isn't checked, as the form can't read its XPath, group[1]",
                "/plant/group/coil: Must be unique, but group [1] / coil [0] has the same id too",
                "/plant/group[2]/coil: Must be unique, but group [0] / coil [0] has the same id too",
                "/plant/group[2]/coil[2]: Missing the id attribute, which the key coil_id needs",
                "/plant/circuit[2]: There's no coil with the id z",
            ]
        );
    }
}
