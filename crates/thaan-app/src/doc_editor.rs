//! Add / view / edit / delete documents in a MongoDB collection view
//! (docs/spec/mongodb-ui.md M9). A child module of `app`, so it can read the app's view
//! state.
//!
//! Every action is an ordinary shell command run through the engine
//! (`insertOne`, `replaceOne`, `deleteOne`, `findOne`), so history, read-only checks and
//! errors behave exactly as in a query tab.

use super::*;
use thaan_core::doc::Doc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DocEditMode {
    View,
    Edit,
    Add,
}

/// The side panel of a collection tab.
pub(super) struct DocEdit {
    mode: DocEditMode,
    /// `_id` of the document being viewed or edited.
    id: Option<Doc>,
    editor: Entity<EditorState>,
    error: Option<String>,
    busy: bool,
    /// `false` when the document holds values the text form cannot round-trip.
    editable: bool,
}

/// Values whose shell text cannot be parsed back without loss.
fn round_trips(d: &Doc) -> bool {
    match d {
        Doc::Binary(text) => text.starts_with("UUID("),
        Doc::Other { .. } => false,
        Doc::Double(f) => f.is_finite(),
        Doc::Array(a) => a.iter().all(round_trips),
        Doc::Object(f) => f.iter().all(|(_, v)| round_trips(v)),
        _ => true,
    }
}

fn collection_expr(name: &str) -> String {
    format!(
        "db.getCollection({})",
        Doc::String(name.to_string()).to_pretty()
    )
}

impl ThaanApp {
    /// The document selected in the active collection tab (table row or tree item).
    pub(super) fn selected_doc(&self, cx: &App) -> Option<Doc> {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return None;
        };
        let docs = t.page.as_ref()?.result.documents.as_ref()?;
        let ix = match t.docs.read(cx).view() {
            DocView::Table => {
                let grid = t.grid.read(cx);
                grid.delegate().data_index(
                    // A selected cell picks its row's document too.
                    grid.selected_row()
                        .or_else(|| grid.selected_cell().map(|(row, _)| row))?,
                )?
            }
            _ => t.docs.read(cx).selected()?,
        };
        docs.get(ix).cloned()
    }

    pub(super) fn open_doc_editor(
        &mut self,
        mode: DocEditMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = if mode == DocEditMode::Add {
            None
        } else {
            match self.selected_doc(cx).and_then(|d| d.get("_id").cloned()) {
                Some(id) => Some(id),
                None => {
                    return self.set_status(
                        "Select a document with an _id first (click a row)",
                        true,
                        cx,
                    )
                }
            }
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("javascript")
                .line_number(true)
        });
        if mode == DocEditMode::Add {
            editor.update(cx, |e, cx| e.set_value("{\n  \n}", window, cx));
        }
        let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) else {
            return;
        };
        t.pending_delete = false;
        t.doc_edit = Some(DocEdit {
            mode,
            id: id.clone(),
            editor: editor.clone(),
            error: None,
            busy: id.is_some(),
            editable: true,
        });
        cx.notify();
        let Some(id) = id else {
            editor.update(cx, |e, cx| e.focus(window, cx));
            return;
        };

        // Load the whole document: the grid may show a projection, and saving a partial
        // document with replaceOne would drop the hidden fields.
        let Some(session) = &self.session else { return };
        let req = QueryRequest {
            session_id: session.session_id,
            sql: format!(
                "{}.findOne({{ _id: {} }})",
                collection_expr(&t.object.name),
                id.to_pretty()
            ),
            scope: RunScope::All,
            params: BTreeMap::new(),
        };
        let engine = self.engine.clone();
        let ix = self.active;
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(Tab::Table(t)) = this.tabs.get_mut(ix) else {
                    return;
                };
                let Some(edit) = t.doc_edit.as_mut() else {
                    return;
                };
                edit.busy = false;
                let doc = res
                    .ok()
                    .and_then(|r| r.into_iter().next()?.result?.documents?.into_iter().next());
                match doc {
                    Some(doc) => {
                        edit.editable = round_trips(&doc);
                        let text = doc.to_pretty();
                        edit.editor
                            .update(cx, |e, cx| e.set_value(text, window, cx));
                    }
                    None => edit.error = Some("The document no longer exists".into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn close_doc_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) {
            t.doc_edit = None;
            cx.notify();
        }
    }

    pub(super) fn save_doc(&mut self, cx: &mut Context<Self>) {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        let Some(edit) = &t.doc_edit else { return };
        let text = edit.editor.read(cx).value().trim().to_string();
        let coll = collection_expr(&t.object.name);
        let (cmd, done) = match (edit.mode, &edit.id) {
            (DocEditMode::Add, _) => (format!("{coll}.insertOne({text})"), "Document added"),
            (DocEditMode::Edit, Some(id)) => (
                format!("{coll}.replaceOne({{ _id: {} }}, {text})", id.to_pretty()),
                "Document saved",
            ),
            _ => return,
        };
        self.run_doc_command(cmd, done, cx);
    }

    pub(super) fn ask_delete_doc(&mut self, cx: &mut Context<Self>) {
        if self
            .selected_doc(cx)
            .and_then(|d| d.get("_id").cloned())
            .is_none()
        {
            return self.set_status(
                "Select a document with an _id first (click a row)",
                true,
                cx,
            );
        }
        if let Some(Tab::Table(t)) = self.tabs.get_mut(self.active) {
            t.pending_delete = true;
            cx.notify();
        }
    }

    pub(super) fn delete_doc(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_doc(cx).and_then(|d| d.get("_id").cloned()) else {
            return;
        };
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        let cmd = format!(
            "{}.deleteOne({{ _id: {} }})",
            collection_expr(&t.object.name),
            id.to_pretty()
        );
        self.run_doc_command(cmd, "Document deleted", cx);
    }

    /// Runs one write and reloads the page on success. Errors stay in the panel (or the
    /// status bar for deletes) so the user can fix the text and retry.
    fn run_doc_command(&mut self, cmd: String, done: &'static str, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let req = QueryRequest {
            session_id: session.session_id,
            sql: cmd,
            scope: RunScope::All,
            params: BTreeMap::new(),
        };
        let ix = self.active;
        if let Some(Tab::Table(t)) = self.tabs.get_mut(ix) {
            if let Some(e) = t.doc_edit.as_mut() {
                e.busy = true;
                e.error = None;
            }
        }
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update(cx, |this, cx| {
                let error = match &res {
                    Err(e) => Some(e.to_string()),
                    Ok(r) => r.iter().find_map(|s| s.error.clone()),
                };
                let Some(Tab::Table(t)) = this.tabs.get_mut(ix) else {
                    return;
                };
                t.pending_delete = false;
                match error {
                    Some(e) => match t.doc_edit.as_mut() {
                        Some(edit) => {
                            edit.busy = false;
                            edit.error = Some(e);
                        }
                        None => this.set_status(e, true, cx),
                    },
                    None => {
                        t.doc_edit = None;
                        this.set_status(done, false, cx);
                        if this.active == ix {
                            this.load_page(cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Add · View · Edit · Delete, or the delete confirmation in their place.
    pub(super) fn render_doc_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return None;
        };
        if !self.is_mongo() || t.view != TableView::Data {
            return None;
        }
        let theme = cx.theme().clone();
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let selected = self.selected_doc(cx);
        let id_text = selected
            .as_ref()
            .and_then(|d| d.get("_id"))
            .map(|id| id.summary());
        let bar = h_flex()
            .h(px(36.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border);
        if t.pending_delete {
            return Some(
                bar.bg(theme.danger.opacity(0.08))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(theme.danger)
                            .child(format!(
                                "Delete the document with _id {}? This cannot be undone.",
                                id_text.unwrap_or_default()
                            )),
                    )
                    .child(
                        Button::new("doc-del-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                    t.pending_delete = false;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("doc-del-confirm")
                            .small()
                            .danger()
                            .label("Delete")
                            .on_click(cx.listener(|this, _, _, cx| this.delete_doc(cx))),
                    )
                    .into_any_element(),
            );
        }
        let has = selected.is_some();
        let action = |id: &'static str, ic: &'static str, label: &'static str| {
            Button::new(id).small().ghost().icon(icon(ic)).label(label)
        };
        Some(
            bar.child(
                action("doc-add", "plus", "Add")
                    .disabled(read_only)
                    .tooltip(if read_only {
                        "Read-only connection"
                    } else {
                        "Add a document"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_doc_editor(DocEditMode::Add, window, cx)
                    })),
            )
            .child(
                action("doc-view", "eye", "View")
                    .disabled(!has)
                    .tooltip("View the selected document")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_doc_editor(DocEditMode::View, window, cx)
                    })),
            )
            .child(
                action("doc-edit", "pencil", "Edit")
                    .disabled(!has || read_only)
                    .tooltip("Edit the selected document")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_doc_editor(DocEditMode::Edit, window, cx)
                    })),
            )
            .child(
                action("doc-delete", "trash", "Delete")
                    .disabled(!has || read_only)
                    .tooltip("Delete the selected document")
                    .on_click(cx.listener(|this, _, _, cx| this.ask_delete_doc(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .font_family(theme.mono_font_family.clone())
                    .child(match id_text {
                        Some(id) => format!("_id {id}"),
                        None => "Click a row to select a document".into(),
                    }),
            )
            .into_any_element(),
        )
    }

    /// The side panel for viewing, editing or adding a document.
    pub(super) fn render_doc_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return None;
        };
        let edit = t.doc_edit.as_ref()?;
        let theme = cx.theme().clone();
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let writing = edit.mode != DocEditMode::View && edit.editable;
        let title = match edit.mode {
            DocEditMode::Add => "New document".to_string(),
            DocEditMode::View => "Document".to_string(),
            DocEditMode::Edit => "Edit document".to_string(),
        };
        let footer = h_flex()
            .h(px(44.))
            .px_3()
            .gap_2()
            .justify_end()
            .border_t_1()
            .border_color(theme.border)
            .when(
                edit.mode == DocEditMode::View && !read_only && edit.editable,
                |d| {
                    d.child(
                        Button::new("doc-panel-edit")
                            .small()
                            .icon(icon("pencil"))
                            .label("Edit")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                    if let Some(e) = t.doc_edit.as_mut() {
                                        e.mode = DocEditMode::Edit;
                                    }
                                }
                                cx.notify();
                            })),
                    )
                },
            )
            .child(
                Button::new("doc-panel-close")
                    .small()
                    .ghost()
                    .label(if writing { "Cancel" } else { "Close" })
                    .on_click(cx.listener(|this, _, _, cx| this.close_doc_editor(cx))),
            )
            .when(writing, |d| {
                d.child(
                    Button::new("doc-panel-save")
                        .small()
                        .primary()
                        .label(if edit.busy {
                            "Saving…"
                        } else if edit.mode == DocEditMode::Add {
                            "Insert"
                        } else {
                            "Save"
                        })
                        .disabled(edit.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.save_doc(cx))),
                )
            });
        Some(
            v_flex()
                .w(px(440.))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .child(
                    h_flex()
                        .h(px(40.))
                        .px_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(div().flex_1().text_sm().font_medium().child(title))
                        .children(edit.id.as_ref().map(|id| {
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .font_family(theme.mono_font_family.clone())
                                .child(id.summary())
                        }))
                        .child(
                            Button::new("doc-panel-x")
                                .ghost()
                                .xsmall()
                                .icon(icon("x"))
                                .on_click(cx.listener(|this, _, _, cx| this.close_doc_editor(cx))),
                        ),
                )
                .when(!edit.editable, |d| {
                    d.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(theme.warning)
                            .child("This document holds binary or special values that the text editor cannot save back without loss, so it opens read-only."),
                    )
                })
                .children(edit.error.clone().map(|e| {
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(theme.danger)
                        .font_family(theme.mono_font_family.clone())
                        .child(e)
                }))
                .child(
                    div().flex_1().overflow_hidden().child(
                        Editor::new(&edit.editor)
                            .readonly(!writing || edit.busy)
                            .bordered(false)
                            .h_full(),
                    ),
                )
                .child(footer)
                .into_any_element(),
        )
    }
}
