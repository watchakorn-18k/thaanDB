//! Structure view of a SQL table tab: Columns · Indexes · Relations · Triggers · DDL,
//! each a read-only list (docs/spec/workspace.md, "Structure"). MongoDB collections keep
//! their own structure view in `app.rs`. A child module of `app`.

use super::*;

/// Which list the Structure view shows.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum StructTab {
    #[default]
    Columns,
    Indexes,
    Relations,
    Triggers,
    Ddl,
}

/// One list column: its header and a width (`None` takes what is left).
#[derive(Clone, Copy)]
struct Col {
    title: &'static str,
    width: Option<f32>,
}

const fn col(title: &'static str, width: Option<f32>) -> Col {
    Col { title, width }
}

impl ThaanApp {
    /// Loads the active table's DDL off the UI thread, once (the DDL tab and the first
    /// switch to Structure ask for it). A late answer for a closed tab is dropped.
    pub(super) fn load_structure_ddl(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        if t.ddl.is_some() && !force {
            return;
        }
        let (sid, object) = (t.sid, t.object.clone());
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let target = object.clone();
            let ddl = cx
                .background_spawn(async move { engine.ddl(sid, &target) })
                .await;
            this.update(cx, |this, cx| {
                for tab in this.tabs.iter_mut() {
                    if let Tab::Table(t) = tab {
                        if t.sid == sid && t.object == object {
                            t.ddl = Some(ddl.as_ref().cloned().unwrap_or_else(|e| e.to_string()));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Structure view of a SQL table tab.
    pub(super) fn render_structure(&self, t: &TableTab, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let muted = theme.muted_foreground;
        let current = t.struct_tab;
        let info = t.info.as_ref();
        let count = |n: Option<usize>| n.map(|n| n.to_string());
        let writable = !self.session.as_ref().is_some_and(|s| s.read_only);
        // What + adds on this tab (triggers and DDL have none).
        let add_form = match current {
            StructTab::Columns => Some(FormKind::Column),
            StructTab::Indexes => Some(FormKind::Index),
            StructTab::Relations => Some(FormKind::Relation),
            _ => None,
        };

        let tab = |id: &'static str, label: &'static str, which: StructTab, n: Option<usize>| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(current == which)
                .child(h_flex().gap_1p5().child(label).when_some(count(n), |d, n| {
                    d.child(div().font_family(mono.clone()).text_color(muted).child(n))
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                        t.struct_tab = which;
                    }
                    if which == StructTab::Ddl {
                        this.load_structure_ddl(false, cx);
                    }
                    cx.notify();
                }))
        };
        let header = h_flex()
            .h(px(40.))
            .px_4()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(tab(
                "st-cols",
                "Columns",
                StructTab::Columns,
                info.map(|i| i.columns.len()),
            ))
            .child(tab(
                "st-idx",
                "Indexes",
                StructTab::Indexes,
                info.map(|i| i.indexes.len()),
            ))
            .child(tab(
                "st-rel",
                "Relations",
                StructTab::Relations,
                info.map(|i| i.foreign_keys.len()),
            ))
            .child(tab(
                "st-trg",
                "Triggers",
                StructTab::Triggers,
                info.map(|i| i.triggers.len()),
            ))
            .child(tab("st-ddl", "DDL", StructTab::Ddl, None))
            .child(div().flex_1())
            .when_some(add_form.filter(|_| writable), |d, kind| {
                d.child(
                    Button::new("st-add")
                        .xsmall()
                        .ghost()
                        .icon(icon("plus"))
                        .tooltip(kind.title())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_struct_form(kind, None, window, cx)
                        })),
                )
            })
            .child(
                Button::new("st-refresh")
                    .xsmall()
                    .ghost()
                    .icon(icon("refresh-cw"))
                    .tooltip("Reload structure")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let Some(Tab::Table(t)) = this.tabs.get(this.active) else {
                            return;
                        };
                        let (ws, sid, object) = (t.ws, t.sid, t.object.clone());
                        this.describe_in_background(ws, sid, object, window, cx);
                        this.load_structure_ddl(true, cx);
                        this.set_status("Reloading structure…", false, cx);
                    })),
            );

        // A list: a header row, then one bordered panel of hairline rows.
        let cell = |c: &Col| {
            let d = div().px_3().min_w_0().truncate();
            match c.width {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1(),
            }
        };
        let list = |cols: &[Col], rows: Vec<Vec<AnyElement>>, empty: &'static str| {
            let head = h_flex().h(px(28.)).text_xs().text_color(muted).children(
                cols.iter()
                    .map(|c| cell(c).font_medium().child(c.title).into_any_element()),
            );
            let body = if rows.is_empty() {
                div()
                    .px_3()
                    .py_4()
                    .text_sm()
                    .text_color(muted)
                    .child(empty)
                    .into_any_element()
            } else {
                v_flex()
                    .children(rows.into_iter().enumerate().map(|(i, cells)| {
                        h_flex()
                            .h(px(36.))
                            .text_sm()
                            .when(i > 0, |d| d.border_t_1().border_color(theme.border))
                            .hover(|s| s.bg(theme.list_hover))
                            .children(
                                cells
                                    .into_iter()
                                    .zip(cols)
                                    .map(|(content, c)| cell(c).child(content)),
                            )
                    }))
                    .into_any_element()
            };
            v_flex()
                .gap_1()
                .child(head)
                .child(
                    div()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.sidebar)
                        .overflow_hidden()
                        .child(body),
                )
                .into_any_element()
        };
        // Small cell pieces.
        let text = |s: String| div().truncate().child(s).into_any_element();
        let code = |s: String| {
            div()
                .truncate()
                .font_family(mono.clone())
                .child(s)
                .into_any_element()
        };
        let null = || {
            div()
                .font_family(mono.clone())
                .text_color(muted.opacity(0.7))
                .child("NULL")
                .into_any_element()
        };
        let opt_code = |v: Option<String>| v.map(code).unwrap_or_else(null);
        let tick = |on: bool| {
            div()
                .when(on, |d| {
                    d.child(icon("check").size_4().text_color(theme.foreground))
                })
                .into_any_element()
        };

        // The × at the end of a row: asks, then drops (writable connections only).
        // The buttons at the end of a row: edit (columns) and drop, on writable
        // connections. Both open a dialog first.
        let row_btns = |i: usize,
                        edit: Option<thaan_core::schema::ColumnInfo>,
                        tip: &'static str,
                        op: Option<StructureOp>| {
            h_flex()
                .gap_0p5()
                .justify_end()
                .when_some(edit.filter(|_| writable), |d, c| {
                    d.child(
                        Button::new(("st-edit", i))
                            .xsmall()
                            .ghost()
                            .icon(icon("pencil"))
                            .tooltip("Rename or change column")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_struct_form(FormKind::Column, Some(c.clone()), window, cx)
                            })),
                    )
                })
                .when_some(op.filter(|_| writable), |d, op| {
                    d.child(
                        Button::new(("st-drop", i))
                            .xsmall()
                            .ghost()
                            .icon(icon("x"))
                            .tooltip(tip)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_structure_drop(op.clone(), window, cx)
                            })),
                    )
                })
                .into_any_element()
        };
        let action_col = col("", Some(72.));

        let body: AnyElement = match (current, info) {
            (StructTab::Ddl, _) => {
                let ddl = t.ddl.clone();
                div()
                    .p_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.sidebar)
                    .text_sm()
                    .font_family(mono.clone())
                    .when(ddl.is_none(), |d| d.text_color(muted))
                    .child(ddl.unwrap_or_else(|| "Loading DDL…".into()))
                    .into_any_element()
            }
            (_, None) => div()
                .text_sm()
                .text_color(muted)
                .child("Loading structure…")
                .into_any_element(),
            (StructTab::Columns, Some(info)) => {
                let cols = [
                    col("Name", None),
                    col("Type", Some(180.)),
                    col("Nullable", Some(88.)),
                    col("Default", Some(200.)),
                    col("Comment", Some(200.)),
                    col("Primary", Some(80.)),
                    action_col,
                ];
                let rows = info
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        vec![
                            text(c.name.clone()),
                            code(c.data_type.clone()),
                            tick(c.nullable),
                            opt_code(c.default.clone()),
                            c.comment.clone().map(text).unwrap_or_else(null),
                            h_flex()
                                .gap_1()
                                .when_some(c.primary_key, |d, p| {
                                    // Position shown only for a key of several columns.
                                    let many = info.primary_key().len() > 1;
                                    d.child(icon("key-round").size_3p5().text_color(muted))
                                        .when(many, |d| {
                                            d.child(
                                                div()
                                                    .text_xs()
                                                    .font_family(mono.clone())
                                                    .text_color(muted)
                                                    .child(p.to_string()),
                                            )
                                        })
                                })
                                .into_any_element(),
                            row_btns(
                                i,
                                Some(c.clone()),
                                "Drop column",
                                Some(StructureOp::DropColumn {
                                    name: c.name.clone(),
                                }),
                            ),
                        ]
                    })
                    .collect();
                list(&cols, rows, "No columns")
            }
            (StructTab::Indexes, Some(info)) => {
                let cols = [
                    col("Name", None),
                    col("Unique", Some(80.)),
                    col("Primary", Some(80.)),
                    col("Columns", Some(320.)),
                    action_col,
                ];
                let rows = info
                    .indexes
                    .iter()
                    .enumerate()
                    .map(|(i, ix)| {
                        vec![
                            text(ix.name.clone()),
                            tick(ix.unique),
                            tick(ix.primary),
                            code(ix.columns.join(", ")),
                            // The primary key's index goes with the key, not from here.
                            row_btns(
                                i,
                                None,
                                "Drop index",
                                (!ix.primary).then(|| StructureOp::DropIndex {
                                    name: ix.name.clone(),
                                }),
                            ),
                        ]
                    })
                    .collect();
                list(&cols, rows, "No indexes")
            }
            (StructTab::Relations, Some(info)) => {
                let cols = [
                    col("Name", None),
                    col("Columns", Some(160.)),
                    col("References", Some(220.)),
                    col("Ref columns", Some(140.)),
                    col("On update", Some(110.)),
                    col("On delete", Some(110.)),
                    action_col,
                ];
                let rows = info
                    .foreign_keys
                    .iter()
                    .enumerate()
                    .map(|(i, fk)| {
                        let target = match &fk.ref_schema {
                            Some(s) => format!("{s}.{}", fk.ref_table),
                            None => fk.ref_table.clone(),
                        };
                        vec![
                            fk.name.clone().map(text).unwrap_or_else(null),
                            code(fk.columns.join(", ")),
                            text(target),
                            code(fk.ref_columns.join(", ")),
                            fk.on_update.clone().map(text).unwrap_or_else(null),
                            fk.on_delete.clone().map(text).unwrap_or_else(null),
                            row_btns(
                                i,
                                None,
                                "Drop relation",
                                fk.name
                                    .clone()
                                    .map(|name| StructureOp::DropForeignKey { name }),
                            ),
                        ]
                    })
                    .collect();
                list(&cols, rows, "No relations")
            }
            (StructTab::Triggers, Some(info)) => {
                let cols = [
                    col("Name", Some(200.)),
                    col("Timing", Some(100.)),
                    col("Event", Some(140.)),
                    col("Action", None),
                    col("Condition", Some(200.)),
                ];
                let rows = info
                    .triggers
                    .iter()
                    .map(|tr| {
                        vec![
                            text(tr.name.clone()),
                            text(tr.timing.clone()),
                            text(tr.event.clone()),
                            // One line in the list; the full body is in the DDL tab.
                            code(tr.action.split_whitespace().collect::<Vec<_>>().join(" ")),
                            opt_code(tr.condition.clone()),
                        ]
                    })
                    .collect();
                list(&cols, rows, "No triggers")
            }
        };

        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("structure-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_6()
                    .child(body),
            )
            .into_any_element()
    }
}

use thaan_core::structure_ops::{ColumnDef, StructureOp, FK_ACTIONS};

/// What the + button of a Structure tab adds.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FormKind {
    Column,
    Index,
    Relation,
}

impl FormKind {
    fn title(self) -> &'static str {
        match self {
            FormKind::Column => "Add column",
            FormKind::Index => "Create index",
            FormKind::Relation => "Add relation",
        }
    }
}

/// First item of the ON UPDATE / ON DELETE pickers: leave the clause out.
const DB_DEFAULT: &str = "Database default";

/// The Add column / Create index / Add relation form. Its SQL preview is rebuilt from
/// the fields on every change (pure string building, no I/O); Apply runs it in the
/// background and reloads the structure.
pub(super) struct StructForm {
    kind: FormKind,
    engine: Arc<Engine>,
    sid: thaan_core::engine::SessionId,
    table: ObjectRef,
    app: WeakEntity<ThaanApp>,
    name: Entity<InputState>,
    data_type: Entity<InputState>,
    default: Entity<InputState>,
    columns: Entity<InputState>,
    ref_table: Entity<InputState>,
    ref_columns: Entity<InputState>,
    on_update: Entity<SelectState<Vec<SharedString>>>,
    on_delete: Entity<SelectState<Vec<SharedString>>>,
    nullable: bool,
    unique: bool,
    /// Column names of the table, shown as a hint for the column fields.
    known_columns: String,
    preview: std::result::Result<Vec<String>, String>,
    busy: bool,
    error: Option<String>,
    /// The column being renamed or changed (Column forms opened by its pencil).
    editing: Option<thaan_core::schema::ColumnInfo>,
}

/// Comma-separated names typed into a field.
fn names(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

impl StructForm {
    fn title(&self) -> &'static str {
        if self.editing.is_some() {
            "Change column"
        } else {
            self.kind.title()
        }
    }

    fn op(&self, cx: &App) -> StructureOp {
        let v = |i: &Entity<InputState>| i.read(cx).value().trim().to_string();
        let act = |s: &Entity<SelectState<Vec<SharedString>>>| {
            s.read(cx)
                .selected_value()
                .map(|v| v.to_string())
                .filter(|v| v != DB_DEFAULT)
        };
        if let (FormKind::Column, Some(was)) = (self.kind, &self.editing) {
            let before = ColumnDef::of(was);
            return StructureOp::AlterColumn {
                name: was.name.clone(),
                rename_to: Some(v(&self.name)),
                to: Some(ColumnDef {
                    data_type: v(&self.data_type),
                    nullable: self.nullable,
                    default: Some(v(&self.default)).filter(|d| !d.is_empty()),
                    // Not edited here; kept so MySQL's MODIFY does not drop them.
                    comment: before.comment.clone(),
                    auto_increment: before.auto_increment,
                }),
                from: Some(before),
            };
        }
        match self.kind {
            FormKind::Column => StructureOp::AddColumn {
                name: v(&self.name),
                data_type: v(&self.data_type),
                nullable: self.nullable,
                default: Some(v(&self.default)).filter(|d| !d.is_empty()),
            },
            FormKind::Index => StructureOp::CreateIndex {
                name: v(&self.name),
                columns: names(&v(&self.columns)),
                unique: self.unique,
            },
            FormKind::Relation => {
                // `schema.table`, or a bare name in the edited table's schema.
                let target = v(&self.ref_table);
                let ref_table = match target.split_once('.') {
                    Some((s, t)) => ObjectRef {
                        schema: Some(s.to_string()),
                        name: t.to_string(),
                    },
                    None => ObjectRef {
                        schema: self.table.schema.clone(),
                        name: target,
                    },
                };
                StructureOp::AddForeignKey {
                    name: Some(v(&self.name)).filter(|n| !n.is_empty()),
                    columns: names(&v(&self.columns)),
                    ref_table,
                    ref_columns: names(&v(&self.ref_columns)),
                    on_update: act(&self.on_update),
                    on_delete: act(&self.on_delete),
                }
            }
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let op = self.op(cx);
        self.preview = self
            .engine
            .structure_op_statements(self.sid, &self.table, &op)
            .map_err(|e| e.to_string());
        self.error = None;
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.preview.is_err() {
            return;
        }
        let op = self.op(cx);
        let (engine, sid, table) = (self.engine.clone(), self.sid, self.table.clone());
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let verb = op.verb();
            let res = cx
                .background_spawn(async move { engine.run_structure_op(sid, &table, &op) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match res {
                    Err(e) => {
                        this.error = Some(e.to_string());
                        cx.notify();
                    }
                    Ok(()) => {
                        let table = this.table.clone();
                        this.app
                            .update(cx, |app, cx| {
                                app.structure_changed(sid, &table, verb, window, cx)
                            })
                            .ok();
                        window.close_dialog(cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for StructForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let muted = theme.muted_foreground;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(div().text_xs().text_color(muted).child(label))
                .child(Input::new(input).small())
        };
        let picker = |label: &'static str, s: &Entity<SelectState<Vec<SharedString>>>| {
            v_flex()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(div().text_xs().text_color(muted).child(label))
                .child(Select::new(s).small())
        };
        let hint = div()
            .text_xs()
            .text_color(muted)
            .font_family(mono.clone())
            .truncate()
            .child(format!("Columns: {}", self.known_columns));

        let fields = match self.kind {
            FormKind::Column => v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_3()
                        .child(field("Name", &self.name))
                        .child(field("Type", &self.data_type)),
                )
                .child(field("Default (SQL expression)", &self.default))
                .child(
                    Checkbox::new("sf-nullable")
                        .label("Nullable")
                        .checked(self.nullable)
                        .on_click(cx.listener(|this, v: &bool, _, cx| {
                            this.nullable = *v;
                            this.refresh(cx);
                        })),
                ),
            FormKind::Index => v_flex()
                .gap_3()
                .child(field("Name", &self.name))
                .child(field("Columns (comma-separated, in order)", &self.columns))
                .child(hint)
                .child(
                    Checkbox::new("sf-unique")
                        .label("Unique")
                        .checked(self.unique)
                        .on_click(cx.listener(|this, v: &bool, _, cx| {
                            this.unique = *v;
                            this.refresh(cx);
                        })),
                ),
            FormKind::Relation => v_flex()
                .gap_3()
                .child(field("Name (optional)", &self.name))
                .child(field("Columns", &self.columns))
                .child(hint)
                .child(
                    h_flex()
                        .gap_3()
                        .child(field("References table (schema.table)", &self.ref_table))
                        .child(field("Referenced columns", &self.ref_columns)),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .child(picker("On update", &self.on_update))
                        .child(picker("On delete", &self.on_delete)),
                ),
        };

        let preview = match &self.preview {
            Ok(sql) => div()
                .p_3()
                .rounded(px(8.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .text_sm()
                .font_family(mono.clone())
                .child(sql.join(";\n")),
            Err(e) => div().text_sm().text_color(muted).child(e.clone()),
        };
        let can_apply = self.preview.is_ok() && !self.busy;
        v_flex()
            .gap_4()
            .child(
                div()
                    .text_xs()
                    .font_family(mono.clone())
                    .text_color(muted)
                    .child(self.table.name.clone()),
            )
            .child(fields)
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("Will run"))
                    .child(preview),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| div().text_sm().text_color(theme.danger).child(e)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("sf-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sf-apply")
                            .small()
                            .primary()
                            .label(if self.busy {
                                "Applying…"
                            } else {
                                self.title()
                            })
                            .loading(self.busy)
                            .disabled(!can_apply)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

impl ThaanApp {
    /// After a structure change: reload the table's structure and DDL, and say so.
    pub(super) fn structure_changed(
        &mut self,
        sid: thaan_core::engine::SessionId,
        table: &ObjectRef,
        verb: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = self.tabs.iter().find_map(|t| match t {
            Tab::Table(t) if t.sid == sid && &t.object == table => Some(t.ws),
            _ => None,
        });
        if let Some(ws) = ws {
            self.describe_in_background(ws, sid, table.clone(), window, cx);
        }
        for tab in self.tabs.iter_mut() {
            if let Tab::Table(t) = tab {
                if t.sid == sid && &t.object == table {
                    t.ddl = None;
                }
            }
        }
        self.load_structure_ddl(true, cx);
        self.set_status(format!("{verb}: done"), false, cx);
    }

    /// Opens the Add column / Create index / Add relation form for the active table.
    pub(super) fn open_struct_form(
        &mut self,
        kind: FormKind,
        editing: Option<thaan_core::schema::ColumnInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        if self.session.as_ref().is_some_and(|s| s.read_only) {
            return self.set_status("Read-only connection: structure cannot change", true, cx);
        }
        let known_columns = t
            .info
            .as_ref()
            .map(|i| {
                i.columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let (sid, table) = (t.sid, t.object.clone());
        let input = |ph: &str, window: &mut Window, cx: &mut Context<Self>| {
            let ph = ph.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(ph))
        };
        let name = input(
            &match kind {
                FormKind::Column => "note".to_string(),
                FormKind::Index => format!("idx_{}_…", table.name),
                FormKind::Relation => "generated when empty".to_string(),
            },
            window,
            cx,
        );
        let data_type = input("text, integer, varchar(40)…", window, cx);
        let default = input("none", window, cx);
        let columns = input("shop_id, status", window, cx);
        let ref_table = input("schema.table", window, cx);
        let ref_columns = input("id", window, cx);
        let actions: Vec<SharedString> = std::iter::once(DB_DEFAULT)
            .chain(FK_ACTIONS)
            .map(SharedString::from)
            .collect();
        let on_update =
            cx.new(|cx| SelectState::new(actions.clone(), Some(IndexPath::new(0)), window, cx));
        let on_delete = cx.new(|cx| SelectState::new(actions, Some(IndexPath::new(0)), window, cx));
        let app = cx.entity().downgrade();
        let form = cx.new(|_| StructForm {
            kind,
            engine: self.engine.clone(),
            sid,
            table,
            app,
            name: name.clone(),
            data_type: data_type.clone(),
            default: default.clone(),
            columns: columns.clone(),
            ref_table: ref_table.clone(),
            ref_columns: ref_columns.clone(),
            on_update: on_update.clone(),
            on_delete: on_delete.clone(),
            nullable: true,
            unique: false,
            known_columns,
            preview: Err("Fill in the fields to see the SQL".into()),
            busy: false,
            error: None,
            editing: editing.clone(),
        });
        for i in [
            &name,
            &data_type,
            &default,
            &columns,
            &ref_table,
            &ref_columns,
        ] {
            let f = form.clone();
            cx.subscribe(i, move |_, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    f.update(cx, |f, cx| f.refresh(cx));
                }
            })
            .detach();
        }
        for s in [&on_update, &on_delete] {
            let f = form.clone();
            cx.subscribe(s, move |_, _, _: &SelectEvent<Vec<SharedString>>, cx| {
                f.update(cx, |f, cx| f.refresh(cx));
            })
            .detach();
        }
        // Editing starts from the column as it is now; the preview says "nothing
        // changed" until a field differs.
        if let Some(c) = &editing {
            let set =
                |i: &Entity<InputState>, v: String, window: &mut Window, cx: &mut Context<Self>| {
                    i.update(cx, |i, cx| i.set_value(v, window, cx))
                };
            set(&name, c.name.clone(), window, cx);
            set(&data_type, c.data_type.clone(), window, cx);
            set(&default, c.default.clone().unwrap_or_default(), window, cx);
            form.update(cx, |f, cx| {
                f.nullable = c.nullable;
                f.refresh(cx);
            });
        }
        name.update(cx, |i, cx| i.focus(window, cx));
        let title = if editing.is_some() {
            "Change column"
        } else {
            kind.title()
        };
        window.open_dialog(cx, move |modal, _, _| {
            modal.title(title).w(px(560.)).child(form.clone())
        });
    }

    /// The × on a Structure row: shows the exact statement, asks, then runs it in the
    /// background. Dropping a column says its data goes with it.
    pub(super) fn confirm_structure_drop(
        &mut self,
        op: StructureOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        if self.session.as_ref().is_some_and(|s| s.read_only) {
            return self.set_status("Read-only connection: structure cannot change", true, cx);
        }
        let (sid, table) = (t.sid, t.object.clone());
        let sql = match self.engine.structure_op_statements(sid, &table, &op) {
            Ok(s) => s.join(";\n"),
            Err(e) => return self.set_status(e.to_string(), true, cx),
        };
        let what = match &op {
            StructureOp::DropColumn { name }
            | StructureOp::DropIndex { name }
            | StructureOp::DropForeignKey { name } => name.clone(),
            _ => String::new(),
        };
        let title = format!("{} “{what}”?", op.verb());
        let note = if op.destroys_data() {
            "The column's data is deleted with it. This cannot be undone."
        } else {
            "This cannot be undone."
        };
        let this = cx.entity();
        let engine = self.engine.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, engine, table, op) =
                (this.clone(), engine.clone(), table.clone(), op.clone());
            alert
                .confirm()
                .title(SharedString::from(title.clone()))
                .description(SharedString::from(format!("{sql}\n\n{note}")))
                .ok_text("Drop")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, window, cx| {
                    let (engine, table, op) = (engine.clone(), table.clone(), op.clone());
                    this.update(cx, |_, cx| {
                        cx.spawn_in(window, async move |app, cx| {
                            let verb = op.verb();
                            let t2 = table.clone();
                            let res = cx
                                .background_spawn(
                                    async move { engine.run_structure_op(sid, &t2, &op) },
                                )
                                .await;
                            app.update_in(cx, |app, window, cx| match res {
                                Ok(()) => app.structure_changed(sid, &table, verb, window, cx),
                                Err(e) => app.set_status(format!("{verb} failed: {e}"), true, cx),
                            })
                            .ok();
                        })
                        .detach();
                    });
                    true
                })
        });
    }
}
