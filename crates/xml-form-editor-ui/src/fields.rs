//! The rows' views: section headings, and the controls that edit values.

use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use xml_form_editor_core::{AttributeField, EditOp, EnumValue, Field, Note, Severity, Variant};

use crate::Ctx;
use crate::rows::{AttributeRow, Change, LinkedFile, Place, Row, SectionRow, ValueRow};

/// Rows deeper than this are indented no further.
const MAX_INDENT: usize = 12;

/// How long a copy button shows its tick.
const COPIED_TICK: Duration = Duration::from_millis(1500);

pub(crate) fn row_view(row: Row, ctx: Ctx) -> AnyView {
    match row {
        Row::Section(row) => section_view(row, ctx).into_any(),
        Row::Value(row) => value_view(row, ctx).into_any(),
        Row::Attribute(row) => attribute_view(row, ctx).into_any(),
        Row::Matrix(row) => crate::matrix::matrix_view(row, ctx).into_any(),
    }
}

pub(crate) fn row_class(kind: &str, depth: usize, unused: bool, problem: bool) -> String {
    let mut class = format!("row {kind} depth-{}", depth.min(MAX_INDENT));
    if unused {
        class.push_str(" unused");
    }
    if problem {
        class.push_str(" problem");
    }
    class
}

fn section_view(row: SectionRow, ctx: Ctx) -> impl IntoView {
    let SectionRow {
        place,
        depth,
        name,
        index,
        copy_path,
        doc,
        notes,
        variant,
        unused,
        expanded,
        empty,
        offset,
        linked_file,
    } = row;
    let problem = notes.iter().any(|n| n.severity == Severity::Error);
    let Place { document, key, .. } = place;
    let toggle = {
        let key = key.clone();
        move |_| {
            ctx.expanded.update(|sections| {
                sections.insert(key.clone(), !expanded);
            })
        }
    };
    view! {
        <div class=row_class("section", depth, unused, problem) data-path=key>
            <div class="title">
                {(!empty).then(|| view! {
                    <button
                        class=if expanded { "twisty open" } else { "twisty" }
                        title=if expanded { "Collapse" } else { "Expand" }
                        on:click=toggle
                    >
                        <svg class="chevron" viewBox="0 0 16 16" aria-hidden="true">
                            <path d="M6 3.5 10.5 8 6 12.5" />
                        </svg>
                    </button>
                })}
                <span class="name" title="Show in the XML" on:click=move |_| ctx.reveal(&document, offset)>{label(name, index)}</span>
                {linked_file.map(|file| file_badge(ctx, file))}
                {variant_badge(&variant)}
                {copy_path.map(|path| copy_button(ctx, path))}
            </div>
            {doc.map(|doc| view! { <div class="doc">{doc}</div> })}
            {empty.then(|| view! { <div class="doc">"(no settings)"</div> })}
            {notes_view(notes)}
        </div>
    }
}

/// The name of the file a section shows, which opens it.
fn file_badge(ctx: Ctx, file: LinkedFile) -> impl IntoView {
    let LinkedFile {
        href,
        document,
        offset,
    } = file;
    view! {
        <span class="file" title="The file these settings are in. Click to show it." on:click=move |_| ctx.reveal(&document, offset)>
            {href}
        </span>
    }
}

fn value_view(row: ValueRow, ctx: Ctx) -> impl IntoView {
    let ValueRow {
        place,
        depth,
        name,
        index,
        copy_path,
        doc,
        field,
        notes,
        variant,
        unused,
        selector,
        offset,
        units,
        change,
    } = row;
    let problem = field.error.is_some() || notes.iter().any(|n| n.severity == Severity::Error);
    let summary = field.ty.as_ref().map(|ty| ty.summary());
    let type_doc = field.ty.as_ref().and_then(|ty| ty.doc.clone());
    let (key, document) = (place.key.clone(), place.document.clone());
    view! {
        <div class=row_class("value", depth, unused, problem) data-path=key>
            <div class="title">
                <span class="name" title="Show in the XML" on:click=move |_| ctx.reveal(&document, offset)>{label(name, index)}</span>
                {summary.map(|summary| view! { <span class="type" title=type_doc>{summary}</span> })}
                {variant_badge(&variant)}
                {copy_path.map(|path| copy_button(ctx, path))}
            </div>
            {doc.map(|doc| view! { <div class="doc">{doc}</div> })}
            {selector.then(|| view! { <div class="doc hint">"Chooses which of the sections below is used."</div> })}
            {editor(ctx, Target::Element(place), field, units, change)}
            {notes_view(notes)}
        </div>
    }
}

fn attribute_view(row: AttributeRow, ctx: Ctx) -> impl IntoView {
    let AttributeRow {
        place,
        depth,
        copy_path,
        attribute,
        unused,
        offset,
        change,
    } = row;
    let AttributeField {
        name,
        doc,
        field,
        units,
        warning,
    } = attribute;
    let problem = field.error.is_some();
    let summary = field.ty.as_ref().map(|ty| ty.summary());
    let key = format!("{}/@{name}", place.key);
    let document = place.document.clone();
    let label = format!("@{name}");
    view! {
        <div class=row_class("value attribute", depth, unused, problem) data-path=key>
            <div class="title">
                <span class="name" title="Show in the XML" on:click=move |_| ctx.reveal(&document, offset)>{label}</span>
                {summary.map(|summary| view! { <span class="type">{summary}</span> })}
                {copy_button(ctx, copy_path)}
            </div>
            {doc.map(|doc| view! { <div class="doc">{doc}</div> })}
            {editor(ctx, Target::Attribute(place, name), field, units, change)}
            {warning.map(|warning| view! { <div class="note warning">{warning}</div> })}
        </div>
    }
}

/// Where an edited value goes.
#[derive(Clone)]
enum Target {
    Element(Place),
    /// An attribute of the element, named as written
    Attribute(Place, String),
}

impl Target {
    fn place(&self) -> &Place {
        match self {
            Target::Element(place) | Target::Attribute(place, _) => place,
        }
    }

    fn set(&self, value: String) -> EditOp {
        match self {
            Target::Element(place) => EditOp::SetValue {
                path: place.path.clone(),
                value,
            },
            Target::Attribute(place, name) => EditOp::SetAttribute {
                path: place.path.clone(),
                name: name.clone(),
                value,
            },
        }
    }
}

enum Widget {
    Choice(Vec<EnumValue>),
    Checkbox,
    Text,
}

fn widget(field: &Field) -> Widget {
    match &field.ty {
        Some(ty) if !ty.enumeration.is_empty() => Widget::Choice(ty.enumeration.clone()),
        // A checkbox can't show nil
        Some(ty) if ty.is_boolean() && field.nillable => Widget::Choice(
            ["true", "false"]
                .map(|value| EnumValue {
                    value: value.to_string(),
                    doc: None,
                })
                .to_vec(),
        ),
        // Text shows a value that isn't a boolean, so it can be seen and fixed
        Some(ty) if ty.is_boolean() && field.error.is_none() => Widget::Checkbox,
        _ => Widget::Text,
    }
}

/// The control that edits a value, with its nil checkbox and any error.
///
/// Only valid values are written. An invalid one stays in the control with the reason it wasn't
/// written, until it's fixed or Escape restores the value in the file.
fn editor(
    ctx: Ctx,
    target: Target,
    field: Field,
    units: Option<String>,
    change: Change,
) -> impl IntoView {
    // Set once the user types or picks a value, which then decides what error shows
    let touched = RwSignal::new(false);
    let rejected = RwSignal::new(None::<String>);
    // Unchecking nil enables the control; the file stays nil until a value is written
    let nil = RwSignal::new(field.nil);
    let input = NodeRef::<html::Input>::new();

    let widget = widget(&field);
    let current = if field.nil {
        String::new()
    } else {
        field.display_value()
    };
    let (file_nil, nillable, file_error) = (field.nil, field.nillable, field.error.clone());
    let chosen_doc = match &widget {
        Widget::Choice(options) if !file_nil => options
            .iter()
            .find(|o| o.value == current)
            .and_then(|o| o.doc.clone()),
        _ => None,
    };
    let field = StoredValue::new(field);
    let target = StoredValue::new(target);

    let commit = move |raw: String| {
        let (value, error, unchanged) = field.with_value(|f| {
            let value = f.normalize(&raw);
            let error = f.check(&value);
            let unchanged = !f.nil && value == f.display_value();
            (value, error, unchanged)
        });
        touched.set(true);
        rejected.set(error.clone());
        if error.is_none() && !unchanged {
            let (document, op) = target.with_value(|t| (t.place().document.clone(), t.set(value)));
            ctx.edit(document, op);
        }
    };

    let mark = Mark::from(change);
    let control = match widget {
        Widget::Choice(options) => choice(options, current, file_nil, nil, mark, commit).into_any(),
        Widget::Checkbox => {
            checkbox(matches!(current.as_str(), "true" | "1"), mark, commit).into_any()
        }
        Widget::Text => text(
            current,
            field,
            nil,
            Entry { touched, rejected },
            input,
            mark,
            commit,
        )
        .into_any(),
    };
    let units =
        units.map(|units| view! { <span class="units" title="Units">{units_label(units)}</span> });

    let nil_toggle = nillable.then(|| {
        let on_change = move |ev: web_sys::Event| {
            if event_target_checked(&ev) {
                nil.set(true);
                touched.set(false);
                rejected.set(None);
                if !file_nil && let Target::Element(place) = target.get_value() {
                    ctx.edit(place.document, EditOp::SetNil { path: place.path });
                }
            } else {
                nil.set(false);
                request_animation_frame(move || {
                    if let Some(element) = input.get() {
                        let _ = element.focus();
                    }
                });
            }
        };
        view! {
            <label class="nil" title="xsi:nil: the element has no value">
                <input type="checkbox" prop:checked=move || nil.get() on:change=on_change />
                "nil"
            </label>
        }
    });

    view! {
        <div class="control-line">{control}{units}{nil_toggle}</div>
        {chosen_doc.map(|doc| view! { <div class="option-doc">{doc}</div> })}
        {move || (file_nil && !nil.get()).then(|| view! { <div class="hint">"The file keeps nil until you enter a value."</div> })}
        {move || {
            if touched.get() {
                rejected.get().map(|error| view! { <div class="error">"Not saved: " {error}</div> }.into_any())
            } else {
                file_error.clone().map(|error| view! { <div class="error">{error}</div> }.into_any())
            }
        }}
    }
}

fn choice(
    options: Vec<EnumValue>,
    current: String,
    file_nil: bool,
    nil: RwSignal<bool>,
    mark: Mark,
    commit: impl Fn(String) + Copy + 'static,
) -> impl IntoView {
    let known = options.iter().any(|o| o.value == current);
    let placeholder = if file_nil {
        Some("nil".to_string())
    } else if known {
        None
    } else if current.is_empty() {
        Some("(empty)".to_string())
    } else {
        Some(format!("{current} (not allowed)"))
    };
    let options = options
        .into_iter()
        .map(|EnumValue { value, doc }| {
            let selected = !file_nil && value == current;
            let label = value.clone();
            view! { <option value=value selected=selected title=doc>{label}</option> }
        })
        .collect_view();
    view! {
        <select class=mark.class("control") title=mark.title prop:disabled=move || nil.get() on:change=move |ev| commit(event_target_value(&ev))>
            {placeholder.map(|text| view! { <option value="" disabled=true selected=true>{text}</option> })}
            {options}
        </select>
    }
}

fn checkbox(checked: bool, mark: Mark, commit: impl Fn(String) + Copy + 'static) -> impl IntoView {
    view! {
        <label class=mark.class("checkbox") title=mark.title>
            <input
                type="checkbox"
                prop:checked=checked
                on:change=move |ev| commit(if event_target_checked(&ev) { "true" } else { "false" }.to_string())
            />
            <span>{if checked { "true" } else { "false" }}</span>
        </label>
    }
}

fn text(
    current: String,
    field: StoredValue<Field>,
    nil: RwSignal<bool>,
    entry: Entry,
    input: NodeRef<html::Input>,
    mark: Mark,
    commit: impl Fn(String) + Copy + 'static,
) -> impl IntoView {
    let Entry { touched, rejected } = entry;
    let file_invalid = field.with_value(|f| f.error.is_some() && !f.nil);
    let original = current.clone();
    let on_keydown = move |ev: web_sys::KeyboardEvent| match ev.key().as_str() {
        // Leaving the field fires the change event, which writes the value
        "Enter" => {
            if let Some(element) = input.get() {
                let _ = element.blur();
            }
        }
        "Escape" => {
            if let Some(element) = input.get() {
                element.set_value(&original);
            }
            touched.set(false);
            rejected.set(None);
        }
        _ => {}
    };
    view! {
        <input
            type="text"
            class=mark.class("control")
            title=mark.title
            spellcheck="false"
            node_ref=input
            class:invalid=move || if touched.get() { rejected.with(Option::is_some) } else { file_invalid }
            prop:value=current
            prop:disabled=move || nil.get()
            placeholder=move || if nil.get() { "nil" } else { "" }
            on:input=move |ev| {
                let raw = event_target_value(&ev);
                touched.set(true);
                rejected.set(field.with_value(|f| f.check(&f.normalize(&raw))));
            }
            on:change=move |ev| commit(event_target_value(&ev))
            on:keydown=on_keydown
        />
    }
}

pub(crate) fn notes_view(notes: Vec<Note>) -> impl IntoView {
    notes
        .into_iter()
        .map(|note| {
            let class = match note.severity {
                Severity::Error => "note error",
                Severity::Warning => "note warning",
            };
            view! { <div class=class>{note.message}</div> }
        })
        .collect_view()
}

pub(crate) fn variant_badge(variant: &Variant) -> Option<impl IntoView + use<>> {
    let (class, text) = match variant {
        Variant::Plain => return None,
        Variant::Selected { selector } => ("badge used", format!("used: chosen by {selector}")),
        Variant::Unselected { selector, value } if value.is_empty() => {
            ("badge unused", format!("not used: {selector} is not set"))
        }
        Variant::Unselected { selector, value } => {
            ("badge unused", format!("not used: {selector} is {value}"))
        }
    };
    Some(view! { <span class=class>{text}</span> })
}

/// The button that copies a row's path (see [`crate::rows::SectionRow::copy_path`]). It shows a
/// tick for a moment after copying.
pub(crate) fn copy_button(ctx: Ctx, path: String) -> impl IntoView {
    let copied = RwSignal::new(false);
    let title = format!("Copy path: {path}");
    view! {
        <button
            class="copy"
            class:copied=move || copied.get()
            title=move || if copied.get() { "Copied".to_string() } else { title.clone() }
            aria-label="Copy path"
            on:click=move |_| {
                ctx.copy(path.clone());
                copied.set(true);
                set_timeout(move || copied.set(false), COPIED_TICK);
            }
        >
            <svg class="icon clipboard" viewBox="0 0 16 16" aria-hidden="true">
                <rect x="2.5" y="5.5" width="8" height="8" rx="1" />
                <path d="M5.5 5.5v-2a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-2" />
            </svg>
            <svg class="icon tick" viewBox="0 0 16 16" aria-hidden="true">
                <path d="M3 8.5 6.5 12 13 4.5" />
            </svg>
        </button>
    }
}

/// A value typed or picked in a control: whether there is one, and why it wasn't written.
#[derive(Clone, Copy)]
struct Entry {
    touched: RwSignal<bool>,
    rejected: RwSignal<Option<String>>,
}

/// How a control shows that its value changed: a green border until the file is saved, then blue.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    change: Change,
    pub(crate) title: Option<&'static str>,
}

impl From<Change> for Mark {
    fn from(change: Change) -> Self {
        let title = match change {
            Change::None => None,
            Change::Unsaved => Some("Changed, not saved yet"),
            Change::Saved => Some("Changed and saved"),
        };
        Mark { change, title }
    }
}

impl Mark {
    /// The control's classes: `base`, and one for the change.
    pub(crate) fn class(self, base: &str) -> String {
        match self.change {
            Change::None => base.to_string(),
            Change::Unsaved => format!("{base} unsaved"),
            Change::Saved => format!("{base} saved"),
        }
    }
}

/// An element's name, with its index when it's an item of a list: `constraint [0]`.
pub(crate) fn label(name: String, index: Option<usize>) -> String {
    match index {
        Some(index) => format!("{name} [{index}]"),
        None => name,
    }
}

/// Units as the schema writes them, except `1`, the IMAS convention for no units.
pub(crate) fn units_label(units: String) -> String {
    if units == "1" {
        "dimensionless".to_string()
    } else {
        units
    }
}
