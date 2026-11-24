//! Result grid: renders a `ResultSet` with the virtualized table component.

use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use thaan_core::{ResultSet, Value};

pub struct ResultGrid {
    pub rs: ResultSet,
    /// Row order after a client-side sort (indices into `rs.rows`).
    order: Vec<usize>,
}

impl ResultGrid {
    pub fn new(rs: ResultSet) -> Self {
        let order = (0..rs.rows.len()).collect();
        Self { rs, order }
    }

    pub fn set(&mut self, rs: ResultSet) {
        self.order = (0..rs.rows.len()).collect();
        self.rs = rs;
    }

    fn value(&self, row: usize, col: usize) -> Option<&Value> {
        self.order
            .get(row)
            .and_then(|r| self.rs.rows.get(*r))
            .and_then(|r| r.get(col))
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

impl TableDelegate for ResultGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.rs.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rs.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let c = &self.rs.columns[col_ix];
        // Width from the header and the first rows, clamped.
        let sample = self
            .rs
            .rows
            .iter()
            .take(50)
            .filter_map(|r| r.get(col_ix))
            .map(|v| display(v).chars().count());
        let chars = sample
            .chain(std::iter::once(c.name.chars().count() + 2))
            .max()
            .unwrap_or(8)
            .clamp(4, 48);
        Column::new(
            SharedString::from(format!("c{col_ix}")),
            SharedString::from(c.name.clone()),
        )
        .width(px(chars as f32 * 7.5 + 24.))
        .sortable()
        .resizable(true)
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let rows = &self.rs.rows;
        let mut order: Vec<usize> = (0..rows.len()).collect();
        match sort {
            ColumnSort::Ascending => {
                order.sort_by(|a, b| cmp(&rows[*a][col_ix], &rows[*b][col_ix]))
            }
            ColumnSort::Descending => {
                order.sort_by(|a, b| cmp(&rows[*b][col_ix], &rows[*a][col_ix]))
            }
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
        match self.value(row_ix, col_ix) {
            None | Some(Value::Null) => div()
                .font_family(mono)
                .text_color(cx.theme().muted_foreground.opacity(0.7))
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
        self.value(row_ix, col_ix)
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}
