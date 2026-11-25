//! Result grid: renders a `ResultSet` with the virtualized table component.
//!
//! Column 0 is a fixed gutter with row numbers (counted across pages); data column `i`
//! is table column `i + 1`. Headers show the column type and a key mark on primary keys
//! (docs/spec/workspace.md §4).

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableEvent, TableState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use thaan_core::schema::TableInfo;
use thaan_core::{ResultSet, Value};

/// Something a cell's right-click menu asks the tab to do (it needs the tab's own state).
#[derive(Clone)]
pub enum GridAction {
    /// Filter the table on `column` with `op` and this value (Quick filter).
    Filter {
        column: String,
        op: thaan_core::table::FilterOp,
        value: Value,
    },
    /// Start editing a cell, as a double-click does (table row and column indices).
    Edit { row: usize, col: usize },
    /// Paste the clipboard into a cell, staged like an edit.
    Paste { row: usize, col: usize },
    /// Copy one row in a format that needs the table (Markdown, SQL INSERT).
    CopyAs {
        row: usize,
        format: thaan_core::export::ExportFormat,
    },
    /// Add a copy of a row (a prefilled Insert document dialog on MongoDB).
    CloneRow { row: usize },
    /// Stage the row's delete, or take it back.
    DeleteRow { row: usize },
    /// Show every field of the row.
    Details { row: usize },
}

pub type GridActions = Rc<dyn Fn(GridAction, &mut Window, &mut App)>;

/// Called instead of a client-side sort when the grid shows a server-paged table.
/// Receives the column name and the new direction (`Default` clears the sort).
pub type ServerSort = Box<dyn Fn(String, ColumnSort, &mut Window, &mut App)>;

pub struct ResultGrid {
    pub rs: ResultSet,
    /// Row order after a client-side sort (indices into `rs.rows`).
    order: Vec<usize>,
    /// Number of rows before this page, so the gutter counts across pages.
    row_offset: u64,
    /// Declared type and primary-key flag per column name, from the table's schema.
    meta: HashMap<String, (String, bool)>,
    /// The sort the header shows, as (data column, direction).
    sort: Option<(usize, ColumnSort)>,
    server_sort: Option<ServerSort>,
    /// Staged cell edits, keyed by (data row, data column). Applied with the table's
    /// Apply button; cleared whenever new rows arrive.
    pub edits: BTreeMap<(usize, usize), Value>,
    /// The cell being edited and its input, as (data row, data column).
    pub editing: Option<(usize, usize, Entity<InputState>)>,
    /// Rows staged for deletion (data rows), drawn in red until applied or reset.
    pub deleted: BTreeSet<usize>,
    /// Rows added with Add row (data rows past `base_rows`), inserted on Apply.
    pub added: BTreeSet<usize>,
    /// Rows the page came with; added rows sit after them.
    base_rows: usize,
    /// Rows can be edited and deleted (a table tab on a writable connection).
    pub editable: bool,
    /// Filter and edit requests from the cell menu; only table tabs set it.
    pub on_action: Option<GridActions>,
    /// The selected cell as (row, table column), mirrored from the table's events so the
    /// header and gutter can mark its column and row ([`track_focus`]).
    focus: Option<(usize, usize)>,
}

/// Keeps the grid's [`ResultGrid::focus`] in step with the table's selection.
pub fn track_focus(grid: &Entity<TableState<ResultGrid>>, cx: &mut App) {
    cx.subscribe(grid, |table, ev: &TableEvent, cx| {
        let focus = match ev {
            TableEvent::SelectCell(r, c) => Some((*r, *c)),
            TableEvent::SelectRow(r) => Some((*r, 0)),
            _ => return,
        };
        table.update(cx, |t, cx| {
            t.delegate_mut().focus = focus;
            cx.notify();
        });
    })
    .detach();
}

/// Inner padding of every grid cell. The table's own padding is turned off per column so
/// a highlight can fill the whole cell.
const CELL_PX: f32 = 8.;
/// Header rows hold two lines (name, then type): 20 + 16 + 4 of air.
pub const GRID_HEADER_H: f32 = 40.;

impl ResultGrid {
    /// `true` while the cell at table row `row_ix`, table column `col_ix` is being edited.
    fn is_editing(&self, row_ix: usize, col_ix: usize) -> bool {
        col_ix > 0
            && self.editing.as_ref().is_some_and(|(r, c, _)| {
                Some(*r) == self.order.get(row_ix).copied() && *c == col_ix - 1
            })
    }

    pub fn new(rs: ResultSet) -> Self {
        let order = (0..rs.rows.len()).collect();
        Self {
            rs,
            order,
            row_offset: 0,
            meta: HashMap::new(),
            sort: None,
            server_sort: None,
            edits: BTreeMap::new(),
            editing: None,
            deleted: BTreeSet::new(),
            added: BTreeSet::new(),
            base_rows: 0,
            editable: false,
            on_action: None,
            focus: None,
        }
    }

    /// Replaces the rows. A client-side sort is dropped; a server sort is kept, since the
    /// new rows already come back in that order.
    pub fn set(&mut self, rs: ResultSet) {
        self.order = (0..rs.rows.len()).collect();
        self.focus = None;
        if self.server_sort.is_none() {
            self.sort = None;
        }
        self.base_rows = rs.rows.len();
        self.rs = rs;
        self.edits.clear();
        self.editing = None;
        self.deleted.clear();
        self.added.clear();
    }

    /// Adds an empty row at the end, staged as an insert; returns its table row. Cells
    /// start as NULL and only the ones typed into are sent, so column defaults apply.
    pub fn add_row(&mut self) -> usize {
        let data_row = self.rs.rows.len();
        self.rs.rows.push(vec![Value::Null; self.rs.columns.len()]);
        self.added.insert(data_row);
        self.order.push(data_row);
        self.order.len() - 1
    }

    /// The first column worth typing into on a new row: not a primary key (those are
    /// usually generated), else the first one. A table column index (gutter is 0).
    pub fn first_input_column(&self) -> usize {
        (0..self.rs.columns.len())
            .find(|c| !self.col_meta(*c).1)
            .unwrap_or(0)
            + 1
    }

    /// Statements the Apply button will run: one per edited, added or deleted row (an
    /// added row that is deleted again is simply dropped).
    pub fn pending(&self) -> usize {
        let edited = self
            .edits
            .keys()
            .map(|(r, _)| *r)
            .filter(|r| !self.deleted.contains(r) && !self.added.contains(r))
            .collect::<BTreeSet<usize>>()
            .len();
        let added = self.added.difference(&self.deleted).count();
        let deleted = self.deleted.difference(&self.added).count();
        edited + added + deleted
    }

    pub fn reset_staged(&mut self) {
        self.edits.clear();
        self.editing = None;
        self.deleted.clear();
        // Added rows go away with the rest of the staged work.
        let base = self.base_rows;
        self.rs.rows.truncate(base);
        self.order.retain(|r| *r < base);
        self.added.clear();
    }

    /// The selected cell as (table row, table column; 0 is the gutter).
    pub fn selected(&self) -> Option<(usize, usize)> {
        self.focus
    }

    /// Stages the deletion of a displayed row, or takes it back if already staged.
    /// Returns `true` when the row is now staged for deletion.
    pub fn toggle_delete(&mut self, row: usize) -> bool {
        let Some(r) = self.data_index(row) else {
            return false;
        };
        if self.deleted.remove(&r) {
            false
        } else {
            self.deleted.insert(r);
            true
        }
    }

    /// Column names and values of a displayed row, for the row viewer.
    pub fn row_fields(&self, row: usize) -> Vec<(String, Value)> {
        let Some(r) = self.data_index(row) else {
            return Vec::new();
        };
        self.rs
            .columns
            .iter()
            .enumerate()
            .map(|(c, col)| {
                let v = self
                    .edits
                    .get(&(r, c))
                    .or_else(|| self.rs.rows.get(r).and_then(|x| x.get(c)))
                    .cloned()
                    .unwrap_or(Value::Null);
                (col.name.clone(), v)
            })
            .collect()
    }

    /// Index into `rs.rows` of a displayed row (after any client-side sort).
    pub fn data_index(&self, row: usize) -> Option<usize> {
        self.order.get(row).copied()
    }

    /// The typed field behind a cell, for document results.
    fn doc_field(&self, row: usize, col: usize) -> Option<&thaan_core::doc::Doc> {
        let docs = self.rs.documents.as_ref()?;
        let name = &self.rs.columns.get(col)?.name;
        docs.get(*self.order.get(row)?)?.get(name)
    }

    pub fn set_row_offset(&mut self, offset: u64) {
        self.row_offset = offset;
    }

    pub fn set_table_info(&mut self, info: &TableInfo) {
        self.meta = info
            .columns
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    (c.data_type.clone(), c.primary_key.is_some()),
                )
            })
            .collect();
    }

    pub fn set_server_sort(&mut self, f: ServerSort) {
        self.server_sort = Some(f);
    }

    fn value(&self, row: usize, col: usize) -> Option<&Value> {
        self.order
            .get(row)
            .and_then(|r| self.rs.rows.get(*r))
            .and_then(|r| r.get(col))
    }

    /// (type, is primary key) for a data column.
    fn col_meta(&self, col: usize) -> (Option<String>, bool) {
        let Some(c) = self.rs.columns.get(col) else {
            return (None, false);
        };
        match self.meta.get(&c.name) {
            Some((ty, pk)) => (Some(ty.clone()).filter(|t| !t.is_empty()), *pk),
            None => (c.type_name.clone().filter(|t| !t.is_empty()), false),
        }
    }

    fn gutter_chars(&self) -> usize {
        (self.row_offset + self.rs.rows.len() as u64)
            .max(1)
            .to_string()
            .len()
    }
}

fn display(v: &Value) -> String {
    let s = v.to_string();
    let s = s.replace('\n', "⏎");
    if s.chars().count() > 200 {
        format!("{}…", s.chars().take(199).collect::<String>())
    } else {
        s
    }
}

fn cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    use Value::*;
    match (a, b) {
        (Null, Null) => Equal,
        (Null, _) => Less,
        (_, Null) => Greater,
        (Int(x), Int(y)) => x.cmp(y),
        (Int(x), Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Equal),
        (Float(x), Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Equal),
        (Float(x), Float(y)) => x.partial_cmp(y).unwrap_or(Equal),
        (x, y) => x.to_string().cmp(&y.to_string()),
    }
}

/// Approximate width of one character at the grid's text size.
const CH: f32 = 7.5;

impl TableDelegate for ResultGrid {
    fn columns_count(&self, _: &App) -> usize {
        if self.rs.columns.is_empty() {
            0
        } else {
            self.rs.columns.len() + 1
        }
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rs.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        if col_ix == 0 {
            return Column::new("gutter", "")
                .width(px(self.gutter_chars().max(2) as f32 * CH + 20.))
                .paddings(Edges::all(px(0.)))
                .fixed_left()
                .resizable(false)
                .movable(false);
        }
        let col = col_ix - 1;
        let c = &self.rs.columns[col];
        let (ty, pk) = self.col_meta(col);
        // Width from the header (name with its key and sort marks, the type on the line
        // under it) and the first rows.
        let head = (c.name.chars().count() + if pk { 3 } else { 0 } + 3)
            .max(ty.as_ref().map_or(0, |t| t.chars().count() + 3));
        let sample = self
            .rs
            .rows
            .iter()
            .take(50)
            .filter_map(|r| r.get(col))
            .map(|v| display(v).chars().count());
        let chars = sample
            .chain(std::iter::once(head))
            .max()
            .unwrap_or(8)
            .clamp(4, 48);
        let column = Column::new(
            SharedString::from(format!("c{col}")),
            SharedString::from(c.name.clone()),
        )
        .width(px(chars as f32 * CH + 24.))
        .paddings(Edges::all(px(0.)))
        .sortable()
        .resizable(true);
        match self.sort {
            Some((ix, ColumnSort::Ascending)) if ix == col => column.ascending(),
            Some((ix, ColumnSort::Descending)) if ix == col => column.descending(),
            _ => column,
        }
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        if col_ix == 0 {
            return h_flex().into_any_element();
        }
        // The selected cell's column header lifts one surface step; the accent stays on
        // the cell's frame.
        let on = self.focus.is_some_and(|(_, c)| c == col_ix);
        let (fg, quiet) = (theme.foreground, theme.muted_foreground);
        let col = col_ix - 1;
        let name = self
            .rs
            .columns
            .get(col)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let (ty, pk) = self.col_meta(col);
        // Two lines: the name (after the key mark), then the declared type under it.
        let name_line = h_flex()
            .min_w_0()
            .gap_1p5()
            .items_center()
            .when(pk, |d| {
                d.child(
                    Icon::empty()
                        .path("icons/key-round.svg")
                        .size_3()
                        .flex_none()
                        .text_color(quiet),
                )
            })
            .child(
                div()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg)
                    .child(name),
            );
        v_flex()
            .size_full()
            .min_w_0()
            .px(px(CELL_PX))
            .justify_center()
            .when(on, |d| d.bg(theme.foreground.opacity(0.06)))
            .child(name_line)
            .when_some(ty, |d, ty| {
                d.child(
                    div()
                        .truncate()
                        .text_xs()
                        .line_height(px(16.))
                        .font_family(theme.mono_font_family.clone())
                        .text_color(quiet)
                        .child(ty.to_lowercase()),
                )
            })
            .into_any_element()
    }

    /// Double-clicking a column's resize handle fits it to the widest of its header and
    /// the cells on this page, measured in the fonts the grid draws with.
    fn fit_column(
        &mut self,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Option<Pixels> {
        const MAX_W: f32 = 640.;
        let theme = cx.theme();
        let ui = window.text_style().font();
        let mono = font(theme.mono_font_family.clone());
        let rem = window.rem_size();
        let sm = rems(0.875).to_pixels(rem);
        let xs = rems(0.75).to_pixels(rem);
        let measure = |window: &mut Window, text: String, font: Font, size: Pixels| -> f32 {
            if text.is_empty() {
                return 0.;
            }
            let run = TextRun {
                len: text.len(),
                font,
                color: theme.foreground,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(
                window
                    .text_system()
                    .shape_line(text.into(), size, &[run], None)
                    .width(),
            )
        };
        let pad = 2. * CELL_PX;
        if col_ix == 0 {
            let last = (self.row_offset + self.rs.rows.len() as u64).to_string();
            return Some(px(measure(window, last, mono, xs) + pad));
        }
        let col = col_ix - 1;
        let name = self.rs.columns.get(col)?.name.clone();
        let (ty, pk) = self.col_meta(col);
        let mut bold = ui.clone();
        bold.weight = FontWeight::MEDIUM;
        // Header: key mark, name, type, and the table's sort icon (12 px + 2 px a side).
        let mut head = measure(window, name, bold, sm) + 16.;
        if pk {
            head += 12. + 6.;
        }
        if let Some(ty) = ty {
            head += 6. + measure(window, ty.to_lowercase(), mono.clone(), xs);
        }
        let mut widest = head;
        for r in 0..self.rs.rows.len() {
            let data_row = self.order.get(r).copied().unwrap_or(r);
            let v = self
                .edits
                .get(&(data_row, col))
                .or_else(|| self.rs.rows.get(data_row).and_then(|row| row.get(col)));
            let (text, f) = match v {
                None | Some(Value::Null) => ("NULL".to_string(), mono.clone()),
                Some(v @ (Value::Int(_) | Value::Float(_))) => (display(v), mono.clone()),
                Some(v) => (display(v), ui.clone()),
            };
            widest = widest.max(measure(window, text, f, sm));
            if widest >= MAX_W {
                break;
            }
        }
        Some(px((widest + pad).min(MAX_W)))
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(col) = col_ix.checked_sub(1) else {
            return;
        };
        self.sort = (sort != ColumnSort::Default).then_some((col, sort));
        if let Some(f) = &self.server_sort {
            let name = self.rs.columns[col].name.clone();
            f(name, sort, window, cx);
            return;
        }
        let rows = &self.rs.rows;
        let mut order: Vec<usize> = (0..rows.len()).collect();
        match sort {
            ColumnSort::Ascending => order.sort_by(|a, b| cmp(&rows[*a][col], &rows[*b][col])),
            ColumnSort::Descending => order.sort_by(|a, b| cmp(&rows[*b][col], &rows[*a][col])),
            _ => {}
        }
        self.order = order;
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let inner = self.cell_inner(row_ix, col_ix, window, cx);
        // Clicking a row number selects the whole row, as in a spreadsheet.
        let row_selected = self.focus == Some((row_ix, 0));
        let row_wash = cx.theme().primary;
        // A row staged for deletion is painted on its cells in one solid red (the red
        // laid over the background), so the table's own row hover cannot show through
        // and change it.
        let deleted_bg = self
            .order
            .get(row_ix)
            .filter(|r| self.deleted.contains(r))
            .map(|_| deleted_row_bg(cx));
        if col_ix == 0 {
            let table = cx.entity().downgrade();
            return div()
                .id(("gutter", row_ix))
                .size_full()
                .flex()
                .items_center()
                .px(px(CELL_PX))
                .when(row_selected, |d| d.bg(row_wash.opacity(0.22)))
                .when_some(deleted_bg, |d, bg| d.bg(bg))
                .child(inner)
                .context_menu(move |menu, _, cx| match table.upgrade() {
                    Some(t) => row_menu(menu, &t, row_ix, cx),
                    None => menu,
                })
                .into_any_element();
        }
        // The selected cell: we draw its accent frame ourselves (the kit's square one is
        // turned off in theme.rs) so it covers the whole cell with 4px corners, plus a
        // round handle in the bottom-right corner. While the cell is being edited the
        // frame stays as the editor's only border: the handle goes and the cell lifts
        // one surface step.
        let editing = self.is_editing(row_ix, col_ix);
        let framed = self.focus == Some((row_ix, col_ix));
        let selected = framed && !editing;
        let accent = cx.theme().primary;
        let lifted = cx.theme().foreground.opacity(0.06);
        // The menu sits on the cell itself: with cell selection on, the table stops a
        // right-click at the cell, so a row-level menu would never open. It reads the
        // grid when it opens, so nothing is copied per frame.
        let table = cx.entity().downgrade();
        let tip_table = table.clone();
        div()
            .id(("cell", row_ix * 4096 + col_ix))
            .size_full()
            .flex()
            .items_center()
            .relative()
            .px(px(CELL_PX))
            .when(row_selected && !editing, |d| d.bg(row_wash.opacity(0.08)))
            .when_some(deleted_bg, |d, bg| d.bg(bg))
            .when(editing, |d| d.bg(lifted))
            .child(inner)
            .when(framed, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(4.))
                        .border(px(1.5))
                        .border_color(accent),
                )
            })
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(1.))
                        .bottom(px(1.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(accent),
                )
            })
            // The full value on hover, read only when the tooltip opens.
            .tooltip(move |window, cx| {
                let text = tip_table
                    .upgrade()
                    .map(|t| t.read(cx).delegate().full_text(row_ix, col_ix))
                    .unwrap_or_default();
                Tooltip::new(text).build(window, cx)
            })
            .context_menu(move |menu, window, cx| match table.upgrade() {
                Some(t) => cell_menu(menu, &t, row_ix, col_ix, window, cx),
                None => menu,
            })
            .into_any_element()
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        if col_ix == 0 {
            return (self.row_offset + row_ix as u64 + 1).to_string();
        }
        let edited = self
            .order
            .get(row_ix)
            .and_then(|r| self.edits.get(&(*r, col_ix - 1)));
        edited
            .or_else(|| self.value(row_ix, col_ix - 1))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
    /// Rows staged for deletion get a solid red fill until Apply or Reset, so they read
    /// at a glance across the whole row. The text keeps the normal ink to stay legible.
    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let deleted = self
            .order
            .get(row_ix)
            .is_some_and(|r| self.deleted.contains(r));
        let theme = cx.theme();
        let ink = theme.foreground;
        let bg = deleted_row_bg(cx);
        // The row of the selected cell lifts one surface step (solid, so the stripe
        // under it does not show through); the cell keeps its teal frame.
        let current = !deleted && self.focus.is_some_and(|(r, _)| r == row_ix);
        let lifted = theme.background.blend(theme.foreground.opacity(0.06));
        div()
            .id(("row", row_ix))
            .when(current, |d| d.bg(lifted))
            .when(deleted, |d| d.bg(bg).text_color(ink))
    }
}

/// The solid red of a row staged for deletion: the danger colour at 50% laid over the
/// background, so it looks the same whatever sits under it (hover, stripes).
fn deleted_row_bg(cx: &App) -> Hsla {
    let theme = cx.theme();
    theme.background.blend(theme.danger.opacity(0.5))
}

impl ResultGrid {
    /// One row as tab-separated text, the format Excel and Google Sheets paste into
    /// cells. Values with tabs, newlines or quotes are quoted by the exporter; NULL is an
    /// empty cell. Staged edits are included.
    pub fn row_tsv(&self, row_ix: usize, header: bool) -> Option<String> {
        let one = self.row_set(row_ix)?;
        let opts = thaan_core::export::ExportOptions {
            format: thaan_core::export::ExportFormat::Tsv,
            include_header: header,
            table_name: None,
            null_text: String::new(),
        };
        let text = thaan_core::export::export(&one, &opts, &thaan_core::dialect::Dialect::SQLITE);
        Some(text.trim_end_matches(['\r', '\n']).to_string())
    }

    /// One displayed row (staged edits included) as a one-row result, for exporting it.
    pub fn row_set(&self, row_ix: usize) -> Option<ResultSet> {
        let data_row = *self.order.get(row_ix)?;
        let mut row = self.rs.rows.get(data_row)?.clone();
        for (c, v) in row.iter_mut().enumerate() {
            if let Some(e) = self.edits.get(&(data_row, c)) {
                *v = e.clone();
            }
        }
        Some(ResultSet {
            columns: self.rs.columns.clone(),
            rows: vec![row],
            ..Default::default()
        })
    }

    /// Adds a copy of a displayed row at the end, staged as an insert: every value but
    /// the primary key (left for the database to generate) is sent. Returns its table row.
    pub fn clone_row(&mut self, row_ix: usize) -> Option<usize> {
        let source = self.row_set(row_ix)?.rows.into_iter().next()?;
        let new_row = self.add_row();
        let data_row = *self.order.get(new_row)?;
        for (c, v) in source.into_iter().enumerate() {
            if !self.col_meta(c).1 && !v.is_null() {
                self.edits.insert((data_row, c), v);
            }
        }
        Some(new_row)
    }

    /// What ⌘C copies: the whole row (TSV) when its row number is selected, else the
    /// selected cell's value.
    pub fn selection_text(&self) -> Option<String> {
        let (row_ix, col_ix) = self.focus?;
        if col_ix == 0 {
            return self.row_tsv(row_ix, false);
        }
        let data_row = *self.order.get(row_ix)?;
        let v = self
            .edits
            .get(&(data_row, col_ix - 1))
            .or_else(|| self.rs.rows.get(data_row).and_then(|r| r.get(col_ix - 1)))?;
        Some(if v.is_null() {
            String::new()
        } else {
            v.to_string()
        })
    }

    /// The untruncated value for the hover tooltip (capped, so a huge text or blob
    /// cannot build a giant tooltip).
    fn full_text(&self, row_ix: usize, col_ix: usize) -> String {
        const MAX: usize = 2000;
        let edited = self
            .order
            .get(row_ix)
            .and_then(|r| self.edits.get(&(*r, col_ix - 1)));
        let text = match edited.or_else(|| self.value(row_ix, col_ix - 1)) {
            None | Some(Value::Null) => return "NULL".into(),
            Some(v) => v.to_string(),
        };
        match text.char_indices().nth(MAX) {
            Some((at, _)) => format!("{}…", &text[..at]),
            None => text,
        }
    }

    /// What a cell shows (figures in mono, NULL quiet, staged edits tinted).
    fn cell_inner(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Div {
        // Figures are set in the mono face so digits line up down a column; NULL is
        // quiet text, not italic.
        let mono = cx.theme().mono_font_family.clone();
        let muted = cx.theme().muted_foreground;
        if col_ix == 0 {
            return div()
                .w_full()
                .text_right()
                .text_xs()
                .font_family(mono)
                .text_color(muted.opacity(0.8))
                // An added row has no number yet; "+" marks it until Apply.
                .child(match self.order.get(row_ix) {
                    Some(r) if self.added.contains(r) => "+".to_string(),
                    _ => (self.row_offset + row_ix as u64 + 1).to_string(),
                });
        }
        let data_row = self.order.get(row_ix).copied();
        if let Some((r, c, input)) = &self.editing {
            if Some(*r) == data_row && *c == col_ix - 1 {
                // Flush with the cell's own text: no box, border or inset of its own.
                let numeric = matches!(
                    self.value(row_ix, col_ix - 1),
                    Some(Value::Int(_) | Value::Float(_))
                );
                return div().w_full().child(
                    Input::new(input)
                        .small()
                        .appearance(false)
                        .px_0()
                        .py_0()
                        .when(numeric, |i| i.font_family(mono)),
                );
            }
        }
        if let Some(v) = data_row.and_then(|r| self.edits.get(&(r, col_ix - 1))) {
            // A staged edit: the new value on a tinted cell until Apply or Reset.
            let tint = cx.theme().warning.opacity(0.18);
            return div()
                .w_full()
                .px_1()
                .rounded(px(3.))
                .bg(tint)
                .font_family(mono)
                .child(if v.is_null() {
                    "NULL".to_string()
                } else {
                    display(v)
                });
        }
        // Document results: objects and arrays read as `{ n fields }` / `[ n items ]`.
        if let Some(d) = self
            .doc_field(row_ix, col_ix - 1)
            .filter(|d| d.is_container())
        {
            return div()
                .font_family(mono)
                .text_color(crate::docview::kind_color(d.kind(), cx.theme()))
                .child(d.summary());
        }
        match self.value(row_ix, col_ix - 1) {
            None | Some(Value::Null) => div()
                .font_family(mono)
                .text_color(muted.opacity(0.7))
                .child("NULL"),
            Some(v @ (Value::Int(_) | Value::Float(_))) => div()
                .w_full()
                .text_right()
                .font_family(mono)
                .child(display(v)),
            Some(v) => div().child(display(v)),
        }
    }
}

/// A menu row in the danger colour, for actions that remove something. The trash icon
/// goes through the menu's own icon column, so it lines up with the other items.
fn danger_item(label: &'static str, danger: Hsla) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().text_color(danger).child(label))
        .icon(Icon::empty().path("icons/trash.svg").text_color(danger))
}

/// Keycap for a menu row: the item shows the key bound to `action` (its own click
/// handler still runs).
fn keyed(item: PopupMenuItem, action: impl gpui_kit::Action) -> PopupMenuItem {
    item.action(Box::new(action))
}

/// The row actions both menus share: copy the row in the formats people paste
/// elsewhere, clone it, stage or undo its delete, and see all its fields.
fn row_items(
    mut menu: PopupMenu,
    table: &Entity<TableState<ResultGrid>>,
    row_ix: usize,
    cx: &mut App,
) -> PopupMenu {
    let g = table.read(cx).delegate();
    let Some(data_row) = g.order.get(row_ix).copied() else {
        return menu;
    };
    let names: Vec<String> = g.rs.columns.iter().map(|c| c.name.clone()).collect();
    let row = g
        .row_set(row_ix)
        .and_then(|r| r.rows.into_iter().next())
        .unwrap_or_default();
    let json = serde_json::Value::Object(
        names
            .iter()
            .cloned()
            .zip(row.iter().map(Value::to_json))
            .collect(),
    )
    .to_string();
    let (tsv, with_header) = (
        g.row_tsv(row_ix, false).unwrap_or_default(),
        g.row_tsv(row_ix, true).unwrap_or_default(),
    );
    let (editable, staged, actions) = (
        g.editable,
        g.deleted.contains(&data_row),
        g.on_action.clone(),
    );
    let documents = g.rs.documents.is_some();
    let icon = |name: &str| Icon::empty().path(SharedString::from(format!("icons/{name}.svg")));
    let copy = |label: &'static str, text: String| {
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
        })
    };
    let act = |a: GridAction| {
        let actions = actions.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(f) = &actions {
                f(a.clone(), window, cx)
            }
        }
    };
    menu = menu
        .item(copy("Copy row for spreadsheets", tsv).icon(icon("sheet")))
        .item(copy("Copy row with column names", with_header))
        .item(copy("Copy row as JSON", json).icon(icon("braces")));
    if actions.is_some() && !documents {
        menu = menu
            .item(
                PopupMenuItem::new("Copy row as Markdown").on_click(act(GridAction::CopyAs {
                    row: row_ix,
                    format: thaan_core::export::ExportFormat::Markdown,
                })),
            )
            .item(
                PopupMenuItem::new("Copy row as SQL INSERT")
                    .icon(icon("code"))
                    .on_click(act(GridAction::CopyAs {
                        row: row_ix,
                        format: thaan_core::export::ExportFormat::SqlInsert,
                    })),
            );
    }
    if editable && actions.is_some() {
        menu = menu.separator().item(keyed(
            PopupMenuItem::new("Clone row")
                .icon(icon("copy-plus"))
                .on_click(act(GridAction::CloneRow { row: row_ix })),
            crate::app::CloneRow,
        ));
        menu = if staged {
            menu.item(
                PopupMenuItem::new("Keep row")
                    .icon(icon("undo-2"))
                    .on_click(act(GridAction::DeleteRow { row: row_ix })),
            )
        } else {
            menu.item(keyed(
                danger_item("Delete row", cx.theme().danger)
                    .on_click(act(GridAction::DeleteRow { row: row_ix })),
                crate::app::DeleteRow,
            ))
        };
    }
    if actions.is_some() {
        menu = menu.separator().item(
            PopupMenuItem::new(if documents { "See document" } else { "See row" })
                .icon(icon("eye"))
                .on_click(act(GridAction::Details { row: row_ix })),
        );
    }
    menu
}

/// The right-click menu of a row number.
fn row_menu(
    menu: PopupMenu,
    table: &Entity<TableState<ResultGrid>>,
    row_ix: usize,
    cx: &mut App,
) -> PopupMenu {
    row_items(menu, table, row_ix, cx)
}

/// The right-click menu of one cell: set NULL, quick filters on its value, copy it,
/// edit or paste into it, then the row actions (`row_items`). Built when it opens.
fn cell_menu(
    menu: PopupMenu,
    table: &Entity<TableState<ResultGrid>>,
    row_ix: usize,
    col_ix: usize,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    use thaan_core::table::FilterOp;
    let g = table.read(cx).delegate();
    let col = col_ix - 1;
    let Some(data_row) = g.order.get(row_ix).copied() else {
        return menu;
    };
    let column =
        g.rs.columns
            .get(col)
            .map(|c| c.name.clone())
            .unwrap_or_default();
    let value = g
        .edits
        .get(&(data_row, col))
        .cloned()
        .or_else(|| g.rs.rows.get(data_row).and_then(|r| r.get(col)).cloned())
        .unwrap_or_default();
    let (editable, actions) = (g.editable, g.on_action.clone());
    let icon = |name: &str| Icon::empty().path(SharedString::from(format!("icons/{name}.svg")));
    let act = |a: GridAction| {
        let actions = actions.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(f) = &actions {
                f(a.clone(), window, cx)
            }
        }
    };
    let mut menu = menu;
    if editable {
        let t_null = table.downgrade();
        menu = menu.item(
            PopupMenuItem::new("Set to NULL")
                .icon(icon("circle-slash"))
                .disabled(value.is_null())
                .on_click(move |_, _, cx| {
                    t_null
                        .update(cx, |t, cx| {
                            t.delegate_mut().edits.insert((data_row, col), Value::Null);
                            t.refresh(cx);
                            cx.notify();
                        })
                        .ok();
                }),
        );
    }
    // Quick filters on this cell's value (table tabs only).
    if actions.is_some() {
        let mut shown = value.to_string();
        if shown.chars().count() > 24 {
            shown = format!("{}…", shown.chars().take(23).collect::<String>());
        }
        let (f_actions, f_column) = (actions.clone(), column.clone());
        let filter = move |label: String, op: FilterOp, v: Value| {
            let (actions, column) = (f_actions.clone(), f_column.clone());
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                if let Some(f) = &actions {
                    f(
                        GridAction::Filter {
                            column: column.clone(),
                            op,
                            value: v.clone(),
                        },
                        window,
                        cx,
                    )
                }
            })
        };
        let quick: Vec<(String, FilterOp, Value)> = if value.is_null() {
            vec![
                (format!("{column} is NULL"), FilterOp::IsNull, Value::Null),
                (
                    format!("{column} is not NULL"),
                    FilterOp::IsNotNull,
                    Value::Null,
                ),
            ]
        } else {
            let mut v = vec![
                (format!("{column} = {shown}"), FilterOp::Eq, value.clone()),
                (format!("{column} ≠ {shown}"), FilterOp::Ne, value.clone()),
            ];
            if matches!(value, Value::Text(_)) {
                v.push((
                    format!("{column} contains {shown}"),
                    FilterOp::Like,
                    Value::Text(format!("%{value}%")),
                ));
            } else {
                v.push((format!("{column} > {shown}"), FilterOp::Gt, value.clone()));
                v.push((format!("{column} < {shown}"), FilterOp::Lt, value.clone()));
            }
            v.push((format!("{column} is NULL"), FilterOp::IsNull, Value::Null));
            v
        };
        menu = menu.submenu_with_icon(
            Some(icon("list-filter")),
            "Quick filter",
            window,
            cx,
            move |mut m, _, _| {
                for (label, op, v) in &quick {
                    m = m.item(filter(label.clone(), *op, v.clone()));
                }
                m
            },
        );
    }
    let copy = |label: &'static str, text: String| {
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
        })
    };
    menu = menu
        .separator()
        .item(keyed(
            copy(
                "Copy value",
                if value.is_null() {
                    String::new()
                } else {
                    value.to_string()
                },
            )
            .icon(icon("copy")),
            crate::app::CopySelection,
        ))
        .item(copy("Copy column name", column.clone()));
    if editable && actions.is_some() {
        menu = menu
            .item(keyed(
                PopupMenuItem::new("Paste")
                    .icon(icon("clipboard-paste"))
                    .on_click(act(GridAction::Paste {
                        row: row_ix,
                        col: col_ix,
                    })),
                crate::app::PasteCell,
            ))
            .item(
                PopupMenuItem::new("Edit cell")
                    .icon(icon("pencil"))
                    .on_click(act(GridAction::Edit {
                        row: row_ix,
                        col: col_ix,
                    })),
            );
    }
    row_items(menu.separator(), table, row_ix, cx)
}

#[cfg(test)]
mod tsv_tests {
    // Not `super::*`: that brings GPUI's own `test` attribute into scope.
    use super::ResultGrid;
    use thaan_core::{Column, ResultSet, Value};

    fn grid() -> ResultGrid {
        ResultGrid::new(ResultSet {
            columns: vec![Column::new("id"), Column::new("note"), Column::new("gone")],
            rows: vec![
                vec![Value::Int(1), Value::Text("plain".into()), Value::Null],
                vec![
                    Value::Int(2),
                    Value::Text("tab\there, \"quoted\"\nnext line".into()),
                    Value::Text("x".into()),
                ],
            ],
            ..Default::default()
        })
    }

    #[test]
    fn row_pastes_into_spreadsheets() {
        let g = grid();
        // Tab between cells, NULL as an empty cell, no trailing newline.
        assert_eq!(g.row_tsv(0, false).as_deref(), Some("1\tplain\t"));
        // A cell with a tab, quote or newline is quoted, quotes doubled.
        assert_eq!(
            g.row_tsv(1, false).as_deref(),
            Some("2\t\"tab\there, \"\"quoted\"\"\nnext line\"\tx")
        );
        assert_eq!(
            g.row_tsv(0, true).as_deref(),
            Some("id\tnote\tgone\r\n1\tplain\t")
        );
        assert_eq!(g.row_tsv(9, false), None);
    }

    #[test]
    fn staged_edits_are_copied() {
        let mut g = grid();
        g.edits.insert((0, 1), Value::Text("edited".into()));
        assert_eq!(g.row_tsv(0, false).as_deref(), Some("1\tedited\t"));
    }
}
