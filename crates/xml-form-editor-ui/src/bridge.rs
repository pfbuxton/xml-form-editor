//! Messages between the webview and the extension (`src/extension.ts`).

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use xml_form_editor_core::TextEdit;

#[wasm_bindgen]
extern "C" {
    type VsCodeApi;

    #[wasm_bindgen(js_name = acquireVsCodeApi)]
    fn acquire_vs_code_api() -> VsCodeApi;

    #[wasm_bindgen(method, js_name = postMessage)]
    fn post_message(this: &VsCodeApi, message: &JsValue);
}

thread_local! {
    // acquireVsCodeApi may only be called once
    static VSCODE: VsCodeApi = acquire_vs_code_api();
}

/// A message to the extension. A `document` is a file the form shows, named as the extension
/// names it: the file the form was opened on, or one it links to.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum Out {
    /// The webview is ready for the document
    Ready,
    /// Read the schema a document names, whose location is relative to the document
    RequestSchema { document: String, location: String },
    /// Read the file a document links to, relative to the document, and follow its changes, then
    /// answer with `link`
    OpenLink { document: String, href: String },
    /// Apply edits made against this version of a document, then answer with `editDone`
    Edit {
        document: String,
        id: u32,
        version: f64,
        edits: Vec<Edit>,
    },
    /// Show this offset of a document in the text editor
    Reveal { document: String, offset: u32 },
    /// Open the document the form was opened on in the text editor
    OpenSource,
    /// Open a document's schema in the text editor
    OpenSchema { document: String },
    /// Save the documents the form shows
    Save,
    /// Copy this text to the clipboard
    Copy { text: String },
}

#[derive(Serialize)]
pub(crate) struct Edit {
    start: u32,
    end: u32,
    text: String,
}

impl From<TextEdit> for Edit {
    fn from(edit: TextEdit) -> Self {
        Edit {
            start: edit.start,
            end: edit.end,
            text: edit.text,
        }
    }
}

/// A message from the extension:
///
/// - `document`, with a document's `uri`, `text`, `version`, whether it's `dirty`, and whether it's
///   the `main` one, which the form was opened on
/// - `saved`, with the `uri` and `text` of a file on disk: as the form opens, after each save, and
///   when the file changes
/// - `schema`, with the `document` naming it and the `location` it names, and `text` or `error`
/// - `link`, with the `document` and the `href` it links to, and the `uri` of that file or `error`
/// - `editDone`, with `id` and `applied`, which comes after the `document` holding the edit
#[derive(Deserialize)]
pub(crate) struct In {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub main: Option<bool>,
    #[serde(default)]
    pub document: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub version: Option<f64>,
    #[serde(default)]
    pub dirty: Option<bool>,
    #[serde(default)]
    pub id: Option<f64>,
    #[serde(default)]
    pub applied: Option<bool>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub href: Option<String>,
    /// Where the schema was read from
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub(crate) fn post(message: &Out) {
    match serde_wasm_bindgen::to_value(message) {
        Ok(value) => VSCODE.with(|api| api.post_message(&value)),
        Err(error) => web_sys::console::error_1(&error.into()),
    }
}

pub(crate) fn on_message(mut handler: impl FnMut(In) + 'static) {
    let listener =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            match serde_wasm_bindgen::from_value::<In>(event.data()) {
                Ok(message) => handler(message),
                Err(error) => web_sys::console::warn_1(&error.into()),
            }
        });
    if let Some(window) = web_sys::window() {
        let _ =
            window.add_event_listener_with_callback("message", listener.as_ref().unchecked_ref());
    }
    // The listener lives as long as the page
    listener.forget();
}
