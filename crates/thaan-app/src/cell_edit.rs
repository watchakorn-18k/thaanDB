//! Staged edits in a table tab: double-click a cell to edit it (relational tables),
//! right-click a row to stage its deletion (every database). Nothing is written until
//! Apply, which runs every staged change in one transaction (one by one on MongoDB);
//! Reset drops them (docs/spec/workspace.md, data editing). A child module of `app`.

use super::*;
use thaan_core::table::StagedChange;
use thaan_core::Value;

/// Reads typed text back into a value of the original cell's kind. An exact `NULL`
/// sets SQL NULL; anything that does not parse as the original kind stays text and the
/// database decides.
fn parse_cell(text: &str, original: &Value) -> Value {
    let t = text.trim();
    if t == "NULL" {
        return Value::Null;
    }
    match original {
        Value::Int(_) => t
            .parse()
            .map(Value::Int)
            .unwrap_or_else(|_| Value::Text(text.into())),
        Value::Float(_) => t
            .parse()
            .map(Value::Float)
            .unwrap_or_else(|_| Value::Text(text.into())),
        Value::Bool(_) => match t {
            "true" | "1" => Value::Bool(true),
            "false" | "0" => Value::Bool(false),
            _ => Value::Text(text.into()),
        },
        Value::Null if t.is_empty() => Value::Null,
        _ => Value::Text(text.into()),
    }
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl ThaanApp {
    fn table_tab(&self) -> Option<&TableTab> {
        match self.tabs.get(self.active) {
            Some(Tab::Table(t)) => Some(t),
            _ => None,
        }
    }

    /// Opens an input in the double-clicked cell (relational tables only; documents
    /// are edited in their dialog).
    /// Add row (status bar +, ⌘N): a new empty row at the end of the grid, its first
    /// typeable cell open for editing, inserted on Apply. On MongoDB it opens the
    /// Insert document dialog instead.
    pub(super) fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_home {
            return;
        }
        let Some(t) = self.table_tab() else { return };
        if self.session.as_ref().is_some_and(|s| s.read_only) {
            return self.set_status("Read-only connection: rows cannot be added", true, cx);
        }
        if self.is_mongo() {
            return self.open_doc_dialog(super::doc_editor::DocDialogKind::Insert, window, cx);
        }
        let grid = t.grid.clone();
        let (row, col) = grid.update(cx, |g, cx| {
            let d = g.delegate_mut();
            let row = d.add_row();
            let col = d.first_input_column();
            g.refresh(cx);
            g.scroll_to_row(row, cx);
            g.set_selected_cell(row, col, cx);
            (row, col)
        });
        self.begin_cell_edit(row, col, window, cx);
        cx.notify();
    }

    pub(super) fn begin_cell_edit(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_mongo() || col == 0 {
            return;
        }
        let Some(t) = self.table_tab() else { return };
        let grid = t.grid.clone();
        let (data_row, current) = {
            let g = grid.read(cx).delegate();
            if !g.editable {
                return;
            }
            let Some(r) = g.data_index(row) else { return };
            let c = col - 1;
            let original = g.rs.rows.get(r).and_then(|x| x.get(c)).cloned();
            let Some(original) = original else { return };
            (r, g.edits.get(&(r, c)).cloned().unwrap_or(original))
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("NULL"));
        input.update(cx, |s, cx| {
            s.set_value(cell_text(&current), window, cx);
            s.focus(window, cx);
        });
        // Enter or leaving the cell keeps the value; it is only staged, not written.
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_cell_edit(cx);
            }
        })
        .detach();
        grid.update(cx, |g, cx| {
            g.delegate_mut().editing = Some((data_row, col - 1, input));
            g.refresh(cx);
        });
        cx.notify();
    }

    fn commit_cell_edit(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.table_tab() else { return };
        let grid = t.grid.clone();
        grid.update(cx, |g, cx| {
            let d = g.delegate_mut();
            let Some((r, c, input)) = d.editing.take() else {
                return;
            };
            let text = input.read(cx).value().to_string();
            if let Some(original) = d.rs.rows.get(r).and_then(|x| x.get(c)).cloned() {
                let new = parse_cell(&text, &original);
                if new == original {
                    d.edits.remove(&(r, c));
                } else {
                    d.edits.insert((r, c), new);
                }
            }
            g.refresh(cx);
        });
        cx.notify();
    }

    fn reset_staged(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.table_tab() else { return };
        t.grid.update(cx, |g, cx| {
            g.delegate_mut().reset_staged();
            g.refresh(cx);
        });
        cx.notify();
    }

    /// The staged edits and deletes as engine changes, keyed by primary key (or `_id`;
    /// or every original value when the table has no key).
    fn staged_changes(&self, cx: &App) -> Vec<StagedChange> {
        let Some(t) = self.table_tab() else {
            return Vec::new();
        };
        let g = t.grid.read(cx).delegate();
        let columns: Vec<String> = g.rs.columns.iter().map(|c| c.name.clone()).collect();
        let pk: Vec<String> = if self.is_mongo() {
            vec!["_id".into()]
        } else {
            t.info
                .as_ref()
                .map(|i| i.primary_key().into_iter().map(String::from).collect())
                .unwrap_or_default()
        };
        let key_of = |row: &Vec<Value>| -> Vec<(String, Value)> {
            columns
                .iter()
                .zip(row)
                .filter(|(name, _)| pk.is_empty() || pk.contains(name))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect()
        };
        let mut changes = Vec::new();
        let mut rows: BTreeMap<usize, Vec<(String, Value)>> = BTreeMap::new();
        for ((r, c), v) in &g.edits {
            if !g.deleted.contains(r) && !g.added.contains(r) {
                if let Some(name) = columns.get(*c) {
                    rows.entry(*r).or_default().push((name.clone(), v.clone()));
                }
            }
        }
        for (r, set) in rows {
            if let Some(row) = g.rs.rows.get(r) {
                changes.push(StagedChange::Update {
                    key: key_of(row),
                    set,
                });
            }
        }
        // Added rows: only the typed cells, so the table's defaults fill the rest.
        for r in g.added.difference(&g.deleted) {
            let values = columns
                .iter()
                .enumerate()
                .filter_map(|(c, name)| g.edits.get(&(*r, c)).map(|v| (name.clone(), v.clone())))
                .collect();
            changes.push(StagedChange::Insert { values });
        }
        for r in g.deleted.difference(&g.added) {
            if let Some(row) = g.rs.rows.get(*r) {
                changes.push(StagedChange::Delete { key: key_of(row) });
            }
        }
        changes
    }

    fn apply_staged(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.active_sid() else { return };
        let Some(t) = self.table_tab() else { return };
        let sid = t.sid;
        let changes = self.staged_changes(cx);
        if changes.is_empty() {
            return;
        }
        let table = t.object.clone();
        let n = changes.len();
        let engine = self.engine.clone();
        self.set_status(
            format!("Applying {}…", plural(n as u64, "change")),
            false,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.apply_changes(sid, &table, &changes) })
                .await;
            this.update(cx, |this, cx| match res {
                // Another connection came to the front meanwhile: the write is done; its
                // page reloads when that tab is next paged.
                _ if this.active_sid() != Some(ws) => {}
                Ok(affected) => {
                    this.set_status(
                        format!(
                            "Applied {} · {} affected",
                            plural(n as u64, "change"),
                            plural(affected, "row")
                        ),
                        false,
                        cx,
                    );
                    this.load_page(cx);
                }
                // Nothing was written (one transaction); the changes stay staged.
                Err(e) => this.set_status(format!("Apply failed, nothing saved: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Reset and "N Apply" in the status bar while changes are staged.
    /// `ink` is set when the bar has the connection's colour; then both buttons use it.
    /// Staged edits in the status bar: a count, Discard, and Apply as the one solid
    /// button. `bar` is the coloured bar's (ink, background), if it has a colour.
    pub(super) fn render_staged_actions(
        &self,
        bar: Option<(Hsla, Hsla)>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = self.table_tab()?;
        let pending = t.grid.read(cx).delegate().pending();
        if pending == 0 {
            return None;
        }
        let theme = cx.theme().clone();
        let ink = bar.map(|(i, _)| i);
        Some(
            h_flex()
                .gap_2()
                .child(bar_divider(ink, &theme))
                .child(
                    div()
                        .pr_1()
                        .font_family(theme.mono_font_family.clone())
                        .when_some(ink, |d, i| d.text_color(i.opacity(0.75)))
                        .child(plural(pending as u64, "change")),
                )
                .when_some(bar, |d, (ink, bg)| {
                    d.child(
                        bar_pill("staged-reset", "Discard", None, false, ink, bg)
                            .on_click(cx.listener(|this, _, _, cx| this.reset_staged(cx))),
                    )
                    .child(
                        bar_pill("staged-apply", "Apply", Some("check"), true, ink, bg)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_staged(cx))),
                    )
                })
                .when(bar.is_none(), |d| {
                    d.child(
                        Button::new("staged-reset")
                            .outline()
                            .xsmall()
                            .label("Discard")
                            .tooltip("Drop the staged changes")
                            .on_click(cx.listener(|this, _, _, cx| this.reset_staged(cx))),
                    )
                    .child(
                        Button::new("staged-apply")
                            .primary()
                            .xsmall()
                            .icon(icon("check"))
                            .label("Apply")
                            .tooltip("Write every staged change in one transaction")
                            .on_click(cx.listener(|this, _, _, cx| this.apply_staged(cx))),
                    )
                })
                .into_any_element(),
        )
    }
}

impl ThaanApp {
    /// Add · View · Edit · Delete for the selected row of a SQL table, the same bar
    /// MongoDB collections have (`render_doc_bar`), with the selected row on the right.
    pub(super) fn render_row_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return None;
        };
        if self.is_mongo() || t.view != TableView::Data || t.command_result.is_some() {
            return None;
        }
        let theme = cx.theme().clone();
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let g = t.grid.read(cx).delegate();
        let selected = g.selected().filter(|(r, _)| g.data_index(*r).is_some());
        let row_text = selected.map(|(r, _)| {
            let data = g.data_index(r).unwrap_or(r);
            if g.added.contains(&data) {
                "New row".to_string()
            } else if g.deleted.contains(&data) {
                format!("Row {} · to delete", r + 1)
            } else {
                format!("Row {}", r + 1)
            }
        });
        let has = selected.is_some();
        let action = |id: &'static str, ic: &'static str, tip: &'static str, enabled: bool| {
            Button::new(id)
                .small()
                .ghost()
                .icon(icon(ic))
                .tooltip(if read_only && id != "row-view" {
                    "Read-only connection"
                } else {
                    tip
                })
                .disabled(!enabled)
        };
        Some(
            h_flex()
                .h(px(36.))
                .px_2()
                .gap_0p5()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    action("row-add", "plus", "Add row (⌘N)", !read_only)
                        .on_click(cx.listener(|this, _, window, cx| this.add_row(window, cx))),
                )
                .child(
                    action("row-view", "eye", "View row", has)
                        .on_click(cx.listener(|this, _, window, cx| this.view_row(window, cx))),
                )
                .child(
                    action("row-edit", "pencil", "Edit cell", has && !read_only).on_click(
                        cx.listener(|this, _, window, cx| {
                            let Some((row, col)) = this
                                .table_tab()
                                .and_then(|t| t.grid.read(cx).delegate().selected())
                            else {
                                return;
                            };
                            let col = if col == 0 {
                                this.table_tab()
                                    .map_or(1, |t| t.grid.read(cx).delegate().first_input_column())
                            } else {
                                col
                            };
                            this.begin_cell_edit(row, col, window, cx);
                        }),
                    ),
                )
                .child(div().w(px(1.)).h(px(16.)).mx_1().bg(theme.border))
                .child(
                    action(
                        "row-delete",
                        "trash",
                        "Delete row (staged until Apply)",
                        has && !read_only,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let Some(t) = this.table_tab() else { return };
                        let grid = t.grid.clone();
                        grid.update(cx, |g, cx| {
                            if let Some((row, _)) = g.delegate().selected() {
                                g.delegate_mut().toggle_delete(row);
                                g.refresh(cx);
                            }
                        });
                        cx.notify();
                    })),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .font_family(theme.mono_font_family.clone())
                        .child(row_text.unwrap_or_else(|| "No row selected".into())),
                )
                .into_any_element(),
        )
    }

    /// Shows the selected row as column / value pairs in a dialog (read-only).
    fn view_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.table_tab() else { return };
        let g = t.grid.read(cx).delegate();
        let Some((row, _)) = g.selected() else { return };
        let fields = g.row_fields(row);
        let title = format!("{} · row {}", t.object.name, row + 1);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme().clone();
            let mono = theme.mono_font_family.clone();
            let mut list = v_flex().gap_2();
            for (name, v) in &fields {
                list = list.child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .child(
                            div()
                                .w(px(160.))
                                .flex_none()
                                .truncate()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .font_family(mono.clone())
                                .when(v.is_null(), |d| d.text_color(theme.muted_foreground))
                                .child(if v.is_null() {
                                    "NULL".to_string()
                                } else {
                                    cell_text(v)
                                }),
                        ),
                );
            }
            dialog
                .w(px(560.))
                .title(SharedString::from(title.clone()))
                .child(
                    div()
                        .id("row-view-body")
                        .max_h(px(480.))
                        .overflow_y_scroll()
                        .child(list),
                )
        });
    }
}

impl ThaanApp {
    /// The selected cell of the open table tab's grid (table row, table column).
    pub(super) fn selected_cell(&self, cx: &App) -> Option<(usize, usize)> {
        if self.on_home {
            return None;
        }
        self.table_tab()?.grid.read(cx).delegate().selected()
    }

    /// The row actions of the grid menus and their shortcuts (⌘V, ⌘D, ⌫).
    pub(super) fn row_action(
        &mut self,
        a: crate::grid::GridAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::grid::GridAction as A;
        let Some(t) = self.table_tab() else { return };
        let grid = t.grid.clone();
        let editable = grid.read(cx).delegate().editable;
        match a {
            A::Paste { row, col } => {
                if !editable || col == 0 || self.is_mongo() {
                    return;
                }
                let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
                    return;
                };
                // One cell takes one value: the first line, first tab-separated field.
                let text = text
                    .lines()
                    .next()
                    .and_then(|l| l.split('\t').next())
                    .unwrap_or_default()
                    .to_string();
                grid.update(cx, |g, cx| {
                    let d = g.delegate_mut();
                    let Some(r) = d.data_index(row) else { return };
                    let original = d.rs.rows.get(r).and_then(|x| x.get(col - 1)).cloned();
                    if let Some(original) = original {
                        let v = parse_cell(&text, &original);
                        if v == original {
                            d.edits.remove(&(r, col - 1));
                        } else {
                            d.edits.insert((r, col - 1), v);
                        }
                    }
                    g.refresh(cx);
                });
                cx.notify();
            }
            A::CopyAs { row, format } => {
                let Some(one) = grid.read(cx).delegate().row_set(row) else {
                    return;
                };
                let driver = self.session.as_ref().map_or("", |s| s.driver.as_str());
                let d = thaan_core::dialect::Dialect::for_driver(driver);
                let opts = thaan_core::export::ExportOptions {
                    format,
                    include_header: true,
                    table_name: Some(t.object.name.clone()),
                    null_text: "NULL".into(),
                };
                let text = thaan_core::export::export(&one, &opts, &d);
                cx.write_to_clipboard(ClipboardItem::new_string(
                    text.trim_end_matches(['\r', '\n']).to_string(),
                ));
                self.set_status("Copied the row", false, cx);
            }
            A::CloneRow { row } => {
                if !editable {
                    return;
                }
                if self.is_mongo() {
                    // A prefilled Insert document dialog: the document without its _id.
                    let doc =
                        grid.read(cx).delegate().data_index(row).and_then(|r| {
                            t.page.as_ref()?.result.documents.as_ref()?.get(r).cloned()
                        });
                    if let Some(thaan_core::doc::Doc::Object(fields)) = doc {
                        let copy = thaan_core::doc::Doc::Object(
                            fields.into_iter().filter(|(k, _)| k != "_id").collect(),
                        );
                        self.open_doc_dialog_with(
                            super::doc_editor::DocDialogKind::Insert,
                            Some(copy.to_pretty()),
                            window,
                            cx,
                        );
                    }
                    return;
                }
                let new_row = grid.update(cx, |g, cx| {
                    let r = g.delegate_mut().clone_row(row);
                    if let Some(r) = r {
                        g.refresh(cx);
                        g.scroll_to_row(r, cx);
                        g.set_selected_row(r, cx);
                    }
                    r
                });
                if new_row.is_some() {
                    self.set_status("Row cloned; Apply to insert it", false, cx);
                }
            }
            A::DeleteRow { row } => {
                if !editable {
                    return;
                }
                grid.update(cx, |g, cx| {
                    g.delegate_mut().toggle_delete(row);
                    g.refresh(cx);
                });
                cx.notify();
            }
            A::Details { row } => {
                grid.update(cx, |g, cx| g.set_selected_row(row, cx));
                if self.is_mongo() {
                    self.open_doc_dialog(super::doc_editor::DocDialogKind::View, window, cx);
                } else {
                    self.view_row(window, cx);
                }
            }
            A::Edit { row, col } => self.begin_cell_edit(row, col, window, cx),
            A::Filter { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` attribute into scope.
    use super::parse_cell;
    use thaan_core::Value;

    #[test]
    fn parse_keeps_kinds() {
        assert_eq!(parse_cell("42", &Value::Int(1)), Value::Int(42));
        assert_eq!(parse_cell("4.5", &Value::Float(1.0)), Value::Float(4.5));
        assert_eq!(parse_cell("x", &Value::Int(1)), Value::Text("x".into()));
        assert_eq!(parse_cell("NULL", &Value::Text("a".into())), Value::Null);
        assert_eq!(parse_cell("", &Value::Null), Value::Null);
        assert_eq!(
            parse_cell("", &Value::Text("a".into())),
            Value::Text(String::new())
        );
        assert_eq!(parse_cell("false", &Value::Bool(true)), Value::Bool(false));
    }
}
