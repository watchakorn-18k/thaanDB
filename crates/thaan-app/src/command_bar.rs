//! The command box at the top of a table or collection tab: a small editor that opens
//! with a ready-to-run command for that object (`db.getCollection("x").find({})` or
//! `SELECT * FROM "x" LIMIT 100`). Run (⌘↵) shows the result in the tab's grid; the
//! filter, paging or the close button return to the paged view. A child module of `app`.

use super::*;
use thaan_core::dialect::Dialect;

/// The command a tab opens with.
pub(super) fn starter_command(driver: &str, object: &ObjectRef) -> String {
    if driver == "mongodb" {
        let name = thaan_core::doc::Doc::String(object.name.clone()).to_pretty();
        return format!("db.getCollection({name}).find({{}})");
    }
    let name = thaan_core::cmdline::quote_arg(&object.name);
    match driver {
        "redis" => return format!("TYPE {name}"),
        "mqtt" => return format!("MESSAGES {name} 100"),
        _ => {}
    }
    let d = Dialect::for_driver(driver);
    format!(
        "SELECT * FROM {} {};",
        d.quote_object(object),
        d.paging_clause(100, 0)
    )
}

impl ThaanApp {
    /// Runs the active table tab's command and shows its last result in the grid.
    pub(super) fn run_table_command(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.active_sid() else { return };
        let ix = self.active;
        let Some(Tab::Table(t)) = self.tabs.get_mut(ix) else {
            return;
        };
        let sid = t.sid;
        if t.command_running {
            return;
        }
        let ed = t.command.read(cx);
        let sql = ed.value().to_string();
        let sel = ed.selected_range();
        let scope = if sel.is_empty() {
            RunScope::All
        } else {
            RunScope::Range {
                start: sel.start,
                end: sel.end,
            }
        };
        t.command_running = true;
        cx.notify();
        let engine = self.engine.clone();
        let req = QueryRequest {
            session_id: sid,
            sql,
            scope,
            params: BTreeMap::new(),
        };
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update(cx, |this, cx| {
                let Some(Tab::Table(t)) = this.tab_at(ws, ix) else {
                    return;
                };
                t.command_running = false;
                this.refresh_home_history();
                let Some(Tab::Table(t)) = this.tab_at(ws, ix) else {
                    return;
                };
                let last = match res {
                    Ok(results) => {
                        // The failing statement if any, else the last one with rows.
                        let pick = results
                            .iter()
                            .position(|r| r.error.is_some())
                            .or_else(|| {
                                results.iter().rposition(|r| {
                                    r.result.as_ref().is_some_and(|rs| !rs.columns.is_empty())
                                })
                            })
                            .unwrap_or(results.len().saturating_sub(1));
                        results.into_iter().nth(pick)
                    }
                    Err(e) => Some(StatementResult {
                        sql: String::new(),
                        result: None,
                        error: Some(e.to_string()),
                    }),
                };
                if let Some(rs) = last.as_ref().and_then(|r| r.result.clone()) {
                    let documents = rs.documents.clone().unwrap_or_default();
                    t.docs.update(cx, |p, cx| p.set_docs(documents, cx));
                    t.grid.update(cx, |g, cx| {
                        g.delegate_mut().set_row_offset(0);
                        g.delegate_mut().set(rs);
                        // A command's rows are not the table's; they cannot be edited.
                        g.delegate_mut().editable = false;
                        g.refresh(cx);
                    });
                }
                let msg = match &last {
                    Some(StatementResult { error: Some(e), .. }) => (e.clone(), true),
                    Some(StatementResult {
                        result: Some(rs), ..
                    }) if rs.columns.is_empty() => (
                        format!(
                            "{} affected · {} ms",
                            plural(rs.rows_affected.unwrap_or(0), "row"),
                            rs.elapsed_ms
                        ),
                        false,
                    ),
                    Some(StatementResult {
                        result: Some(rs), ..
                    }) => (
                        format!(
                            "Command · {} · {} ms",
                            plural(rs.rows.len() as u64, "row"),
                            rs.elapsed_ms
                        ),
                        false,
                    ),
                    _ => ("Nothing to run".into(), false),
                };
                t.command_result = last;
                // The footer already states rows and time; the status line only reports
                // errors and write counts.
                if msg.1 || msg.0.contains("affected") {
                    this.set_status(msg.0, msg.1, cx);
                } else {
                    this.status = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Back to the paged view (also what Apply and paging do).
    fn clear_table_command(&mut self, cx: &mut Context<Self>) {
        if let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) {
            t.command_result = None;
        }
        self.load_page(cx);
    }

    pub(super) fn render_command_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return None;
        };
        if t.view != TableView::Data || !t.command_open {
            return None;
        }
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let showing = t.command_result.is_some();
        let error = t.command_result.as_ref().and_then(|r| r.error.clone());
        // Cursor position for the status line.
        let (line, col) = {
            let ed = t.command.read(cx);
            let text = ed.value();
            let at = ed.cursor().min(text.len());
            let before = text.get(..at).unwrap_or("");
            let line = before.matches('\n').count() + 1;
            let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            (line, col)
        };
        let status = h_flex()
            .h(px(24.))
            .flex_none()
            .px_3()
            .gap_3()
            .text_xs()
            .font_family(mono.clone())
            .text_color(theme.muted_foreground)
            .child(format!("Ln {line}, Col {col}"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when_some(error.clone(), |d, e| d.text_color(theme.danger).child(e)),
            )
            .child(if t.command_running {
                "Running…"
            } else {
                "⌘↵ run"
            });
        Some(
            v_flex()
                .size_full()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .items_start()
                        .gap_2()
                        .px_2()
                        .pt_1p5()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.border)
                                .overflow_hidden()
                                .child(Editor::new(&t.command).bordered(false).h_full()),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    Button::new("cmd-run")
                                        .small()
                                        .primary()
                                        .icon(icon("play"))
                                        .label(if t.command_running {
                                            "Running…"
                                        } else {
                                            "Run"
                                        })
                                        .tooltip("Run the command, or the selection (⌘↵)")
                                        .disabled(t.command_running)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.run_table_command(cx)
                                            }),
                                        ),
                                )
                                .when(showing, |d| {
                                    d.child(
                                        Button::new("cmd-clear")
                                            .small()
                                            .ghost()
                                            .icon(icon("x"))
                                            .label("Clear")
                                            .tooltip("Back to the paged view")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.clear_table_command(cx)
                                            })),
                                    )
                                }),
                        ),
                )
                .child(status)
                .into_any_element(),
        )
    }
}
