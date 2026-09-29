//! The logic of the XML Form Editor, independent of VS Code and the browser: reading an XSD,
//! building a form from an XML document and its schema, checking values, and turning a changed
//! value into minimal edits of the XML text.

mod edit;
mod form;
mod schema;
mod simple;
mod text;
mod xml;

pub use edit::{EditError, EditOp, TextEdit, apply_edit};
pub use form::{
    AttributeField, Field, Form, FormNode, Linked, Matrix, NodeKind, Note, Problems, Severity,
    Variant, build_form, values,
};
pub use schema::{Schema, SchemaError};
pub use simple::{Bound, Builtin, EnumValue, SimpleInfo};
pub use xml::{Path, Step, XmlError, schema_location};
