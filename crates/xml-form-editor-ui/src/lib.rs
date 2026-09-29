//! The form editor's webview. It shows the form for the document the extension sends, with the
//! files that document links to in place, and sends back the edits the form makes;
//! `src/extension.ts` is the other side.

mod bridge;
mod fields;
mod matrix;
mod rows;

use std::collections::{HashMap, VecDeque};
use std::ops::Deref;
use std::sync::Arc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use xml_form_editor_core::{
    EditOp, Form, FormNode, Linked, NodeKind, Note, Problems, Schema, XmlError, apply_edit,
    build_form, schema_location, values,
};

use crate::bridge::{In, Out};
use crate::rows::{Folding, Row, Values, Versions};

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// A value compared by identity, so that recomputing it counts as a change without comparing what
/// it holds.
pub(crate) struct Shared<T>(pub(crate) Arc<T>);

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(Arc::clone(&self.0))
    }
}

impl<T> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// A document, as the extension last sent it.
#[derive(Clone)]
struct XmlText {
    text: Arc<str>,
    version: f64,
}

#[derive(Clone)]
enum SchemaState {
    /// Not known yet: there's no document, or it has never been well-formed
    Unknown,
    /// The document names no schema
    Unnamed,
    Loading {
        location: String,
    },
    Loaded {
        location: String,
        path: String,
        schema: Arc<Schema>,
    },
    Failed {
        location: String,
        message: String,
    },
}

impl SchemaState {
    fn location(&self) -> Option<&str> {
        match self {
            SchemaState::Unknown | SchemaState::Unnamed => None,
            SchemaState::Loading { location }
            | SchemaState::Loaded { location, .. }
            | SchemaState::Failed { location, .. } => Some(location),
        }
    }
}

/// A document's form, or why it can't be built.
type Built = Shared<Result<Form, XmlError>>;

fn form_of(built: &Built) -> Option<&Form> {
    built.0.as_ref().as_ref().ok()
}

fn version_values(text: &Option<Arc<str>>) -> Option<Shared<Values>> {
    let values = values(text.as_deref()?).ok()?;
    Some(Shared(Arc::new(values)))
}

/// A file the form shows: the one it was opened on, or one it links to.
#[derive(Clone, Copy)]
struct Doc {
    xml: RwSignal<Option<XmlText>>,
    dirty: RwSignal<bool>,
    // The file as last saved, and as it was when the form opened, to mark the values that changed
    saved: RwSignal<Option<Arc<str>>>,
    original: RwSignal<Option<Arc<str>>>,
    schema: RwSignal<SchemaState>,
    /// `None` until there's a document, and while its schema loads
    built: Memo<Option<Built>>,
    saved_values: Memo<Option<Shared<Values>>>,
    original_values: Memo<Option<Shared<Values>>>,
}

impl Doc {
    /// The signals of the document the extension calls `uri`. It asks for the document's schema
    /// whenever the document names a different one.
    fn new(uri: Arc<str>) -> Doc {
        let xml = RwSignal::new(None::<XmlText>);
        let schema = RwSignal::new(SchemaState::Unknown);
        let saved = RwSignal::new(None::<Arc<str>>);
        let original = RwSignal::new(None::<Arc<str>>);

        // The schema the document names; `None` while there's no document, or it isn't well-formed
        let named_schema = Memo::new(move |_| {
            xml.with(|x| x.as_ref().and_then(|x| schema_location(&x.text).ok()))
        });

        Effect::new(move |_| {
            let Some(named) = named_schema.get() else {
                return;
            };
            let known = schema.with_untracked(|s| {
                !matches!(s, SchemaState::Unknown) && s.location() == named.as_deref()
            });
            if known {
                return;
            }
            match named {
                Some(location) => {
                    schema.set(SchemaState::Loading {
                        location: location.clone(),
                    });
                    bridge::post(&Out::RequestSchema {
                        document: uri.to_string(),
                        location,
                    });
                }
                None => schema.set(SchemaState::Unnamed),
            }
        });

        let built = Memo::new(move |_| {
            let xml = xml.get()?;
            let state = schema.get();
            let named = named_schema.get().flatten();
            if matches!(state, SchemaState::Loading { .. }) || state.location() != named.as_deref()
            {
                return None;
            }
            let schema = match state {
                SchemaState::Loaded { schema, .. } => Some(schema),
                _ => None,
            };
            Some(Shared(Arc::new(build_form(&xml.text, schema.as_deref()))))
        });

        Doc {
            xml,
            dirty: RwSignal::new(false),
            saved,
            original,
            schema,
            built,
            saved_values: Memo::new(move |_| saved.with(version_values)),
            original_values: Memo::new(move |_| original.with(version_values)),
        }
    }

    /// Takes a version of the document from the extension; whether its text changed.
    fn receive(&self, text: String, version: f64, dirty: bool) -> bool {
        // Only enables Save: the saved text comes in `saved`, as this can lag behind edits
        if self.dirty.get_untracked() != dirty {
            self.dirty.set(dirty);
        }
        let unchanged = self.xml.with_untracked(|x| {
            x.as_ref()
                .is_some_and(|x| x.version == version && *x.text == *text)
        });
        if !unchanged {
            self.xml.set(Some(XmlText {
                text: text.into(),
                version,
            }));
        }
        !unchanged
    }

    fn mark_saved(&self, text: Arc<str>) {
        if self.saved.with_untracked(|s| s.as_deref() != Some(&*text)) {
            self.saved.set(Some(text.clone()));
        }
        if self.original.with_untracked(Option::is_none) {
            self.original.set(Some(text));
        }
    }

    fn receive_schema(
        &self,
        location: String,
        path: Option<String>,
        text: Option<String>,
        error: Option<String>,
    ) {
        // A reply to an earlier request
        if self
            .schema
            .with_untracked(|s| s.location() != Some(location.as_str()))
        {
            return;
        }
        let path = path.unwrap_or_else(|| location.clone());
        self.schema.set(match (text, error) {
            (Some(text), _) => match Schema::parse(&text) {
                Ok(parsed) => SchemaState::Loaded {
                    location,
                    path,
                    schema: Arc::new(parsed),
                },
                Err(error) => SchemaState::Failed {
                    location,
                    message: format!("{path}: {error}"),
                },
            },
            (None, error) => SchemaState::Failed {
                location,
                message: error.unwrap_or_else(|| format!("Could not read {path}")),
            },
        });
    }

    fn versions(&self) -> Versions {
        Versions {
            saved: self.saved_values.get(),
            original: self.original_values.get(),
        }
    }
}

type Docs = RwSignal<HashMap<Arc<str>, Doc>>;

/// The document the extension calls `uri`, made when first heard of.
fn doc(docs: Docs, owner: &Owner, uri: &str) -> Doc {
    if let Some(doc) = docs.with_untracked(|docs| docs.get(uri).copied()) {
        return doc;
    }
    let uri: Arc<str> = uri.into();
    let doc = owner.with(|| Doc::new(uri.clone()));
    docs.update(|docs| {
        docs.insert(uri, doc);
    });
    doc
}

/// What became of a file a document links to.
#[derive(Clone)]
enum LinkState {
    Opening,
    /// Open, with the extension's name for it
    Open(Arc<str>),
    Failed(String),
}

/// The files the documents link to, by the document and the `href` it gives.
type Links = RwSignal<HashMap<(Arc<str>, String), LinkState>>;

/// Puts the contents of the files that `node` and the elements in it link to in place (see
/// [`FormNode::linked`]), or notes why it can't. `node` is in `document`, the last of `chain`,
/// which holds the documents above it, so that a link back to one of them stops there.
fn attach_links(
    node: &mut FormNode,
    document: &Arc<str>,
    chain: &mut Vec<Arc<str>>,
    docs: Docs,
    links: Links,
) {
    let NodeKind::Section { children } = &mut node.kind else {
        return;
    };
    for child in children {
        attach_links(child, document, chain, docs, links);
    }
    let Some(href) = node.link.clone() else {
        return;
    };
    let state = links.with(|links| links.get(&(document.clone(), href.clone())).cloned());
    let target = match state {
        None | Some(LinkState::Opening) => return,
        Some(LinkState::Failed(message)) => {
            node.notes.push(Note::error(message));
            return;
        }
        Some(LinkState::Open(target)) => target,
    };
    if chain.contains(&target) {
        node.notes.push(Note::warning(format!(
            "{href} is shown above already, as this setting is in it, so it isn't shown again"
        )));
        return;
    }
    let Some(doc) = docs.with(|docs| docs.get(&target).copied()) else {
        return;
    };
    // `None` while the file or its schema loads
    let Some(built) = doc.built.get() else {
        return;
    };
    match built.0.as_ref() {
        Err(error) => node.notes.push(Note::error(format!(
            "{href} is not well-formed, so its settings can't be shown: {} (line {})",
            error.message, error.line
        ))),
        Ok(form) => {
            let mut root = form.root.clone();
            match doc.schema.get() {
                SchemaState::Failed { message, .. } => root.notes.push(Note::warning(format!(
                    "{message} The values of {href} are edited as text, without checking."
                ))),
                SchemaState::Loaded { schema, .. } => root
                    .notes
                    .extend(schema.warnings().iter().map(|w| Note::warning(w.clone()))),
                _ => {}
            }
            chain.push(target.clone());
            attach_links(&mut root, &target, chain, docs, links);
            chain.pop();
            node.linked = Some(Box::new(Linked {
                document: target.to_string(),
                root,
            }));
        }
    }
}

/// How often an edit is tried before giving up, when the document keeps changing under it.
const MAX_ATTEMPTS: u32 = 3;

/// Edits wait here while another is applied, so that each is made against the text it changes:
/// one made against text that has since changed would be refused.
#[derive(Default)]
struct EditQueue {
    waiting: VecDeque<Pending>,
    in_flight: Option<InFlight>,
    next_id: u32,
}

struct Pending {
    document: Arc<str>,
    op: EditOp,
    /// How many times it's been tried
    attempts: u32,
}

struct InFlight {
    id: u32,
    edit: Pending,
}

/// What the rows need to make changes.
#[derive(Clone, Copy)]
pub(crate) struct Ctx {
    docs: Docs,
    pub(crate) expanded: RwSignal<HashMap<String, bool>>,
    failure: RwSignal<Option<String>>,
    queue: StoredValue<EditQueue>,
    /// The rows the page shows, which a matrix's grid follows (see [`Row::key`])
    pub(crate) rows: Memo<Vec<Row>>,
}

impl Ctx {
    pub(crate) fn edit(&self, document: Arc<str>, op: EditOp) {
        self.queue.update_value(|queue| {
            queue.waiting.push_back(Pending {
                document,
                op,
                attempts: 0,
            })
        });
        self.send_next();
    }

    /// Sends the next waiting edit, unless one is being applied.
    fn send_next(&self) {
        loop {
            let next = self
                .queue
                .try_update_value(|queue| {
                    if queue.in_flight.is_some() {
                        None
                    } else {
                        queue.waiting.pop_front()
                    }
                })
                .flatten();
            let Some(pending) = next else { return };
            let xml = self.docs.with_untracked(|docs| {
                docs.get(&pending.document)
                    .and_then(|doc| doc.xml.get_untracked())
            });
            let Some(xml) = xml else {
                self.failure.set(Some(format!(
                    "Could not change {}: {} is not open",
                    pending.op.path(),
                    file_name(&pending.document)
                )));
                continue;
            };
            match apply_edit(&xml.text, &pending.op) {
                // Already that way
                Ok(edits) if edits.is_empty() => {}
                Ok(edits) => {
                    let document = pending.document.to_string();
                    let id = self.queue.try_update_value(|queue| {
                        queue.next_id += 1;
                        queue.in_flight = Some(InFlight {
                            id: queue.next_id,
                            edit: Pending {
                                attempts: pending.attempts + 1,
                                ..pending
                            },
                        });
                        queue.next_id
                    });
                    let edits = edits.into_iter().map(Into::into).collect();
                    bridge::post(&Out::Edit {
                        document,
                        id: id.unwrap_or_default(),
                        version: xml.version,
                        edits,
                    });
                    return;
                }
                Err(error) => self.failure.set(Some(error.to_string())),
            }
        }
    }

    /// The extension has applied, or refused, the edit in flight. Without an `error`, it was
    /// refused because the document changed first.
    fn edit_done(&self, id: u32, applied: bool, error: Option<String>) {
        let gave_up = self
            .queue
            .try_update_value(|queue| {
                let done = queue.in_flight.take_if(|in_flight| in_flight.id == id)?;
                if applied {
                    return None;
                }
                if error.is_none() && done.edit.attempts < MAX_ATTEMPTS {
                    // Try again, against the new text
                    queue.waiting.push_front(done.edit);
                    return None;
                }
                Some(done.edit)
            })
            .flatten();
        if let Some(edit) = gave_up {
            let reason = error.unwrap_or_else(|| "the document kept changing".to_string());
            self.failure.set(Some(format!(
                "Could not change {} in {}: {reason}",
                edit.op.path(),
                file_name(&edit.document)
            )));
        }
        self.send_next();
    }

    pub(crate) fn reveal(&self, document: &str, offset: u32) {
        bridge::post(&Out::Reveal {
            document: document.to_string(),
            offset,
        });
    }

    pub(crate) fn copy(&self, text: String) {
        bridge::post(&Out::Copy { text });
    }
}

/// The last part of a document's URI.
fn file_name(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

#[component]
fn App() -> impl IntoView {
    let owner = Owner::current().expect("a component has an owner");
    let docs: Docs = RwSignal::new(HashMap::new());
    // The document the form was opened on
    let main = RwSignal::new(None::<Arc<str>>);
    let links: Links = RwSignal::new(HashMap::new());
    let filter = RwSignal::new(String::new());
    let expanded = RwSignal::new(HashMap::<String, bool>::new());
    let hide_unused = RwSignal::new(false);
    let failure = RwSignal::new(None::<String>);
    let queue = StoredValue::new(EditQueue::default());
    let main_doc = move || {
        let uri = main.get()?;
        docs.with(|docs| docs.get(&uri).copied())
    };

    // Open the files the documents link to
    Effect::new(move |_| {
        let wanted: Vec<(Arc<str>, String)> = docs.with(|docs| {
            docs.iter()
                .flat_map(|(uri, doc)| {
                    let hrefs = doc.built.with(|built| {
                        built
                            .as_ref()
                            .and_then(form_of)
                            .map(Form::links)
                            .unwrap_or_default()
                    });
                    hrefs.into_iter().map(|href| (uri.clone(), href))
                })
                .collect()
        });
        let new: Vec<_> = links.with_untracked(|links| {
            wanted
                .into_iter()
                .filter(|key| !links.contains_key(key))
                .collect()
        });
        if new.is_empty() {
            return;
        }
        links.update(|links| {
            links.extend(new.iter().map(|key| (key.clone(), LinkState::Opening)));
        });
        for (document, href) in new {
            bridge::post(&Out::OpenLink {
                document: document.to_string(),
                href,
            });
        }
    });

    // The main document's form, with the linked files' in place
    let composed = Memo::new(move |_| {
        let uri = main.get()?;
        let built = main_doc()?.built.get()?;
        let mut form = form_of(&built)?.clone();
        attach_links(&mut form.root, &uri, &mut vec![uri.clone()], docs, links);
        Some(Shared(Arc::new(form)))
    });

    let rows = Memo::new(move |_| {
        let (Some(form), Some(uri)) = (composed.get(), main.get()) else {
            return Vec::new();
        };
        let versions: HashMap<Arc<str>, Versions> = docs.with(|docs| {
            docs.iter()
                .map(|(uri, doc)| (uri.clone(), doc.versions()))
                .collect()
        });
        expanded.with(|expanded| {
            filter.with(|filter| {
                rows::rows(
                    &form,
                    &uri,
                    &Folding {
                        filter,
                        expanded,
                        hide_unused: hide_unused.get(),
                        versions: &versions,
                    },
                )
            })
        })
    });

    let ctx = Ctx {
        docs,
        expanded,
        failure,
        queue,
        rows,
    };

    bridge::on_message(move |message| {
        let In {
            kind,
            uri,
            main: is_main,
            document,
            text,
            version,
            dirty,
            id,
            applied,
            location,
            href,
            path,
            error,
        } = message;
        match kind.as_str() {
            "document" => {
                let (Some(uri), Some(text), Some(version)) = (uri, text, version) else {
                    return;
                };
                let doc = doc(docs, &owner, &uri);
                if is_main == Some(true) && main.with_untracked(|m| m.as_deref() != Some(&*uri)) {
                    main.set(Some(uri.into()));
                }
                if doc.receive(text, version, dirty.unwrap_or(false)) {
                    failure.set(None);
                }
            }
            "saved" => {
                if let (Some(uri), Some(text)) = (uri, text) {
                    doc(docs, &owner, &uri).mark_saved(text.into());
                }
            }
            "editDone" => {
                if let Some(id) = id {
                    ctx.edit_done(id as u32, applied.unwrap_or(false), error);
                }
            }
            "schema" => {
                if let (Some(document), Some(location)) = (document, location) {
                    doc(docs, &owner, &document).receive_schema(location, path, text, error);
                }
            }
            "link" => {
                let (Some(document), Some(href)) = (document, href) else {
                    return;
                };
                let state = match (uri, error) {
                    (Some(uri), _) => LinkState::Open(uri.into()),
                    (None, error) => {
                        LinkState::Failed(error.unwrap_or_else(|| format!("Could not open {href}")))
                    }
                };
                links.update(|links| {
                    links.insert((document.into(), href), state);
                });
            }
            _ => {}
        }
    });

    let problems = Memo::new(move |_| {
        composed.with(|form| form.as_ref().map(|f| f.problems()).unwrap_or_default())
    });

    // Whether any file the form shows has unsaved changes
    let dirty = Memo::new(move |_| docs.with(|docs| docs.values().any(|doc| doc.dirty.get())));

    let set_all = move |open: bool| {
        let keys = composed.with_untracked(|form| {
            form.as_ref()
                .map(|f| rows::section_keys(f))
                .unwrap_or_default()
        });
        expanded.update(|sections| sections.extend(keys.into_iter().map(|key| (key, open))));
    };

    // Opens the sections that hold problems and scrolls to the first one
    let show_problems = move |_| {
        let keys = composed.with_untracked(|form| {
            form.as_ref()
                .map(|f| rows::sections_with_problems(f))
                .unwrap_or_default()
        });
        filter.set(String::new());
        hide_unused.set(false);
        expanded.update(|sections| sections.extend(keys.into_iter().map(|key| (key, true))));
        request_animation_frame(|| {
            if let Ok(Some(row)) = document().query_selector(".row.problem") {
                row.scroll_into_view();
            }
        });
    };

    bridge::post(&Out::Ready);

    view! {
        <header class="toolbar">
            <input
                type="search"
                class="filter"
                placeholder="Filter by name, description or value"
                prop:value=move || filter.get()
                on:input=move |ev| filter.set(event_target_value(&ev))
            />
            <button on:click=move |_| set_all(true)>"Expand all"</button>
            <button on:click=move |_| set_all(false)>"Collapse all"</button>
            <label class="toggle">
                <input
                    type="checkbox"
                    prop:checked=move || hide_unused.get()
                    on:change=move |ev| hide_unused.set(event_target_checked(&ev))
                />
                "Hide unused sections"
            </label>
            <span class="spacer"></span>
            {move || {
                let found = problems.get();
                (found.errors + found.warnings > 0)
                    .then(|| view! { <button class="problems" on:click=show_problems>{problem_label(found)}</button> })
            }}
            <button
                class="save"
                title="Save the file, and the files it links to (Ctrl+S)"
                prop:disabled=move || !dirty.get()
                on:click=move |_| bridge::post(&Out::Save)
            >
                "Save"
            </button>
            <button on:click=move |_| bridge::post(&Out::OpenSource)>"Open XML"</button>
        </header>
        {move || main.get().zip(main_doc()).map(|(uri, doc)| schema_status(doc.schema.get(), uri))}
        {move || failure.get().map(|message| view! { <div class="banner error">{message}</div> })}
        {move || match main_doc().and_then(|doc| doc.built.get()) {
            None => Some(view! { <div class="status">"Loading…"</div> }.into_any()),
            Some(built) => built.0.as_ref().as_ref().err().map(|error| {
                view! {
                    <div class="banner error">
                        <strong>"The XML is not well-formed, so the form can't be shown. "</strong>
                        {error.message.clone()}
                        " "
                        <button on:click=move |_| bridge::post(&Out::OpenSource)>"Open XML"</button>
                    </div>
                }
                .into_any()
            }),
        }}
        <main class="rows">
            <For each=move || rows.get() key=Row::key children=move |row| fields::row_view(row, ctx) />
        </main>
    }
}

fn schema_status(state: SchemaState, document: Arc<str>) -> AnyView {
    match state {
        SchemaState::Unknown => ().into_any(),
        SchemaState::Unnamed => view! {
            <div class="banner info">
                "This file names no schema, so every value is edited as text, without checking. To get "
                "dropdowns and checking, name its XSD on the root element, with "
                <code>"xsi:noNamespaceSchemaLocation=\"settings.xsd\""</code>
                "."
            </div>
        }
        .into_any(),
        SchemaState::Loading { location } => view! { <div class="status">"Loading the schema " {location} "…"</div> }.into_any(),
        SchemaState::Loaded { location, path, schema } => {
            let warnings = schema.warnings().to_vec();
            view! {
                <div class="status">
                    "Schema: "
                    <a
                        href="#"
                        title=path
                        on:click=move |ev| {
                            ev.prevent_default();
                            bridge::post(&Out::OpenSchema { document: document.to_string() });
                        }
                    >
                        {location}
                    </a>
                </div>
                {warnings.into_iter().map(|warning| view! { <div class="banner warning">{warning}</div> }).collect_view()}
            }
            .into_any()
        }
        SchemaState::Failed { message, .. } => {
            view! { <div class="banner error">{message} " Values are edited as text, without checking."</div> }.into_any()
        }
    }
}

fn problem_label(problems: Problems) -> String {
    let count = |n: usize, what: &str| {
        if n == 1 {
            format!("1 {what}")
        } else {
            format!("{n} {what}s")
        }
    };
    match (problems.errors, problems.warnings) {
        (errors, 0) => count(errors, "error"),
        (0, warnings) => count(warnings, "warning"),
        (errors, warnings) => format!("{}, {}", count(errors, "error"), count(warnings, "warning")),
    }
}
