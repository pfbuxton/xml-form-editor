# XML Form Editor

Editing XML by hand is unpleasant and error-prone: one mistyped value or lost closing tag, and you
only find out when the code that reads the file fails.

XML Form Editor opens an XML file as a form instead. The structure of the file stays fixed, and you
only change the values. It was written for the input files of IMAS codes, but it works with any XML
file that has an XSD schema. The form is built from the schema, which the file names in its root
element with `xsi:noNamespaceSchemaLocation` or `xsi:schemaLocation`.

## Features

- Dropdowns for enumerations, and checkboxes for booleans.
- Values are checked as you type, and invalid values are never written.
- Every setting shows its documentation and units from the schema.
- Changed values are marked green until you save, then blue.
- Sections that an enumeration such as `method` doesn't choose are dimmed and collapsed.
- Filter the settings, and copy the path of any setting.
- Settings split across files: when an element links to another XML file with `href`, the form
  shows that file's settings in its place, so you can search and edit them all from one form. Save
  saves every file you changed.

Each change edits only the text of that value, so formatting, comments and Undo keep working.

## Getting started

Open an XML file and click **Open Form Editor to the Side** in the editor title bar, or right-click
the file in the Explorer and choose **Open in Form Editor**. Save with Ctrl+S.

## Limitations

- The form never adds or removes elements, or changes which file an element links to. To do that,
  edit the text: **Open XML** in the toolbar opens it beside the form.
- The schema must be a local file, and `xs:include`, `xs:import` and `xs:redefine` aren't supported.
- Undo in the form undoes changes to the file you opened. Changes to a linked file are undone in
  that file's text editor.
