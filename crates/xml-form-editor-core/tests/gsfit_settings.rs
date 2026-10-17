//! Tests against GSFit's settings in `example_data`: every file is valid for its schema, as the form
//! checks it; the schemas' own types accept and refuse what they should; and their keys and unique
//! constraints hold.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use xml_form_editor_core::{Form, FormNode, NodeKind, Schema, Severity, SimpleInfo, build_form, build_schema_form, schema_location};

const SETTINGS: &str = "settings/st40_P2p3_calibrations_new_xml_schema";

fn example_data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example_data")
}

/// The settings files, by their path in the settings directory.
fn settings_files() -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "xml") {
                out.push(path);
            }
        }
    }
    let root = example_data().join(SETTINGS);
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| (path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), path))
        .collect()
}

/// The form of a settings file, with the schema it names.
fn settings_form(xml: &str, path: &Path) -> Form {
    let location = schema_location(xml).unwrap().expect("the file names its schema");
    let xsd = fs::read_to_string(path.parent().unwrap().join(location)).unwrap();
    build_form(xml, Some(&Schema::parse(&xsd).unwrap())).unwrap()
}

/// The form of the settings file at this path in the settings directory, with its text changed.
fn changed_form(file: &str, change: impl Fn(&str) -> String) -> Form {
    let path = example_data().join(SETTINGS).join(file);
    let xml = change(&fs::read_to_string(&path).unwrap());
    settings_form(&xml, &path)
}

/// Every problem in a form, as `path: message`.
fn problems(form: &Form) -> Vec<String> {
    fn walk(node: &FormNode, out: &mut Vec<String>) {
        for note in &node.notes {
            let severity = if note.severity == Severity::Error { "error" } else { "warning" };
            out.push(format!("{}: {severity}: {}", node.path, note.message));
        }
        for attribute in &node.attributes {
            if let Some(error) = &attribute.field.error {
                out.push(format!("{}/@{}: error: {error}", node.path, attribute.name));
            }
            if let Some(warning) = &attribute.warning {
                out.push(format!("{}/@{}: warning: {warning}", node.path, attribute.name));
            }
        }
        match &node.kind {
            NodeKind::Value(field) => {
                if let Some(error) = &field.error {
                    out.push(format!("{}: error: {error}", node.path));
                }
            }
            NodeKind::Section { children } => children.iter().for_each(|c| walk(c, out)),
        }
    }
    let mut out = Vec::new();
    walk(&form.root, &mut out);
    out
}

#[test]
fn every_settings_file_is_valid() {
    let files = settings_files();
    assert!(files.len() > 20, "found the settings files");
    let mut found = Vec::new();
    for (name, path) in files {
        let xml = fs::read_to_string(&path).unwrap();
        found.extend(problems(&settings_form(&xml, &path)).into_iter().map(|p| format!("{name} {p}")));
    }
    assert!(found.is_empty(), "problems found:\n{}", found.join("\n"));
}

/// The types the schemas name, by name, from the fields of the schemas' own forms.
fn schema_types() -> HashMap<String, SimpleInfo> {
    fn walk(node: &FormNode, out: &mut HashMap<String, SimpleInfo>) {
        let fields = node.attributes.iter().map(|a| &a.field);
        let fields = fields.chain(match &node.kind {
            NodeKind::Value(field) => Some(field),
            NodeKind::Section { .. } => None,
        });
        for ty in fields.filter_map(|f| f.ty.as_ref()) {
            for ty in [Some(ty), ty.item.as_deref()].into_iter().flatten() {
                if let Some(name) = &ty.name {
                    out.entry(name.clone()).or_insert_with(|| ty.clone());
                }
            }
        }
        if let NodeKind::Section { children } = &node.kind {
            children.iter().for_each(|c| walk(c, out));
        }
    }
    fn schemas(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                schemas(&path, out);
            } else if path.extension().is_some_and(|e| e == "xsd") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    schemas(&example_data().join("settings_schema/xsd"), &mut paths);
    let mut types = HashMap::new();
    for path in paths {
        let form = build_schema_form(&fs::read_to_string(&path).unwrap()).unwrap();
        walk(&form.root, &mut types);
    }
    types
}

#[test]
fn the_schemas_types_accept_and_refuse_what_they_should() {
    let types = schema_types();
    // The type, values it accepts, and values it refuses
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "FiniteDouble",
            &["0", "1", "-1.5", "+2.0", "1e3", "1.0E-6", ".5", "5.", " 1.0 "],
            &["INF", "-INF", "NaN", "abc", "1.0.0", "", "1e", "0x10", "1,5"],
        ),
        ("FiniteNonNegativeDouble", &["0", "0.0", "3.2", "1e-300"], &["-0.1", "-1", "INF", "NaN"]),
        ("FinitePositiveDouble", &["1e-9", "2", "0.5"], &["0", "0.0", "-1", "INF", "NaN", ""]),
        ("TrueOrFalse", &["true", "false"], &["1", "0", "True", "yes", ""]),
        ("Name", &["P101", "B_BPPROBE_101", "a.b-c"], &["", "P 101", " "]),
        ("XmlPath", &["wall/wall.xml", "../default/wall/wall.xml"], &["wall.txt", "a b.xml", "", ".xml"]),
        ("NumberOfGridPoints", &["2", "65", "513"], &["1", "0", "-3", "2.5", "x"]),
        ("ZeroOrOne", &["0", "1"], &["2", "-1", "0.5", "true"]),
        ("ZeroOrOneList", &["0 1 1 0", "1"], &["0 2", "a", "0,1"]),
        ("FiniteDoubleList", &["0.1 0.2 0.3", "1", "-1 2e3"], &["0.1 INF", "a", "1 NaN"]),
        ("FiniteNonNegativeDoubleList", &["0 0.5 2"], &["0 -0.5", "INF"]),
        ("NameList", &["V_FLOOP_007 V_FLOOP_009", "P101"], &[]),
        (
            "TreeName",
            &["EFIT01", "A", "GSFIT_RUN_01"],
            &["efit01", "1EFIT", "ABCDEFGHIJKLM", "", "EFIT-01"],
        ),
        ("DatabaseReaderMethod", &["st40_mdsplus"], &["other", ""]),
        ("DatabaseWriterMethod", &["tokamak_energy_mdsplus"], &["other", ""]),
    ];
    let mut wrong = Vec::new();
    for (name, good, bad) in cases {
        let ty = types.get(*name).unwrap_or_else(|| panic!("the schemas declare {name}"));
        for value in *good {
            if let Err(error) = ty.validate(value) {
                wrong.push(format!("{name} refuses {value:?}: {error}"));
            }
        }
        for value in *bad {
            if ty.validate(value).is_ok() {
                wrong.push(format!("{name} accepts {value:?}"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{}",
        wrong.join(
            "
"
        )
    );
}

fn errors_at(form: &Form, path: &str) -> Vec<String> {
    problems(form)
        .into_iter()
        .filter_map(|p| p.strip_prefix(&format!("{path}: error: ")).map(str::to_string))
        .collect()
}

#[test]
fn a_probe_name_used_twice_is_marked_on_both() {
    let form = changed_form("equilibrium/constraints_b_field_pol_probe.xml", |xml| {
        xml.replacen("<name>P102</name>", "<name>P101</name>", 1)
    });
    let probes = "/constraints__b_field_pol_probe/b_field_pol_probe";
    assert_eq!(
        errors_at(&form, &format!("{probes}/name")),
        ["Must be unique, but b_field_pol_probe [1] has the same name too"]
    );
    assert_eq!(
        errors_at(&form, &format!("{probes}[2]/name")),
        ["Must be unique, but b_field_pol_probe [0] has the same name too"]
    );
    assert_eq!(problems(&form).len(), 2, "{:?}", problems(&form));
}

#[test]
fn names_used_three_times_list_the_others() {
    let form = changed_form("equilibrium/constraints_flux_loop.xml", |xml| {
        let first = xml.split("<name>").nth(1).unwrap().split("</name>").next().unwrap().to_string();
        let mut names = xml.match_indices("<name>").map(|(i, _)| i).skip(1).take(2).collect::<Vec<_>>();
        let mut xml = xml.to_string();
        // From the end, so that the earlier positions hold
        names.reverse();
        for start in names {
            let end = start + xml[start..].find("</name>").unwrap();
            xml.replace_range(start..end, &format!("<name>{first}"));
        }
        xml
    });
    let loops = "/constraints__flux_loop/flux_loop";
    assert_eq!(
        errors_at(&form, &format!("{loops}/name")),
        ["Must be unique, but flux_loop [1] and flux_loop [2] have the same name too"]
    );
}

#[test]
fn a_key_needs_its_field() {
    // The schema requires the name too, which says it's missing, so the key doesn't again
    let form = changed_form("workflow.xml", |xml| {
        let start = xml.find("<tree_name>").unwrap();
        let end = xml[start..].find("</tree_name>").unwrap() + start + "</tree_name>".len();
        format!("{}{}", &xml[..start], &xml[end..])
    });
    let missing: Vec<String> = problems(&form).into_iter().filter(|p| p.contains("tree_name")).collect();
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(missing[0].contains("Missing <tree_name>, which the schema requires"), "{missing:?}");
}

#[test]
fn a_method_written_twice_breaks_the_unique_constraint() {
    let form = changed_form("database_writer.xml", |xml| {
        let start = xml.find("<method>").unwrap();
        let end = xml[start..].find("</method>").unwrap() + start + "</method>".len();
        format!("{}{}{}", &xml[..end], &xml[start..end], &xml[end..])
    });
    let found: Vec<String> = problems(&form).into_iter().filter(|p| p.contains("unique")).collect();
    assert_eq!(
        found,
        [
            "/database_writer/method: error: Must be unique, but method [1] has the same value too",
            "/database_writer/method[2]: error: Must be unique, but method [0] has the same value too",
        ]
    );
}
