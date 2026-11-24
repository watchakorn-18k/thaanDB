//! Result grid: renders a `ResultSet` with the virtualized table component.
//!
//! Column 0 is a fixed gutter with row numbers (counted across pages); data column `i`
//! is table column `i + 1`. Headers show the column type and a key mark on primary keys
//! (docs/spec/workspace.md §4).

use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::component::{h_flex, ActiveTheme as _, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use thaan_core::schema::TableInfo;
use thaan_core::{ResultSet, Value};

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
}

impl ResultGrid {
    pub fn new(rs: ResultSet) -> Self {
        let order = (0..rs.rows.len()).collect();
        Self {
            rs,
            order,
            row_offset: 0,
            meta: HashMap::new(),
            sort: None,
            server_sort: None,
        }
    }

    /// Replaces the rows. A client-side sort is dropped; a server sort is kept, since the
    /// new rows already come back in that order.
    pub fn set(&mut self, rs: ResultSet) {
        self.order = (0..rs.rows.len()).collect();
        if self.server_sort.is_none() {
            self.sort = None;
        }
        self.rs = rs;
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
                .fixed_left()
                .resizable(false)
                .movable(false);
        }
        let col = col_ix - 1;
        let c = &self.rs.columns[col];
        let (ty, pk) = self.col_meta(col);
        // Width from the header (name, type, key and sort marks) and the first rows.
        let head = c.name.chars().count()
            + ty.as_ref().map_or(0, |t| t.chars().count() + 1)
            + if pk { 3 } else { 0 }
            + 3;
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
        let col = col_ix - 1;
        let name = self
            .rs
            .columns
            .get(col)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let (ty, pk) = self.col_meta(col);
        h_flex()
            .w_full()
            .min_w_0()
            .gap_1p5()
            .items_center()
            .when(pk, |d| {
                d.child(
                    Icon::empty()
                        .path("icons/key-round.svg")
                        .size_3()
                        .text_color(theme.muted_foreground),
                )
            })
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(name),
            )
            .when_some(ty, |d, ty| {
                d.child(
                    div()
                        .truncate()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(ty.to_lowercase()),
                )
            })
            .into_any_element()
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
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
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
                .child((self.row_offset + row_ix as u64 + 1).to_string());
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

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        if col_ix == 0 {
            return (self.row_offset + row_ix as u64 + 1).to_string();
        }
        self.value(row_ix, col_ix - 1)
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}
