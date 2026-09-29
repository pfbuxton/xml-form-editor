//! A matrix's view: its values in a grid, like a spreadsheet's, with its rows and columns numbered
//! from 1 as a spreadsheet numbers them, and buttons that add and remove rows and columns.
//!
//! The grid stays in place as the matrix changes (see [`Row::key`]) and rebuilds only the cells
//! whose values change, so that the cell being edited keeps its focus. Enter and the arrow keys
//! move between the cells as in a spreadsheet, and Tab moves along the row.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use xml_form_editor_core::{EditOp, Matrix, Severity};

use crate::Ctx;
use crate::fields::{Mark, copy_button, label, notes_view, row_class, units_label, variant_badge};
use crate::rows::{Change, MatrixRow, Row};

/// The narrowest and widest a column is, in characters: as wide as its widest value, within these.
/// The narrowest leaves room in a column's number for the button that removes it.
const MIN_WIDTH: usize = 4;
const MAX_WIDTH: usize = 24;

/// How long a cell that an edit makes, such as the first of a new row, waits to be focused.
const FOCUS_WAIT: Duration = Duration::from_secs(3);

/// What the parts of a grid share.
#[derive(Clone, Copy)]
struct Grid {
    ctx: Ctx,
    /// The matrix as it is now
    matrix: Memo<MatrixRow>,
    /// Its numbers of rows and columns
    shape: Memo<(usize, usize)>,
    /// Each column's width, in characters
    widths: Memo<Vec<usize>>,
    /// The cell with the focus, or that had it last, whose row and column numbers are highlighted
    active: RwSignal<Option<(usize, usize)>>,
    /// The last value typed that wasn't written, with its row, its column and why
    rejected: RwSignal<Option<(usize, usize, String)>>,
    /// A cell to focus when it appears
    wanted: StoredValue<Option<(usize, usize)>>,
    table: NodeRef<html::Table>,
}

impl Grid {
    fn edit(&self, op: EditOp) {
        let document = self.matrix.with_untracked(|m| m.place.document.clone());
        self.ctx.edit(document, op);
    }

    /// Focuses a cell and selects its value, as moving to a cell of a spreadsheet does, so that
    /// typing replaces it. Whether there is such a cell.
    fn focus(&self, row: usize, column: usize) -> bool {
        let cell = self.table.get_untracked().and_then(|table| {
            table
                .query_selector(&format!("input[data-cell=\"{row}:{column}\"]"))
                .ok()
                .flatten()
        });
        match cell.and_then(|cell| cell.dyn_into::<HtmlInputElement>().ok()) {
            Some(input) => {
                let _ = input.focus();
                input.select();
                true
            }
            None => false,
        }
    }

    /// Focuses a cell once the edit that makes it shows.
    fn focus_when_shown(&self, cell: (usize, usize)) {
        let wanted = self.wanted;
        wanted.set_value(Some(cell));
        set_timeout(
            move || {
                wanted.update_value(|w| {
                    w.take_if(|w| *w == cell);
                })
            },
            FOCUS_WAIT,
        );
    }

    /// Adds or removes a row or column.
    fn resize(&self, resize: Resize) {
        if self.why_not(resize, false).is_some() {
            return;
        }
        let (rows, columns) = self.shape.get_untracked();
        let op = self.matrix.with_untracked(|m| {
            let path = m.place.path.clone();
            let name = m.matrix.row_name.clone();
            Some(match resize {
                Resize::AddRow => EditOp::InsertRow {
                    path,
                    name,
                    index: rows,
                    value: m.matrix.new_row(new_row_width(m, columns))?,
                },
                Resize::AddColumn => EditOp::InsertColumn {
                    path,
                    name,
                    index: columns,
                    value: m.matrix.fill()?,
                },
                Resize::RemoveRow(row) => EditOp::RemoveRow {
                    path: m.lines.get(row)?.path.clone(),
                },
                Resize::RemoveColumn(column) => EditOp::RemoveColumn {
                    path,
                    name,
                    index: column,
                },
            })
        });
        let Some(op) = op else {
            return;
        };
        match resize {
            Resize::AddRow => self.focus_when_shown((rows, 0)),
            Resize::AddColumn => self.focus_when_shown((0, columns)),
            Resize::RemoveRow(_) | Resize::RemoveColumn(_) => {}
        }
        self.edit(op);
    }

    /// Why a row or column can't be added or removed, if it can't; read reactively when `track`.
    fn why_not(&self, resize: Resize, track: bool) -> Option<String> {
        let why = |m: &MatrixRow| {
            let (rows, columns) = (m.lines.len(), m.columns());
            match resize {
                Resize::AddRow if !m.matrix.allows_rows(rows + 1) => {
                    Some(format!("The schema allows at most {}", count(rows, "row")))
                }
                Resize::AddRow => m
                    .matrix
                    .new_row(new_row_width(m, columns))
                    .is_none()
                    .then(|| "The schema allows no row of values to start it with".to_string()),
                Resize::AddColumn if rows == 0 => Some("Add a row first".to_string()),
                Resize::AddColumn => m.matrix.new_row(columns + 1).is_none().then(|| {
                    format!(
                        "The schema allows at most {} in a row",
                        count(columns, "value")
                    )
                }),
                Resize::RemoveRow(_) if rows == 0 || !m.matrix.allows_rows(rows - 1) => {
                    Some(format!("The schema needs at least {}", count(rows, "row")))
                }
                Resize::RemoveColumn(_)
                    if columns == 0 || m.matrix.new_row(columns - 1).is_none() =>
                {
                    Some(format!(
                        "The schema needs at least {} in a row",
                        count(columns, "value")
                    ))
                }
                Resize::RemoveRow(_) | Resize::RemoveColumn(_) => None,
            }
        };
        if track {
            self.matrix.with(why)
        } else {
            self.matrix.with_untracked(why)
        }
    }

    /// Forgets a value typed in a cell that wasn't written.
    fn clear_rejected(&self, row: usize, column: usize) {
        if self
            .rejected
            .with_untracked(|r| r.as_ref().is_some_and(|r| (r.0, r.1) == (row, column)))
        {
            self.rejected.set(None);
        }
    }
}

/// How many values a new row has: as many as the others, or for the first row, as few as its type
/// allows, and at least one.
fn new_row_width(m: &MatrixRow, columns: usize) -> usize {
    if !m.lines.is_empty() {
        return columns;
    }
    let ty = &m.matrix.row_type;
    ty.length
        .or(ty.min_length)
        .map_or(1, |n| (n as usize).max(1))
}

pub(crate) fn matrix_view(row: MatrixRow, ctx: Ctx) -> impl IntoView {
    let MatrixRow {
        place,
        name,
        index,
        copy_path,
        ..
    } = row.clone();
    let key = place.key.clone();
    let matrix = Memo::new(move |previous: Option<&MatrixRow>| {
        ctx.rows
            .with(|rows| {
                rows.iter().find_map(|r| match r {
                    Row::Matrix(m) if m.place.key == key => Some(m.clone()),
                    _ => None,
                })
            })
            // Kept while the row goes, as when it's collapsed
            .or_else(|| previous.cloned())
            .unwrap_or_else(|| row.clone())
    });
    let grid = Grid {
        ctx,
        matrix,
        shape: Memo::new(move |_| matrix.with(|m| (m.lines.len(), m.columns()))),
        widths: Memo::new(move |_| matrix.with(column_widths)),
        active: RwSignal::new(None),
        rejected: RwSignal::new(None),
        wanted: StoredValue::new(None),
        table: NodeRef::new(),
    };
    let empty = Memo::new(move |_| grid.shape.with(|(rows, _)| *rows == 0));
    let problems = Memo::new(move |_| matrix.with(value_problems));
    let problem = move || {
        !problems.with(Vec::is_empty)
            || matrix.with(|m| m.notes.iter().any(|n| n.severity == Severity::Error))
    };
    let reveal = move |_| matrix.with_untracked(|m| ctx.reveal(&m.place.document, m.offset));
    let shape = move || match grid.shape.get() {
        (0, _) => ("no rows".to_string(), None),
        (rows, columns) => (
            format!("{rows} × {columns}"),
            Some(format!(
                "{}, {}",
                count(rows, "row"),
                count(columns, "column")
            )),
        ),
    };
    // Focus leaving the grid for somewhere else clears the highlighted row and column; focus lost
    // as a cell is rebuilt keeps them, so that the cell takes the focus back (see `cell_view`)
    let on_focusout = move |ev: web_sys::FocusEvent| {
        let Some(to) = ev.related_target() else {
            return;
        };
        let inside = grid.table.get_untracked().is_some_and(|table| {
            to.dyn_ref::<web_sys::Node>()
                .is_some_and(|to| table.contains(Some(to)))
        });
        if !inside {
            grid.active.set(None);
        }
    };

    view! {
        <div
            class=move || {
                let problem = problem();
                matrix.with(|m| row_class("value matrix", m.depth, m.unused, problem))
            }
            data-path=place.key.clone()
        >
            <div class="title">
                <span class="name" title="Show in the XML" on:click=reveal>{label(name, index)}</span>
                <span class="type" title=move || matrix.with(type_title)>{move || matrix.with(type_summary)}</span>
                {move || {
                    let (text, title) = shape();
                    view! { <span class="shape" title=title>{text}</span> }.into_any()
                }}
                {move || matrix.with(|m| m.units.clone()).map(|units| view! { <span class="units" title="Units">{units_label(units)}</span> }.into_any())}
                {move || matrix.with(|m| variant_badge(&m.variant).map(IntoAny::into_any))}
                {copy_path.map(|path| copy_button(ctx, path))}
            </div>
            {move || matrix.with(|m| m.doc.clone()).map(|doc| view! { <div class="doc">{doc}</div> }.into_any())}
            // Hidden while there are no rows, rather than rebuilt when the first comes
            <div class="grid-scroll" class:hidden=move || empty.get()>
                <div class="grid-frame">
                    <table class="grid" node_ref=grid.table on:focusout=on_focusout>
                        <thead>
                            <tr>
                                <th class="corner"></th>
                                <For each=move || 0..grid.shape.get().1 key=|column| *column children=move |column| column_header(grid, column) />
                            </tr>
                        </thead>
                        <tbody>
                            <For each=move || 0..grid.shape.get().0 key=|row| *row children=move |row| grid_row(grid, row) />
                        </tbody>
                    </table>
                    <div class="add-column">{resize_button(grid, Resize::AddColumn, None)}</div>
                </div>
                <div class="add-row">{resize_button(grid, Resize::AddRow, None)}</div>
            </div>
            {move || empty.get().then(|| resize_button(grid, Resize::AddRow, Some("Add a row")))}
            {move || {
                grid.rejected.get().map(|(row, column, error)| {
                    view! { <div class="error">{format!("Not saved: row {}, column {}: {error}", row + 1, column + 1)}</div> }.into_any()
                })
            }}
            {move || problems.get().into_iter().map(|problem| view! { <div class="error">{problem}</div> }).collect_view()}
            {move || notes_view(matrix.with(|m| m.notes.clone()))}
        </div>
    }
}

/// A change to a matrix's shape.
#[derive(Clone, Copy, PartialEq)]
enum Resize {
    AddRow,
    AddColumn,
    RemoveRow(usize),
    RemoveColumn(usize),
}

impl Resize {
    fn describe(self) -> String {
        match self {
            Resize::AddRow => "Add a row".to_string(),
            Resize::AddColumn => "Add a column".to_string(),
            Resize::RemoveRow(row) => format!("Remove row {}", row + 1),
            Resize::RemoveColumn(column) => format!("Remove column {}", column + 1),
        }
    }
}

/// A button that adds or removes a row or column, disabled with the reason when it can't. Those
/// that remove are in the row and column numbers, and hidden when disabled.
fn resize_button(grid: Grid, resize: Resize, text: Option<&'static str>) -> AnyView {
    let adds = matches!(resize, Resize::AddRow | Resize::AddColumn);
    let class = match (adds, text) {
        (_, Some(_)) => "add-first",
        (true, None) => "icon-button",
        (false, None) => "remove icon-button",
    };
    view! {
        <button
            class=class
            // Tab moves along the cells, not the buttons in their numbers
            tabindex=if adds { "0" } else { "-1" }
            title=move || grid.why_not(resize, true).unwrap_or_else(|| resize.describe())
            aria-label=resize.describe()
            prop:disabled=move || grid.why_not(resize, true).is_some()
            on:click=move |_| grid.resize(resize)
        >
            <svg class="icon" viewBox="0 0 16 16" aria-hidden="true">
                <path d=if adds { "M8 3.5v9M3.5 8h9" } else { "M5 5l6 6M11 5l-6 6" } />
            </svg>
            {text}
        </button>
    }
    .into_any()
}

fn column_header(grid: Grid, column: usize) -> AnyView {
    let active = move || grid.active.with(|a| a.is_some_and(|(_, c)| c == column));
    view! {
        <th scope="col" class="index" class:active=active>
            {column + 1}
            {resize_button(grid, Resize::RemoveColumn(column), None)}
        </th>
    }
    .into_any()
}

fn grid_row(grid: Grid, row: usize) -> AnyView {
    let active = move || grid.active.with(|a| a.is_some_and(|(r, _)| r == row));
    let title = move || {
        grid.matrix
            .with(|m| m.matrix.row_doc.clone())
            .map(|doc| format!("Row {}: {doc}", row + 1))
    };
    view! {
        <tr>
            <th scope="row" class="index" class:active=active title=title>
                {resize_button(grid, Resize::RemoveRow(row), None)}
                {row + 1}
            </th>
            <For each=move || cells(grid, row) key=Cell::key children=move |cell| cell_view(grid, cell) />
        </tr>
    }
    .into_any()
}

/// A cell of the grid, with all it shows.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Cell {
    row: usize,
    column: usize,
    /// `None` past the end of a row that's shorter than others
    value: Option<String>,
    /// Why the value isn't valid
    error: Option<String>,
    change: Change,
}

impl Cell {
    /// Identifies the cell for keyed rendering: its column, and a hash of all it shows.
    fn key(&self) -> (usize, u64) {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        (self.column, hasher.finish())
    }
}

fn cells(grid: Grid, row: usize) -> Vec<Cell> {
    grid.matrix.with(|m| {
        let Some(line) = m.lines.get(row) else {
            return Vec::new();
        };
        let ty = m.matrix.value_type();
        (0..m.columns())
            .map(|column| {
                let value = line.values.get(column).cloned();
                Cell {
                    row,
                    column,
                    error: value
                        .as_deref()
                        .zip(ty)
                        .and_then(|(value, ty)| ty.validate(value).err()),
                    change: line.changes.get(column).copied().unwrap_or(Change::None),
                    value,
                }
            })
            .collect()
    })
}

/// Whether the page's focus is lost, as when the element holding it goes.
fn focus_lost() -> bool {
    document()
        .active_element()
        .is_none_or(|element| element.tag_name() == "BODY")
}

/// A cell: an input for its value. Only valid values are written; one that isn't stays in the cell,
/// with the reason below the grid, until it's fixed or Escape restores the value in the file.
fn cell_view(grid: Grid, cell: Cell) -> AnyView {
    let Cell {
        row,
        column,
        value,
        error,
        change,
    } = cell;
    let missing = value.is_none();
    let current = StoredValue::new(value.unwrap_or_default());
    let file_error = StoredValue::new(error.clone());
    // Why the value typed isn't valid: the file value's error until something is typed
    let invalid = RwSignal::new(error);
    let input = NodeRef::<html::Input>::new();

    // A cell rebuilt while it had the focus takes it back, as does one an edit made to be focused
    let wanted = grid.wanted.with_value(|w| *w == Some((row, column)));
    if wanted || grid.active.get_untracked() == Some((row, column)) {
        request_animation_frame(move || {
            let wanted = grid
                .wanted
                .try_update_value(|w| w.take_if(|w| *w == (row, column)).is_some())
                .unwrap_or(false);
            if wanted || (grid.active.get_untracked() == Some((row, column)) && focus_lost()) {
                grid.focus(row, column);
            }
        });
    }

    let unchanged = move |raw: &str| {
        let raw = raw.trim();
        current.with_value(|c| raw == c) || (missing && raw.is_empty())
    };
    // The value typed, or why it can't be written
    let check = move |raw: &str| -> Result<String, String> {
        let value = raw.trim();
        if value.split_whitespace().nth(1).is_some() {
            return Err("A cell holds one value, with no spaces".to_string());
        }
        grid.matrix.with_untracked(|m| {
            m.matrix
                .value_type()
                .map_or(Ok(()), |ty| ty.validate(value))
        })?;
        Ok(value.to_string())
    };
    let commit = move |raw: String| {
        if unchanged(&raw) {
            invalid.set(file_error.get_value());
            grid.clear_rejected(row, column);
            return;
        }
        match check(&raw) {
            Ok(value) => {
                invalid.set(None);
                grid.clear_rejected(row, column);
                let op = grid.matrix.with_untracked(|m| {
                    Some(EditOp::SetCell {
                        path: m.lines.get(row)?.path.clone(),
                        column,
                        fill: fill(&m.matrix, &value),
                        value,
                    })
                });
                if let Some(op) = op {
                    grid.edit(op);
                }
            }
            Err(error) => {
                invalid.set(Some(error.clone()));
                grid.rejected.set(Some((row, column, error)));
            }
        }
    };

    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        let Some(element) = input.get_untracked() else {
            return;
        };
        let raw = element.value();
        let length = raw.encode_utf16().count() as u32;
        let (start, end) = (
            element.selection_start().ok().flatten(),
            element.selection_end().ok().flatten(),
        );
        let all = start == Some(0) && end == Some(length);
        let to = match ev.key().as_str() {
            "Enter" if ev.shift_key() => row.checked_sub(1).map(|r| (r, column)),
            "Enter" | "ArrowDown" => Some((row + 1, column)),
            "ArrowUp" => row.checked_sub(1).map(|r| (r, column)),
            // Along the row from the ends of the value, or when it's all selected
            "ArrowLeft" if all || (start == Some(0) && end == Some(0)) => {
                column.checked_sub(1).map(|c| (row, c))
            }
            "ArrowRight" if all || (start == Some(length) && end == Some(length)) => {
                Some((row, column + 1))
            }
            "Escape" => {
                element.set_value(&current.get_value());
                element.select();
                invalid.set(file_error.get_value());
                grid.clear_rejected(row, column);
                return;
            }
            _ => return,
        };
        ev.prevent_default();
        // A value that can't be written keeps the focus, to fix
        if !unchanged(&raw) && check(&raw).is_err() {
            commit(raw);
            element.select();
            return;
        }
        // Leaving the cell writes its value
        let moved = to.is_some_and(|(r, c)| grid.focus(r, c));
        if !moved && ev.key() == "Enter" {
            commit(raw);
            element.select();
        }
    };

    let mark = Mark::from(change);
    let width = move || {
        let chars = grid
            .widths
            .with(|w| w.get(column).copied().unwrap_or(MIN_WIDTH));
        format!("width: calc({chars}ch + 14px)")
    };
    view! {
        <td
            class=mark.class("cell")
            class:missing=missing
            class:invalid=move || invalid.with(Option::is_some)
            title=move || invalid.get().or(mark.title.map(str::to_string))
        >
            <input
                type="text"
                spellcheck="false"
                autocomplete="off"
                node_ref=input
                data-cell=format!("{row}:{column}")
                aria-label=format!("Row {}, column {}", row + 1, column + 1)
                prop:value=current.get_value()
                style=width
                on:focus=move |_| grid.active.set(Some((row, column)))
                on:input=move |ev| {
                    let raw = event_target_value(&ev);
                    invalid.set(if unchanged(&raw) { file_error.get_value() } else { check(&raw).err() });
                }
                on:change=move |ev| commit(event_target_value(&ev))
                on:keydown=on_keydown
            />
        </td>
    }
    .into_any()
}

/// What pads a row too short for a value: the value new cells start with, or else the value itself.
fn fill(matrix: &Matrix, value: &str) -> String {
    matrix.fill().unwrap_or_else(|| value.to_string())
}

/// Each column's width in characters: its widest value's, within [`MIN_WIDTH`] and [`MAX_WIDTH`].
fn column_widths(m: &MatrixRow) -> Vec<usize> {
    (0..m.columns())
        .map(|column| {
            m.lines
                .iter()
                .filter_map(|line| line.values.get(column))
                .map(|value| value.chars().count())
                .max()
                .unwrap_or(0)
                .clamp(MIN_WIDTH, MAX_WIDTH)
        })
        .collect()
}

/// The problems with the values, one line each: each invalid value, and each row that isn't valid
/// for another reason, such as how many values it has.
fn value_problems(m: &MatrixRow) -> Vec<String> {
    let ty = m.matrix.value_type();
    let mut out = Vec::new();
    for (r, line) in m.lines.iter().enumerate() {
        let before = out.len();
        for (c, value) in line.values.iter().enumerate() {
            if let Some(error) = ty.and_then(|ty| ty.validate(value).err()) {
                out.push(format!("Row {}, column {}: {error}", r + 1, c + 1));
            }
        }
        if let Some(error) = line.error.as_ref().filter(|_| out.len() == before) {
            out.push(format!("Row {}: {error}", r + 1));
        }
    }
    out
}

/// What the type line says: what the values are.
fn type_summary(m: &MatrixRow) -> String {
    match m.matrix.value_type() {
        Some(ty) => format!("matrix of {}", ty.summary()),
        None => "matrix".to_string(),
    }
}

/// The type line's tooltip: what each row is.
fn type_title(m: &MatrixRow) -> String {
    let row_type = &m.matrix.row_type;
    let mut title = format!(
        "Each row is a <{}> holding a {}",
        m.matrix.row_name,
        row_type.summary()
    );
    if let Some(doc) = &row_type.doc {
        title.push_str("\n\n");
        title.push_str(doc);
    }
    title
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}
