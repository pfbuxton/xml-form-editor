# XML Form Editor

Editing XML by hand is unpleasant and error-prone: one mistyped value or lost closing tag, and you
only find out when the code that reads the file fails.

XML Form Editor opens an XML file as a form instead. You change the values, and add and remove the
items of lists as far as the schema allows, and the rest of the file stays as it was. It was
written for the input files of IMAS codes, but it works with any XML file that has an XSD schema.
The form is built from the schema, which the file names in its root element with
`xsi:noNamespaceSchemaLocation` or `xsi:schemaLocation`.

## Features

- Dropdowns for enumerations, and checkboxes for booleans.
- Matrices show as a grid, like a spreadsheet, with their rows and columns numbered. A matrix is an
  element that the schema lets hold one repeating element of a list of numbers, one per row, such
  as `<row>0.0 1.0</row>`. Enter and the arrow keys move between the cells. The + buttons add a row
  or a column, and hovering a row or column number shows a button that removes it.
- Lists: when the schema allows more than one of an element, its items are numbered, as in
  `time [0]`. The **Add** button below a list adds an item, a copy of the last one, and the × on an
  item removes it, as far as the schema allows. An empty list keeps its Add button, which adds an
  item with the elements and attributes the schema requires.
- Values are checked as you type, and invalid values are never written.
- Every setting shows its documentation and units from the schema.
- Changed values are marked green until you save, then blue. Save is enabled while a file differs
  from what's saved, so editing a value back as it was disables it again.
- Sections that an enumeration such as `method` doesn't choose are dimmed and collapsed.
- Filter the settings, and copy the path of any setting.
- Settings split across files: when an element links to another XML file with `href`, the form
  shows that file's settings in its place, so you can search and edit them all from one form. Click
  the file's name beside the section to open it in a form of its own. Save saves every file you
  changed. Until then, the form holds the changes to a linked file itself: VS Code opens a tab for
  every file with unsaved changes, so it would otherwise open one for each linked file you change.
  Open the file, and its tab shows them as unsaved changes. The changes the form holds are backed
  up, and if VS Code closes before you save, the form has them again when it next shows the file.
- Schemas: open an XSD in the form editor to see the form it describes, with every element and
  attribute it declares, its documentation, type and units, and how many of it the schema allows,
  such as `optional` or `1 or more`. The fields are greyed out, as a schema has no values, and the
  page is dark green, so it can't be mistaken for a file's form.

Each change edits only the text it needs to, so formatting, comments and Undo keep working.

## Getting started

Open an XML file and click **Open Form Editor to the Side** in the editor title bar, or right-click
the file in the Explorer and choose **Open in Form Editor**. Save with Ctrl+S. XSD files open the
same way.

## Limitations

- The form adds and removes only the items of lists and the rows of a matrix: other elements and
  attributes, such as an optional setting the file leaves out, are added in the text, and so is a
  change to which file an element links to. **Open XML** in the toolbar opens the text beside the
  form.
- Without a schema, the form can't tell that an element is an item of a list unless there's more
  than one of it.
- The schema must be a local file, and `xs:include`, `xs:import` and `xs:redefine` aren't supported.
- Undo in the form undoes changes to the file you opened, but not those the form holds for a linked
  file: change those back in the form. VS Code's Save All doesn't save them either; the form's Save
  and Ctrl+S do.
