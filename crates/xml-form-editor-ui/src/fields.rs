//! The rows' views: section headings, and the controls that edit values, which are greyed out in
//! the form of a schema.

use std::sync::Arc;
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, HtmlInputElement};
use xml_form_editor_core::{AttributeField, EditOp, EnumValue, Field, Note, Occurs, Severity, Variant};

use crate::Ctx;
use crate::rows::{AddRow, AttributeRow, Change, LinkedFile, Place, Row, SectionRow, ValueRow};

/// Rows deeper than this are indented no further.
const MAX_INDENT: usize = 12;

/// How long a copy button shows its tick.
const COPIED_TICK: Duration = Duration::from_millis(1500);

/// How long typing in a field pauses before what's typed is written.
const TYPING_PAUSE: Duration = Duration::from_millis(300);

pub(crate) fn row_view(row: Row, ctx: Ctx) -> AnyView {
    let key = match &row {
        Row::Section(row) => Some(&row.place.key),
        Row::Value(row) => Some(&row.place.key),
        Row::Matrix(row) => Some(&row.place.key),
        Row::Attribute(_) | Row::Add(_) => None,
    };
    if let Some(key) = key.filter(|key| ctx.take_focus(key)) {
        focus_item(key.clone());
    }
    match row {
        Row::Section(row) => section_view(row, ctx).into_any(),
        Row::Value(row) => value_view(row, ctx).into_any(),
        Row::Attribute(row) => attribute_view(row, ctx).into_any(),
        Row::Matrix(row) => crate::matrix::matrix_view(row, ctx).into_any(),
        Row::Add(row) => add_view(row, ctx).into_any(),
    }
}

/// Once the rows show, focuses the first control of the item whose row has this key, in its row or
/// the rows of what it holds, and selects its value, so that typing replaces it. An item without
/// one is scrolled to.
fn focus_item(key: String) {
    request_animation_frame(move || {
        let key = key.replace('\\', "\\\\").replace('"', "\\\"");
        let find = |selector: String| document().query_selector(&selector).ok().flatten();
        let control = find(format!(
            "[data-path=\"{key}\"] input, [data-path=\"{key}\"] select, \
             [data-path^=\"{key}/\"] input, [data-path^=\"{key}/\"] select"
        ));
        match control {
            Some(control) => {
                if let Some(control) = control.dyn_ref::<HtmlElement>() {
                    let _ = control.focus();
                }
                if let Some(input) = control.dyn_ref::<HtmlInputElement>()
                    && input.type_() == "text"
                {
                    input.select();
                }
            }
            None => {
                if let Some(row) = find(format!("[data-path=\"{key}\"]")) {
                    row.scroll_into_view();
                }
            }
        }
    });
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
        linked_file,
        remove,
        occurs,
    } = row;
    let problem = notes.iter().any(|n| n.severity == Severity::Error);
    let Place { document, key, .. } = place;
    let label = label(name, index);
    let remove = remove.map(|remove| remove_button(ctx, document, &label, remove));
    let toggle = {
        let key = key.clone();
        move |_: web_sys::MouseEvent| {
            ctx.expanded.update(|sections| {
                sections.insert(key.clone(), !expanded);
            })
        }
    };
    let toggle_title = if expanded { "Collapse" } else { "Expand" };
    // The name folds the section, as the twisty does
    let name = if empty {
        view! { <span class="name">{label}</span> }.into_any()
    } else {
        view! { <span class="name toggle" title=toggle_title on:click=toggle.clone()>{label}</span> }.into_any()
    };
    view! {
        <div class=row_class("section", depth, unused, problem) data-path=key>
            <div class="title">
                {(!empty).then(|| view! {
                    <button
                        class=if expanded { "twisty open" } else { "twisty" }
                        title=toggle_title
                        on:click=toggle
                    >
                        <svg class="chevron" viewBox="0 0 16 16" aria-hidden="true">
                            <path d="M6 3.5 10.5 8 6 12.5" />
                        </svg>
                    </button>
                })}
                {name}
                {occurs_badge(occurs)}
                {linked_file.map(|file| file_badge(ctx, file))}
                {variant_badge(&variant)}
                {copy_path.map(|path| copy_button(ctx, path))}
                {remove}
            </div>
            {doc.map(|doc| view! { <div class="doc">{doc_text(doc)}</div> })}
            {empty.then(|| view! { <div class="doc">"(no settings)"</div> })}
            {notes_view(notes)}
        </div>
    }
}

/// The name of the file a section shows, which opens it in a form of its own.
fn file_badge(ctx: Ctx, file: LinkedFile) -> impl IntoView {
    let LinkedFile { href, document } = file;
    view! {
        <span class="file" title="The file these settings are in. Click to open it in the form editor." on:click=move |_| ctx.open_form(&document)>
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
        units,
        change,
        remove,
        occurs,
    } = row;
    let problem = field.error.is_some() || notes.iter().any(|n| n.severity == Severity::Error);
    let summary = field.ty.as_ref().map(|ty| ty.summary());
    let type_doc = field.ty.as_ref().and_then(|ty| ty.doc.clone());
    let key = place.key.clone();
    let label = label(name, index);
    let remove = remove.map(|remove| remove_button(ctx, place.document.clone(), &label, remove));
    view! {
        <div class=row_class("value", depth, unused, problem) data-path=key>
            <div class="title">
                <span class="name">{label}</span>
                {occurs_badge(occurs)}
                {summary.map(|summary| view! { <span class="type" title=type_doc>{summary}</span> })}
                {variant_badge(&variant)}
                {copy_path.map(|path| copy_button(ctx, path))}
                {remove}
            </div>
            {doc.map(|doc| view! { <div class="doc">{doc_text(doc)}</div> })}
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
        change,
    } = row;
    let AttributeField {
        name,
        doc,
        field,
        units,
        warning,
        occurs,
    } = attribute;
    let problem = field.error.is_some();
    let summary = field.ty.as_ref().map(|ty| ty.summary());
    let key = format!("{}/@{name}", place.key);
    let label = format!("@{name}");
    view! {
        <div class=row_class("value attribute", depth, unused, problem) data-path=key>
            <div class="title">
                <span class="name">{label}</span>
                {occurs_badge(occurs)}
                {summary.map(|summary| view! { <span class="type">{summary}</span> })}
                {copy_button(ctx, copy_path)}
            </div>
            {doc.map(|doc| view! { <div class="doc">{doc_text(doc)}</div> })}
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

    /// Identifies the field (see [`Typing::key`]).
    fn key(&self) -> String {
        match self {
            Target::Element(place) => place.key.clone(),
            Target::Attribute(place, name) => format!("{}/@{name}", place.key),
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
/// A value typed is written as typing pauses, and one picked straight away. Only valid values are
/// written: an invalid one stays in the control with the reason it wasn't written, until it's fixed
/// or Escape restores the value in the file. In the form of a schema, the control is greyed out
/// (see [`declared_editor`]).
fn editor(ctx: Ctx, target: Target, field: Field, units: Option<String>, change: Change) -> AnyView {
    if ctx.read_only.get_untracked() {
        return declared_editor(field, units).into_any();
    }
    // Set once the user types or picks a value, which then decides what error shows
    let touched = RwSignal::new(false);
    let rejected = RwSignal::new(None::<String>);
    // Unchecking nil enables the control; the file stays nil until a value is written
    let nil = RwSignal::new(field.nil);
    let input = NodeRef::<html::Input>::new();

    let widget = widget(&field);
    let current = if field.nil { String::new() } else { field.display_value() };
    let (file_nil, nillable, file_error) = (field.nil, field.nillable, field.error.clone());
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
        Widget::Checkbox => checkbox(matches!(current.as_str(), "true" | "1"), mark, commit).into_any(),
        Widget::Text => {
            let key = target.with_value(Target::key);
            let entry = Entry { touched, rejected, nil, input };
            text(ctx, key, current, field, entry, mark, commit).into_any()
        }
    };

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
        <div class="control-line">{control}{units_view(units)}{nil_toggle}</div>
        {move || (file_nil && !nil.get()).then(|| view! { <div class="hint">"The file keeps nil until you enter a value."</div> })}
        {move || {
            if touched.get() {
                rejected.get().map(|error| view! { <div class="error">"Not saved: " {error}</div> }.into_any())
            } else {
                file_error.clone().map(|error| view! { <div class="error">{error}</div> }.into_any())
            }
        }}
    }
    .into_any()
}

/// The control of a value in the form of a schema, greyed out, as a schema has no values: it holds
/// the value the schema fixes, if any. Below an enumeration are the values it allows, which its
/// dropdown can't show, with each one's documentation as its tooltip.
fn declared_editor(field: Field, units: Option<String>) -> impl IntoView {
    let value = field.value.clone();
    let control = match widget(&field) {
        Widget::Choice(_) => view! {
            <select class="control" disabled=true>
                <option>{value}</option>
            </select>
        }
        .into_any(),
        Widget::Checkbox => {
            let checked = matches!(value.as_str(), "true" | "1");
            view! {
                <label class="checkbox">
                    <input type="checkbox" disabled=true prop:checked=checked />
                </label>
            }
            .into_any()
        }
        Widget::Text => view! { <input type="text" class="control" disabled=true prop:value=value /> }.into_any(),
    };
    let nil = field.nillable.then(|| {
        view! {
            <label class="nil" title="xsi:nil: the element may have no value">
                <input type="checkbox" disabled=true />
                "nil"
            </label>
        }
    });
    let allowed = field.ty.map(|ty| ty.enumeration).filter(|values| !values.is_empty()).map(|values| {
        let values = values
            .into_iter()
            .enumerate()
            .map(|(i, EnumValue { value, doc })| view! { {(i > 0).then_some(", ")}<code title=doc>{value}</code> })
            .collect_view();
        view! { <div class="option-doc allowed">"One of: " {values}</div> }
    });
    view! {
        <div class="control-line">{control}{units_view(units)}{nil}</div>
        {allowed}
    }
}

fn units_view(units: Option<String>) -> Option<impl IntoView + use<>> {
    units.map(|units| view! { <span class="units" title="Units">{units_label(units)}</span> })
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
    ctx: Ctx,
    key: String,
    current: String,
    field: StoredValue<Field>,
    entry: Entry,
    mark: Mark,
    commit: impl Fn(String) + Copy + 'static,
) -> impl IntoView {
    let Entry { touched, rejected, nil, input } = entry;
    let file_invalid = field.with_value(|f| f.error.is_some() && !f.nil);
    let original = current.clone();
    let key = StoredValue::new(key);
    let pause = Pause::new();
    // Whether what's typed can be written shows straight away
    let check = move |raw: &str| {
        touched.set(true);
        rejected.set(field.with_value(|f| f.check(&f.normalize(raw))));
    };

    // Writing what's typed rebuilds the field, and this one takes the typing on
    if let Some(typing) = key.with_value(|key| ctx.typed(key)) {
        request_animation_frame(move || {
            if input.is_disposed() || !focus_lost() {
                return;
            }
            if let Some(ahead) = input.get_untracked().and_then(|element| typing.restore(&element)) {
                check(&ahead);
                pause.then(move || commit(ahead));
            }
        });
    }

    let on_keydown = move |ev: web_sys::KeyboardEvent| match ev.key().as_str() {
        // Leaving the field fires the change event, which writes the value
        "Enter" => {
            if let Some(element) = input.get() {
                let _ = element.blur();
            }
        }
        "Escape" => {
            pause.cancel();
            key.with_value(|key| ctx.stop_typing(key));
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
                let element = event_target::<HtmlInputElement>(&ev);
                ctx.typing(Typing::of(key.get_value(), &element));
                let raw = element.value();
                check(&raw);
                pause.then(move || commit(raw));
            }
            on:change=move |ev| {
                pause.cancel();
                key.with_value(|key| ctx.stop_typing(key));
                commit(event_target_value(&ev));
            }
            on:blur=move |ev| left(ctx, key.get_value(), &ev)
            on:keydown=on_keydown
        />
    }
}

/// What's being typed in a text field or a matrix's cell. It's written as typing pauses (see
/// [`Pause`]), which rebuilds the field, and the field rebuilt from it takes on what's typed, which
/// may be ahead of what's written, with the caret where it was, so that typing carries on.
#[derive(Clone)]
pub(crate) struct Typing {
    /// The field's row's key (see [`Place::key`]), and for an attribute or a cell, which one
    pub key: String,
    text: String,
    selection: (u32, u32),
}

impl Typing {
    pub(crate) fn of(key: String, element: &HtmlInputElement) -> Typing {
        let start = element.selection_start().ok().flatten().unwrap_or(0);
        let end = element.selection_end().ok().flatten().unwrap_or(start);
        Typing {
            key,
            text: element.value(),
            selection: (start, end),
        }
    }

    /// Focuses the field rebuilt from the one typed in, with what was typed and the caret where it
    /// was. What was typed, when it's ahead of what the field shows, as it's still to be written.
    pub(crate) fn restore(self, element: &HtmlInputElement) -> Option<String> {
        let ahead = element.value() != self.text;
        if ahead {
            element.set_value(&self.text);
        }
        let _ = element.focus();
        let _ = element.set_selection_range(self.selection.0, self.selection.1);
        ahead.then_some(self.text)
    }
}

/// Waits for typing in a field to pause before writing what's typed. What's waiting is dropped
/// when the field goes, as the field rebuilt from it takes the typing on (see [`Typing`]).
#[derive(Clone, Copy)]
pub(crate) struct Pause(StoredValue<Option<TimeoutHandle>>);

impl Pause {
    pub(crate) fn new() -> Pause {
        let pause = Pause(StoredValue::new(None));
        on_cleanup(move || pause.cancel());
        pause
    }

    /// Writes once typing pauses, instead of what was waiting.
    pub(crate) fn then(self, write: impl FnOnce() + 'static) {
        self.cancel();
        self.0.set_value(set_timeout_with_handle(write, TYPING_PAUSE).ok());
    }

    pub(crate) fn cancel(self) {
        if let Some(handle) = self.0.try_update_value(Option::take).flatten() {
            handle.clear();
        }
    }
}

/// Typing in a field is over once the focus leaves it, unless that's as the field is rebuilt, when
/// the field rebuilt from it takes the typing on.
pub(crate) fn left(ctx: Ctx, key: String, ev: &web_sys::FocusEvent) {
    let element = event_target::<HtmlInputElement>(ev);
    request_animation_frame(move || {
        if element.is_connected() {
            ctx.stop_typing(&key);
        }
    });
}

/// Whether the page's focus is lost, as when the element holding it goes.
pub(crate) fn focus_lost() -> bool {
    document().active_element().is_none_or(|element| element.tag_name() == "BODY")
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
        Variant::Unselected { selector, value } if value.is_empty() => ("badge unused", format!("not used: {selector} is not set")),
        Variant::Unselected { selector, value } => ("badge unused", format!("not used: {selector} is {value}")),
        Variant::Alternative { selector, value } => ("badge used", format!("used when {selector} is {value}")),
    };
    Some(view! { <span class=class>{text}</span> })
}

/// In the form of a schema, how many of an element or attribute it allows, unless exactly one.
fn occurs_badge(occurs: Option<Occurs>) -> Option<impl IntoView + use<>> {
    let text = occurs?.label()?;
    Some(view! { <span class="badge occurs" title="How many the schema allows here">{text}</span> })
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

/// The button that removes an item of a list, `label`, by making `remove`, or that says why it
/// can't be removed.
pub(crate) fn remove_button(ctx: Ctx, document: Arc<str>, label: &str, remove: Result<EditOp, String>) -> impl IntoView + use<> {
    let action = format!("Remove {label}");
    let (title, op) = match remove {
        Ok(op) => (action.clone(), Some(op)),
        Err(why) => (why, None),
    };
    view! {
        <button
            class="remove-item"
            title=title
            aria-label=action
            disabled=op.is_none()
            on:click=move |ev: web_sys::MouseEvent| {
                // The second click of a double click would remove the item that took its place
                if ev.detail() > 1 {
                    return;
                }
                if let Some(op) = op.clone() {
                    ctx.edit(document.clone(), op);
                }
            }
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true">
                <path d="M5 5l6 6M11 5l-6 6" />
            </svg>
        </button>
    }
}

/// The button that adds an item to a list, or that says why it can't. The new item takes the
/// focus when it shows.
fn add_view(row: AddRow, ctx: Ctx) -> impl IntoView {
    let AddRow {
        place,
        depth,
        name,
        unused,
        add,
        new_key,
    } = row;
    let (title, op) = match add {
        Ok((op, adds)) => (adds, Some(op)),
        Err(why) => (why, None),
    };
    let document = place.document;
    view! {
        <div class=row_class("add-item", depth, unused, false)>
            <button
                class="add-first"
                title=title
                disabled=op.is_none()
                on:click=move |ev: web_sys::MouseEvent| {
                    // A double click adds one item
                    if ev.detail() > 1 {
                        return;
                    }
                    if let Some(op) = op.clone() {
                        ctx.focus_when_shown(new_key.clone());
                        ctx.edit(document.clone(), op);
                    }
                }
            >
                <svg class="icon" viewBox="0 0 16 16" aria-hidden="true">
                    <path d="M8 3.5v9M3.5 8h9" />
                </svg>
                {format!("Add {name}")}
            </button>
        </div>
    }
}

/// A text control's state, which its nil checkbox shares: whether a value has been typed, why it
/// wasn't written, whether nil is checked, and the input.
#[derive(Clone, Copy)]
struct Entry {
    touched: RwSignal<bool>,
    rejected: RwSignal<Option<String>>,
    nil: RwSignal<bool>,
    input: NodeRef<html::Input>,
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
    if units == "1" { "dimensionless".to_string() } else { units }
}

/// Documentation, with what it writes in backticks, `like this`, in the editor's font, as code. A
/// backtick with none after it to close it shows as written.
pub(crate) fn doc_text(doc: String) -> impl IntoView {
    code_spans(&doc)
        .into_iter()
        .map(|(text, code)| if code { view! { <code>{text}</code> }.into_any() } else { text.into_any() })
        .collect_view()
}

/// The parts of a text, without the backticks around those that are code, each with whether it is.
fn code_spans(text: &str) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`')
        && let Some(length) = rest[open + 1..].find('`')
    {
        let close = open + 1 + length;
        if open > 0 {
            parts.push((rest[..open].to_string(), false));
        }
        parts.push((rest[open + 1..close].to_string(), true));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        parts.push((rest.to_string(), false));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_spans(text: &str, expected: &[(&str, bool)]) {
        let spans = code_spans(text);
        let spans: Vec<(&str, bool)> = spans.iter().map(|(text, code)| (text.as_str(), *code)).collect();
        assert_eq!(spans, expected);
    }

    #[test]
    fn code_spans_are_what_backticks_wrap() {
        assert_spans(
            "How the `time_slices` are constructed",
            &[("How the ", false), ("time_slices", true), (" are constructed", false)],
        );
        assert_spans("`a` and `b`", &[("a", true), (" and ", false), ("b", true)]);
        assert_spans("No code", &[("No code", false)]);
    }

    #[test]
    fn a_backtick_left_open_shows_as_written() {
        assert_spans("`a` then `b", &[("a", true), (" then `b", false)]);
    }
}
