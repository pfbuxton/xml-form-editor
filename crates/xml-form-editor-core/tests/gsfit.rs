//! Tests against an earlier version of GSFit's settings, kept in `tests/data`: the numerics, the
//! time-slices, and the overall settings that link to both.

use xml_form_editor_core::{
    EditOp, Form, FormNode, NodeKind, Occurs, Schema, Severity, TextEdit, Variant, apply_edit, build_form, build_schema_form, is_schema, values,
};

const XML: &str = include_str!("data/numerics.xml");
const XSD: &str = include_str!("data/numerics.xsd");
const TIME_SLICES_XML: &str = include_str!("data/time_slices.xml");
const TIME_SLICES_XSD: &str = include_str!("data/time_slices.xsd");
const OVERALL_XML: &str = include_str!("data/overall_code_settings.xml");
const OVERALL_XSD: &str = include_str!("data/overall_code_settings.xsd");

/// A time-slices file of its own, to test the lists in.
const TIME_SLICES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<time_slices xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="time_slices.xsd">
    <method>times</method>
    <times>
        <time>0.050</time>
        <time>0.100</time>
        <time>0.150</time>
        <time>0.200</time>
    </times>
    <regular>
        <time_start>0.0</time_start>
        <time_end>0.3</time_end>
        <time_step>5.0e-3</time_step>
    </regular>
    <averaging_window>1.0e-3</averaging_window>
    <initial_guess>previous</initial_guess>
    <stop_on_failure>false</stop_on_failure>
    <override>
        <time_start>0.0</time_start>
        <time_end>0.02</time_end>
        <n_iter_max>500</n_iter_max>
        <grad_shafranov_deviation_tolerance>1.0e-4</grad_shafranov_deviation_tolerance>
    </override>
</time_slices>
"#;

fn form(xml: &str) -> Form {
    let schema = Schema::parse(XSD).unwrap();
    build_form(xml, Some(&schema)).unwrap()
}

fn find<'a>(node: &'a FormNode, path: &str) -> Option<&'a FormNode> {
    if node.path.to_string() == path {
        return Some(node);
    }
    match &node.kind {
        NodeKind::Section { children } => children.iter().find_map(|c| find(c, path)),
        NodeKind::Value(_) => None,
    }
}

fn node<'a>(form: &'a Form, path: &str) -> &'a FormNode {
    find(&form.root, path).unwrap_or_else(|| panic!("{path} is in the form"))
}

fn field<'a>(form: &'a Form, path: &str) -> &'a xml_form_editor_core::Field {
    match &node(form, path).kind {
        NodeKind::Value(field) => field,
        NodeKind::Section { .. } => panic!("{path} is a value"),
    }
}

/// Every problem in the form, as "path: message".
fn problems(node: &FormNode, out: &mut Vec<String>) {
    for note in &node.notes {
        out.push(format!("{}: {:?} {}", node.path, note.severity, note.message));
    }
    for attribute in &node.attributes {
        out.extend(attribute.field.error.iter().map(|e| format!("{}/@{}: {e}", node.path, attribute.name)));
        out.extend(attribute.warning.iter().map(|w| format!("{}/@{}: {w}", node.path, attribute.name)));
    }
    match &node.kind {
        NodeKind::Value(field) => out.extend(field.error.iter().map(|e| format!("{}: {e}", node.path))),
        NodeKind::Section { children } => children.iter().for_each(|c| problems(c, out)),
    }
}

/// Checks that a document is valid against its schema, and returns its form.
fn valid(xml: &str, xsd: &str) -> Form {
    let schema = Schema::parse(xsd).unwrap();
    assert!(schema.warnings().is_empty(), "{:?}", schema.warnings());
    let form = build_form(xml, Some(&schema)).unwrap();
    let mut found = Vec::new();
    problems(&form.root, &mut found);
    assert!(found.is_empty(), "unexpected problems:\n{}", found.join("\n"));
    assert_eq!(form.problems().errors + form.problems().warnings, 0);
    form
}

/// Makes an edit to a document, as VS Code would.
fn edit(xml: &str, op: &EditOp) -> String {
    let edits = apply_edit(xml, op).unwrap();
    assert!(xml.is_ascii(), "UTF-16 offsets are byte offsets");
    let mut out = xml.to_string();
    for TextEdit { start, end, text } in edits.iter().rev() {
        out.replace_range(*start as usize..*end as usize, text);
    }
    out
}

#[test]
fn the_examples_are_valid() {
    let form = valid(XML, XSD);
    assert_eq!(form.schema_location.as_deref(), Some("numerics.xsd"));
    valid(TIME_SLICES_XML, TIME_SLICES_XSD);
    let overall = valid(OVERALL_XML, OVERALL_XSD);
    assert_eq!(overall.links(), ["numerics.xml", "time_slices.xml"]);
}

/// The paths of the elements in a form, in order.
fn paths(node: &FormNode, out: &mut Vec<String>) {
    out.push(node.path.to_string());
    if let NodeKind::Section { children } = &node.kind {
        children.iter().for_each(|c| paths(c, out));
    }
}

#[test]
fn the_schemas_show_as_forms_of_what_they_declare() {
    for (xml, xsd) in [(XML, XSD), (TIME_SLICES_XML, TIME_SLICES_XSD), (OVERALL_XML, OVERALL_XSD)] {
        assert_eq!((is_schema(xsd), is_schema(xml)), (Ok(true), Ok(false)));
        let mut found = Vec::new();
        problems(&build_schema_form(xsd).unwrap().root, &mut found);
        assert!(found.is_empty(), "unexpected problems:\n{}", found.join("\n"));
    }

    // numerics.xml has each element its schema declares, once, so the forms show the same elements
    let declared = build_schema_form(XSD).unwrap();
    let (mut shown, mut expected) = (Vec::new(), Vec::new());
    paths(&declared.root, &mut shown);
    paths(&form(XML).root, &mut expected);
    assert_eq!(shown, expected);
    assert!(field(&declared, "/numerics/n_iter_max").value.is_empty());
    assert!(node(&declared, "/numerics/nonlinear_solver/method").selector);
    assert_eq!(
        node(&declared, "/numerics/nonlinear_solver/newton_krylov").variant,
        Variant::Alternative {
            selector: "method".into(),
            value: "newton_krylov".into()
        }
    );

    let declared = build_schema_form(TIME_SLICES_XSD).unwrap();
    let occurs = |path| node(&declared, path).occurs.and_then(Occurs::label);
    assert_eq!(occurs("/time_slices/times/time").as_deref(), Some("1 or more"));
    assert_eq!(occurs("/time_slices/override").as_deref(), Some("0 or more"));
    assert_eq!(occurs("/time_slices/method"), None);

    let declared = build_schema_form(OVERALL_XSD).unwrap();
    let numerics = node(&declared, "/overall_code_settings/numerics");
    assert_eq!(numerics.attributes[0].name, "href");
    assert_eq!(numerics.link, None, "a schema links to no files");
}

#[test]
fn time_slices_are_lists_to_add_to_and_remove_from() {
    let xml = TIME_SLICES;
    valid(xml, TIME_SLICES_XSD);
    let schema = Schema::parse(TIME_SLICES_XSD).unwrap();
    let form = build_form(xml, Some(&schema)).unwrap();
    let lists = |node: &FormNode| -> Vec<(String, usize)> { node.lists.iter().map(|l| (l.name.clone(), l.count)).collect() };
    assert_eq!(lists(&form.root), [("override".into(), 1), ("exclude".into(), 0)]);
    assert_eq!(lists(node(&form, "/time_slices/times")), [("time".into(), 4)]);

    // The first exclude, after the override
    let exclude = form.root.lists[1].new_item.clone().unwrap();
    let added = edit(
        xml,
        &EditOp::InsertElement {
            path: form.root.path.clone(),
            after: exclude.follows,
            element: exclude.element,
        },
    );
    assert_eq!(
        added,
        xml.replace(
            "    </override>\n",
            "    </override>\n    <exclude>\n        <time_start>0.0</time_start>\n        <time_end>0.0</time_end>\n    </exclude>\n"
        )
    );
    valid(&added, TIME_SLICES_XSD);

    // Another override, a copy of the first, then without the first
    let first = form.root.path.child("override", 0);
    let copied = edit(xml, &EditOp::InsertCopy { path: first.clone() });
    let form = build_form(&copied, Some(&schema)).unwrap();
    assert_eq!(form.root.lists[0].count, 2);
    let removed = edit(&copied, &EditOp::RemoveElement { path: first });
    assert_eq!(&removed, xml);
    valid(&removed, TIME_SLICES_XSD);
}

#[test]
fn enumerations_list_their_values_and_documentation() {
    let form = form(XML);
    let method = field(&form, "/numerics/nonlinear_solver/method");
    assert_eq!(method.value, "picard");
    let ty = method.ty.as_ref().unwrap();
    let values: Vec<&str> = ty.enumeration.iter().map(|e| e.value.as_str()).collect();
    assert_eq!(values, ["picard", "newton_krylov", "newton_picard"]);
    assert_eq!(ty.enumeration[1].doc.as_deref(), Some("Jacobian-free Newton-Krylov"));
    assert_eq!(ty.summary(), "NonlinearSolverMethod");
    assert_eq!(
        node(&form, "/numerics/n_iter_min").doc.as_deref(),
        Some("Minimum number of nonlinear solver iterations before the convergence test may pass")
    );
}

#[test]
fn types_come_from_the_schema() {
    let form = form(XML);
    let ty = |path: &str| field(&form, path).ty.clone().unwrap();
    assert_eq!(
        ty("/numerics/nonlinear_solver/picard/anderson_mixing").summary(),
        "PositiveDouble: double > 0.0"
    );
    assert_eq!(ty("/numerics/grad_shafranov_deviation_tolerance").summary(), "double");
    assert!(ty("/numerics/save_unconverged").is_boolean());
    assert_eq!(ty("/numerics/RAYON_NUM_THREADS").summary(), "nonNegativeInteger");
}

#[test]
fn methods_choose_which_sections_are_used() {
    let form = form(XML);
    let variant = |path: &str| node(&form, path).variant.clone();
    let solver = "/numerics/nonlinear_solver";
    let unselected = Variant::Unselected {
        selector: "method".into(),
        value: "picard".into(),
    };

    assert!(node(&form, &format!("{solver}/method")).selector);
    assert_eq!(variant(&format!("{solver}/picard")), Variant::Selected { selector: "method".into() });
    assert_eq!(variant(&format!("{solver}/newton_krylov")), unselected);
    assert_eq!(variant(&format!("{solver}/newton_picard")), unselected);
    assert_eq!(variant(solver), Variant::Plain);

    // Without the section the method names, none of them is used
    let start = XML.find("<picard>").unwrap();
    let end = XML.find("</picard>").unwrap() + "</picard>".len();
    let form = self::form(&format!("{}{}", &XML[..start], &XML[end..]));
    assert_eq!(node(&form, &format!("{solver}/newton_krylov")).variant, unselected);
    assert_eq!(node(&form, &format!("{solver}/newton_picard")).variant, unselected);
}

#[test]
fn changing_a_method_changes_one_line_and_the_section_in_use() {
    let method = form(XML).root.path.child("nonlinear_solver", 0).child("method", 0);
    let edits = apply_edit(
        XML,
        &EditOp::SetValue {
            path: method,
            value: "newton_krylov".into(),
        },
    )
    .unwrap();
    assert_eq!(edits.len(), 1);
    let edited = format!("{}{}{}", &XML[..edits[0].start as usize], edits[0].text, &XML[edits[0].end as usize..]);

    let changed: Vec<(&str, &str)> = XML.lines().zip(edited.lines()).filter(|(a, b)| a != b).collect();
    assert_eq!(changed, [("        <method>picard</method>", "        <method>newton_krylov</method>")]);

    let form = form(&edited);
    assert_eq!(
        node(&form, "/numerics/nonlinear_solver/newton_krylov").variant,
        Variant::Selected { selector: "method".into() }
    );
    assert!(matches!(node(&form, "/numerics/nonlinear_solver/picard").variant, Variant::Unselected { .. }));
}

#[test]
fn reports_invalid_values_and_missing_elements() {
    let xml = XML
        .replace("<anderson_n_history>5</anderson_n_history>", "<anderson_n_history>-3</anderson_n_history>")
        .replace("<n_iter_no_vertical_feedback>1</n_iter_no_vertical_feedback>", "");
    let form = form(&xml);
    assert_eq!(
        field(&form, "/numerics/nonlinear_solver/picard/anderson_n_history").error.as_deref(),
        Some("Must be a whole number, 0 or more")
    );
    let picard = node(&form, "/numerics/nonlinear_solver/picard");
    assert_eq!(picard.notes.len(), 1);
    assert_eq!(picard.notes[0].severity, Severity::Error);
    assert!(picard.notes[0].message.starts_with("Missing <n_iter_no_vertical_feedback>"));
    assert_eq!(form.problems().errors, 2);
}

#[test]
fn works_without_a_schema() {
    let form = build_form(XML, None).unwrap();
    let method = field(&form, "/numerics/nonlinear_solver/method");
    assert!(method.ty.is_none());
    assert_eq!(method.value, "picard");
    assert_eq!(form.problems().errors, 0);
}

#[test]
fn units_come_from_appinfo() {
    let form = form(XML);
    let units = |path: &str| node(&form, path).units.clone();
    assert_eq!(units("/numerics/grad_shafranov_deviation_tolerance").as_deref(), Some("1"));
    assert_eq!(units("/numerics/nonlinear_solver/picard/anderson_mixing").as_deref(), Some("1"));
    assert_eq!(units("/numerics/n_iter_max"), None);
    assert_eq!(
        node(&form, "/numerics/nonlinear_solver/newton_krylov/krylov_tolerance").doc.as_deref(),
        Some("The Krylov solve stops once the linearised residual is this fraction of the residual")
    );
}

#[test]
fn values_show_what_changed() {
    let before = values(XML).unwrap();
    let edited = XML.replace("<n_iter_max>100</n_iter_max>", "<n_iter_max>200</n_iter_max>");
    let after = values(&edited).unwrap();
    let changed: Vec<&String> = after.keys().filter(|key| after[*key] != before[*key]).collect();
    assert_eq!(changed, ["/numerics/n_iter_max"]);
    assert_eq!(after["/numerics/n_iter_max"].as_deref(), Some("200"));
    // Nil is its own value
    let nil = XML.replace("<n_iter_min>0</n_iter_min>", r#"<n_iter_min xsi:nil="true"/>"#);
    assert_eq!(values(&nil).unwrap()["/numerics/n_iter_min"], None);
}
