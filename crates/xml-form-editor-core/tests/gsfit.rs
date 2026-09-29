//! Tests against GSFit's numerics settings in `example_data`.

use xml_form_editor_core::{
    EditOp, Form, FormNode, NodeKind, Schema, Severity, Variant, apply_edit, build_form, values,
};

const XML: &str = include_str!("../../../example_data/numerics.xml");
const XSD: &str = include_str!("../../../example_data/numerics.xsd");

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
        out.push(format!(
            "{}: {:?} {}",
            node.path, note.severity, note.message
        ));
    }
    for attribute in &node.attributes {
        out.extend(
            attribute
                .field
                .error
                .iter()
                .map(|e| format!("{}/@{}: {e}", node.path, attribute.name)),
        );
        out.extend(
            attribute
                .warning
                .iter()
                .map(|w| format!("{}/@{}: {w}", node.path, attribute.name)),
        );
    }
    match &node.kind {
        NodeKind::Value(field) => {
            out.extend(field.error.iter().map(|e| format!("{}: {e}", node.path)))
        }
        NodeKind::Section { children } => children.iter().for_each(|c| problems(c, out)),
    }
}

#[test]
fn the_example_is_valid() {
    let schema = Schema::parse(XSD).unwrap();
    assert!(schema.warnings().is_empty(), "{:?}", schema.warnings());
    let form = build_form(XML, Some(&schema)).unwrap();
    assert_eq!(form.schema_location.as_deref(), Some("numerics.xsd"));
    let mut found = Vec::new();
    problems(&form.root, &mut found);
    assert!(
        found.is_empty(),
        "unexpected problems:\n{}",
        found.join("\n")
    );
    assert_eq!(form.problems().errors + form.problems().warnings, 0);
}

#[test]
fn enumerations_list_their_values_and_documentation() {
    let form = form(XML);
    let method = field(&form, "/numerics/nonlinear_solver/method");
    assert_eq!(method.value, "picard");
    let ty = method.ty.as_ref().unwrap();
    let values: Vec<&str> = ty.enumeration.iter().map(|e| e.value.as_str()).collect();
    assert_eq!(values, ["picard", "newton_krylov", "newton_picard"]);
    assert_eq!(
        ty.enumeration[1].doc.as_deref(),
        Some("Jacobian-free Newton-Krylov")
    );
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
    assert_eq!(
        ty("/numerics/grad_shafranov_deviation_tolerance").summary(),
        "double"
    );
    assert!(ty("/numerics/save_unconverged").is_boolean());
    assert_eq!(
        ty("/numerics/RAYON_NUM_THREADS").summary(),
        "nonNegativeInteger"
    );
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
    assert_eq!(
        variant(&format!("{solver}/picard")),
        Variant::Selected {
            selector: "method".into()
        }
    );
    assert_eq!(variant(&format!("{solver}/newton_krylov")), unselected);
    assert_eq!(variant(&format!("{solver}/newton_picard")), unselected);
    assert_eq!(variant(solver), Variant::Plain);

    // Without the section the method names, none of them is used
    let start = XML.find("<picard>").unwrap();
    let end = XML.find("</picard>").unwrap() + "</picard>".len();
    let form = self::form(&format!("{}{}", &XML[..start], &XML[end..]));
    assert_eq!(
        node(&form, &format!("{solver}/newton_krylov")).variant,
        unselected
    );
    assert_eq!(
        node(&form, &format!("{solver}/newton_picard")).variant,
        unselected
    );
}

#[test]
fn changing_a_method_changes_one_line_and_the_section_in_use() {
    let method = form(XML)
        .root
        .path
        .child("nonlinear_solver", 0)
        .child("method", 0);
    let edits = apply_edit(
        XML,
        &EditOp::SetValue {
            path: method,
            value: "newton_krylov".into(),
        },
    )
    .unwrap();
    assert_eq!(edits.len(), 1);
    let edited = format!(
        "{}{}{}",
        &XML[..edits[0].start as usize],
        edits[0].text,
        &XML[edits[0].end as usize..]
    );

    let changed: Vec<(&str, &str)> = XML
        .lines()
        .zip(edited.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(
        changed,
        [(
            "        <method>picard</method>",
            "        <method>newton_krylov</method>"
        )]
    );

    let form = form(&edited);
    assert_eq!(
        node(&form, "/numerics/nonlinear_solver/newton_krylov").variant,
        Variant::Selected {
            selector: "method".into()
        }
    );
    assert!(matches!(
        node(&form, "/numerics/nonlinear_solver/picard").variant,
        Variant::Unselected { .. }
    ));
}

#[test]
fn reports_invalid_values_and_missing_elements() {
    let xml = XML
        .replace(
            "<anderson_n_history>5</anderson_n_history>",
            "<anderson_n_history>-3</anderson_n_history>",
        )
        .replace(
            "<n_iter_no_vertical_feedback>1</n_iter_no_vertical_feedback>",
            "",
        );
    let form = form(&xml);
    assert_eq!(
        field(
            &form,
            "/numerics/nonlinear_solver/picard/anderson_n_history"
        )
        .error
        .as_deref(),
        Some("Must be a whole number, 0 or more")
    );
    let picard = node(&form, "/numerics/nonlinear_solver/picard");
    assert_eq!(picard.notes.len(), 1);
    assert_eq!(picard.notes[0].severity, Severity::Error);
    assert!(
        picard.notes[0]
            .message
            .starts_with("Missing <n_iter_no_vertical_feedback>")
    );
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
    assert_eq!(
        units("/numerics/grad_shafranov_deviation_tolerance").as_deref(),
        Some("1")
    );
    assert_eq!(
        units("/numerics/nonlinear_solver/picard/anderson_mixing").as_deref(),
        Some("1")
    );
    assert_eq!(units("/numerics/n_iter_max"), None);
    assert_eq!(
        node(
            &form,
            "/numerics/nonlinear_solver/newton_krylov/krylov_tolerance"
        )
        .doc
        .as_deref(),
        Some(
            "The Krylov solve stops once the linearised residual is this fraction of the residual"
        )
    );
}

#[test]
fn values_show_what_changed() {
    let before = values(XML).unwrap();
    let edited = XML.replace(
        "<n_iter_max>100</n_iter_max>",
        "<n_iter_max>200</n_iter_max>",
    );
    let after = values(&edited).unwrap();
    let changed: Vec<&String> = after
        .keys()
        .filter(|key| after[*key] != before[*key])
        .collect();
    assert_eq!(changed, ["/numerics/n_iter_max"]);
    assert_eq!(after["/numerics/n_iter_max"].as_deref(), Some("200"));
    // Nil is its own value
    let nil = XML.replace(
        "<n_iter_min>0</n_iter_min>",
        r#"<n_iter_min xsi:nil="true"/>"#,
    );
    assert_eq!(values(&nil).unwrap()["/numerics/n_iter_min"], None);
}
