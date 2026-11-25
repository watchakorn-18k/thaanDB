//! Insert / view / edit / delete documents in a MongoDB collection view
//! (docs/spec/mongodb-ui.md M9). A child module of `app`, so it can read the app's view
//! state.
//!
//! The toolbar is four icon buttons; each opens a dialog. Every write is an ordinary
//! shell command run through the engine (`insertOne`, `replaceOne`, `deleteMany`), so
//! history, read-only checks and errors behave exactly as in a query tab.

use super::*;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::WindowExt as _;
use thaan_core::doc::Doc;
use thaan_core::engine::SessionId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DocDialogKind {
    Insert,
    View,
    Edit,
    Delete,
}

/// What a delete applies to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeleteScope {
    Selected,
    Filter,
    All,
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

const INSERT_TEMPLATE: &str = "{\n  \n}";

/// Parses the editor text as a document, for Validate and Format.
#[cfg(feature = "mongodb")]
fn parse_doc(text: &str) -> Result<Doc, String> {
    thaan_driver_mongodb::shell::parse_document(text)
        .map(|d| thaan_driver_mongodb::to_doc(&thaan_driver_mongodb::bson::Bson::Document(d)))
}

#[cfg(not(feature = "mongodb"))]
fn parse_doc(_: &str) -> Result<Doc, String> {
    Err("Built without the MongoDB driver".into())
}

/// The dialog's state. Lives in its own entity because the dialog builder may run
/// again while the dialog is open.
pub(super) struct DocDialog {
    kind: DocDialogKind,
    engine: Arc<Engine>,
    session_id: SessionId,
    app: WeakEntity<ThaanApp>,
    tab_ix: usize,
    collection: String,
    /// `connection / collection`, shown above the editor.
    place: String,
    id: Option<Doc>,
    /// The collection view's filter text, for the "Current filter" delete scope.
    view_filter: String,
    editor: Entity<EditorState>,
    /// Validation result, error or note, and whether it is an error.
    note: Option<(String, bool)>,
    busy: bool,
    wrap: bool,
    editable: bool,
    read_only: bool,
    scope: DeleteScope,
    matches: Option<u64>,
    inserted: u64,
}

impl DocDialog {
    pub(super) fn title(&self) -> &'static str {
        match self.kind {
            DocDialogKind::Insert => "Insert document",
            DocDialogKind::View => "Document",
            DocDialogKind::Edit => "Edit document",
            DocDialogKind::Delete => "Delete documents",
        }
    }

    fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().trim().to_string()
    }

    fn set_text(&self, text: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.into();
        self.editor
            .update(cx, |e, cx| e.set_value(text, window, cx));
    }

    fn validate(&mut self, cx: &mut Context<Self>) -> bool {
        let res = parse_doc(&self.text(cx));
        self.note = Some(match &res {
            Ok(Doc::Object(f)) => (
                format!("Valid · {}", plural(f.len() as u64, "field")),
                false,
            ),
            Ok(_) => ("Valid".into(), false),
            Err(e) => (e.clone(), true),
        });
        cx.notify();
        res.is_ok()
    }

    fn format(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match parse_doc(&self.text(cx)) {
            Ok(d) => {
                self.set_text(d.to_pretty(), window, cx);
                self.note = None;
            }
            Err(e) => self.note = Some((e, true)),
        }
        cx.notify();
    }

    fn set_wrap(&mut self, wrap: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.wrap = wrap;
        self.editor
            .update(cx, |e, cx| e.set_soft_wrap(wrap, window, cx));
        cx.notify();
    }

    fn request(&self, sql: String) -> QueryRequest {
        QueryRequest {
            session_id: self.session_id,
            sql,
            scope: RunScope::All,
            params: BTreeMap::new(),
        }
    }

    /// Loads the whole document for View / Edit (the grid may show a projection, and
    /// saving a partial document with replaceOne would drop the hidden fields).
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = &self.id else { return };
        self.busy = true;
        let req = self.request(format!(
            "{}.findOne({{ _id: {} }})",
            collection_expr(&self.collection),
            id.to_pretty()
        ));
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let doc = res
                    .ok()
                    .and_then(|r| r.into_iter().next()?.result?.documents?.into_iter().next());
                match doc {
                    Some(doc) => {
                        this.editable = round_trips(&doc);
                        this.set_text(doc.to_pretty(), window, cx);
                    }
                    None => this.note = Some(("The document no longer exists".into(), true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete_query(&self) -> String {
        match (self.scope, &self.id) {
            (DeleteScope::Selected, Some(id)) => format!("{{ _id: {} }}", id.to_pretty()),
            (DeleteScope::Filter, _) if !self.view_filter.trim().is_empty() => {
                self.view_filter.clone()
            }
            _ => "{}".into(),
        }
    }

    fn set_scope(&mut self, scope: DeleteScope, window: &mut Window, cx: &mut Context<Self>) {
        self.scope = scope;
        self.set_text(self.delete_query(), window, cx);
        self.count(window, cx);
    }

    /// Counts what the delete query matches, so the button can say how many go.
    fn count(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.matches = None;
        self.note = None;
        let req = self.request(format!(
            "{}.countDocuments({})",
            collection_expr(&self.collection),
            self.text(cx)
        ));
        let engine = self.engine.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Err(e) => this.note = Some((e.to_string(), true)),
                    Ok(r) => match r.into_iter().next() {
                        Some(StatementResult { error: Some(e), .. }) => this.note = Some((e, true)),
                        Some(StatementResult {
                            result: Some(rs), ..
                        }) => {
                            this.matches = match rs.rows.first().and_then(|r| r.first()) {
                                Some(thaan_core::Value::Int(n)) => Some((*n).max(0) as u64),
                                _ => None,
                            };
                        }
                        _ => {}
                    },
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Insert, update or delete. `again` keeps an insert dialog open for the next one.
    fn submit(&mut self, again: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let coll = collection_expr(&self.collection);
        let text = self.text(cx);
        let sql = match (self.kind, &self.id) {
            (DocDialogKind::Insert, _) => format!("{coll}.insertOne({text})"),
            (DocDialogKind::Edit, Some(id)) => {
                format!("{coll}.replaceOne({{ _id: {} }}, {text})", id.to_pretty())
            }
            (DocDialogKind::Delete, _) => format!("{coll}.deleteMany({text})"),
            _ => return,
        };
        self.busy = true;
        self.note = None;
        cx.notify();
        let req = self.request(sql);
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let first = match res {
                    Err(e) => {
                        this.note = Some((e.to_string(), true));
                        return cx.notify();
                    }
                    Ok(r) => r.into_iter().next(),
                };
                if let Some(e) = first.as_ref().and_then(|s| s.error.clone()) {
                    this.note = Some((e, true));
                    return cx.notify();
                }
                let affected = first
                    .and_then(|s| s.result)
                    .and_then(|r| r.rows_affected)
                    .unwrap_or(0);
                let msg = match this.kind {
                    DocDialogKind::Insert => "Document inserted".to_string(),
                    DocDialogKind::Edit => "Document updated".to_string(),
                    _ => format!("Deleted {}", plural(affected, "document")),
                };
                let tab_ix = this.tab_ix;
                this.app
                    .update(cx, |app, cx| {
                        app.set_status(msg, false, cx);
                        if app.active == tab_ix {
                            app.load_page(cx);
                        }
                    })
                    .ok();
                if again {
                    this.inserted += 1;
                    this.set_text(INSERT_TEMPLATE, window, cx);
                    this.note = Some((
                        format!(
                            "{} inserted · ready for the next",
                            plural(this.inserted, "document")
                        ),
                        false,
                    ));
                    cx.notify();
                } else {
                    window.close_dialog(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Render for DocDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let kind = self.kind;
        let writing = matches!(kind, DocDialogKind::Insert | DocDialogKind::Edit) && self.editable;
        let readonly_editor = (!writing && kind != DocDialogKind::Delete) || self.busy;

        // Where the document lives, then (for delete) what the delete applies to.
        let place = h_flex()
            .gap_3()
            .text_xs()
            .font_family(mono.clone())
            .text_color(theme.muted_foreground)
            .child(self.place.clone())
            .children(
                self.id
                    .as_ref()
                    .filter(|_| kind != DocDialogKind::Delete)
                    .map(|id| div().child(format!("_id {}", id.summary()))),
            );
        let scopes = (kind == DocDialogKind::Delete).then(|| {
            let current = self.scope;
            let has_id = self.id.is_some();
            let scope_btn =
                |id: &'static str, label: &'static str, s: DeleteScope, enabled: bool| {
                    Button::new(id)
                        .xsmall()
                        .ghost()
                        .label(label)
                        .selected(current == s)
                        .disabled(!enabled)
                };
            h_flex()
                .gap_1()
                .child(
                    scope_btn(
                        "scope-selected",
                        "Selected document",
                        DeleteScope::Selected,
                        has_id,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_scope(DeleteScope::Selected, window, cx)
                    })),
                )
                .child(
                    scope_btn("scope-filter", "Current filter", DeleteScope::Filter, true)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_scope(DeleteScope::Filter, window, cx)
                        })),
                )
                .child(
                    scope_btn("scope-all", "All documents", DeleteScope::All, true).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.set_scope(DeleteScope::All, window, cx)
                        }),
                    ),
                )
        });

        let editor = div()
            .h(px(360.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                Editor::new(&self.editor)
                    .readonly(readonly_editor)
                    .bordered(false)
                    .h_full(),
            );

        let note = self.note.clone().map(|(text, err)| {
            div()
                .text_xs()
                .font_family(mono.clone())
                .text_color(if err {
                    theme.danger
                } else {
                    theme.muted_foreground
                })
                .child(text)
        });
        let warn = (!self.editable).then(|| {
            div().text_xs().text_color(theme.warning).child(
                "Holds binary or special values the text form cannot save back, so it opens read-only",
            )
        });
        let delete_note = (kind == DocDialogKind::Delete).then(|| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Deleted documents cannot be restored")
        });

        // Footer: tools on the left, the decision on the right.
        let tools = h_flex()
            .gap_1()
            .when(writing || kind == DocDialogKind::Delete, |d| {
                d.child(
                    Button::new("doc-validate")
                        .small()
                        .ghost()
                        .icon(icon("check"))
                        .label("Validate")
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.validate(cx) && this.kind == DocDialogKind::Delete {
                                this.count(window, cx);
                            }
                        })),
                )
            })
            .when(writing, |d| {
                d.child(
                    Button::new("doc-format")
                        .small()
                        .ghost()
                        .icon(icon("braces"))
                        .label("Format")
                        .on_click(cx.listener(|this, _, window, cx| this.format(window, cx))),
                )
            })
            .child(
                div().pl_2().child(
                    Checkbox::new("doc-wrap")
                        .label("Wrap lines")
                        .checked(self.wrap)
                        .on_click(
                            cx.listener(|this, v: &bool, window, cx| this.set_wrap(*v, window, cx)),
                        ),
                ),
            );

        let esc = Keystroke::parse("escape")
            .ok()
            .map(|k| Kbd::new(k).outline());
        let decide = h_flex()
            .gap_2()
            .items_center()
            .children(esc)
            .child(
                Button::new("doc-cancel")
                    .small()
                    .ghost()
                    .label(if kind == DocDialogKind::View {
                        "Close"
                    } else {
                        "Cancel"
                    })
                    .on_click(cx.listener(|_, _, window, cx| window.close_dialog(cx))),
            )
            .when(
                kind == DocDialogKind::View && self.editable && !self.read_only,
                |d| {
                    d.child(
                        Button::new("doc-to-edit")
                            .small()
                            .icon(icon("pencil"))
                            .label("Edit")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.kind = DocDialogKind::Edit;
                                this.note = None;
                                cx.notify();
                            })),
                    )
                },
            )
            .when(kind == DocDialogKind::Insert, |d| {
                d.child(
                    Button::new("doc-insert-again")
                        .small()
                        .label("Insert and add another")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(true, window, cx))),
                )
            })
            .when(writing, |d| {
                d.child(
                    Button::new("doc-save")
                        .small()
                        .primary()
                        .label(match (kind, self.busy) {
                            (_, true) => "Saving…",
                            (DocDialogKind::Insert, _) => "Insert",
                            _ => "Update",
                        })
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.submit(false, window, cx)),
                        ),
                )
            })
            .when(kind == DocDialogKind::Delete, |d| {
                let label = match (self.busy, self.matches) {
                    (true, _) => "Deleting…".to_string(),
                    (_, Some(n)) => format!("Delete {}", plural(n, "document")),
                    (_, None) => "Delete".into(),
                };
                d.child(
                    Button::new("doc-delete")
                        .small()
                        .danger()
                        .label(label)
                        .disabled(
                            self.busy || self.read_only || self.matches.is_none_or(|n| n == 0),
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.submit(false, window, cx)),
                        ),
                )
            });

        v_flex()
            .gap_3()
            .child(place)
            .children(scopes)
            .child(editor)
            .children(warn)
            .children(note)
            .children(delete_note)
            .child(h_flex().pt_1().justify_between().child(tools).child(decide))
    }
}

impl ThaanApp {
    /// The document selected in the active collection tab (table row or tree item).
    pub(super) fn selected_doc(&self, cx: &App) -> Option<Doc> {
        self.selected_doc_ref(cx).cloned()
    }

    /// The selected document without copying it (for render-time checks).
    pub(super) fn selected_doc_ref(&self, cx: &App) -> Option<&Doc> {
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
        docs.get(ix)
    }

    pub(super) fn open_doc_dialog(
        &mut self,
        kind: DocDialogKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_doc_dialog_with(kind, None, window, cx)
    }

    /// `prefill` replaces the empty Insert template (Clone row puts a copy there).
    pub(super) fn open_doc_dialog_with(
        &mut self,
        kind: DocDialogKind,
        prefill: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        // Insert starts from nothing; the others act on the selected document.
        let id = (kind != DocDialogKind::Insert)
            .then(|| self.selected_doc(cx).and_then(|d| d.get("_id").cloned()))
            .flatten();
        if matches!(kind, DocDialogKind::View | DocDialogKind::Edit) && id.is_none() {
            return self.set_status("Select a document first", true, cx);
        }
        let collection = t.object.name.clone();
        let view_filter = t.filter.read(cx).value().trim().to_string();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("javascript")
                .line_number(true)
        });
        let app = cx.entity().downgrade();
        let dialog = cx.new(|_| DocDialog {
            kind,
            engine: self.engine.clone(),
            session_id: t.sid,
            app,
            tab_ix: self.active,
            place: format!("{} / {}", session.name, collection),
            collection,
            scope: if id.is_some() {
                DeleteScope::Selected
            } else {
                DeleteScope::Filter
            },
            id,
            view_filter,
            editor,
            note: None,
            busy: false,
            wrap: false,
            editable: true,
            read_only: session.read_only,
            matches: None,
            inserted: 0,
        });
        dialog.update(cx, |d, cx| match kind {
            DocDialogKind::Insert => {
                d.set_text(prefill.as_deref().unwrap_or(INSERT_TEMPLATE), window, cx)
            }
            DocDialogKind::Delete => {
                let scope = d.scope;
                d.set_scope(scope, window, cx);
            }
            _ => d.load(window, cx),
        });
        window.open_dialog(cx, move |modal, _, cx| {
            let title = dialog.read(cx).title();
            modal.title(title).w(px(760.)).child(dialog.clone())
        });
    }

    /// Asks before deleting the selected document, then runs `deleteOne({ _id })` off the
    /// UI thread and reloads the page.
    pub(super) fn confirm_delete_selected_doc(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        let Some(id) = self.selected_doc(cx).and_then(|d| d.get("_id").cloned()) else {
            return self.set_status("Select a document first", true, cx);
        };
        let (sid, tab_ix, collection) = (t.sid, self.active, t.object.name.clone());
        let this = cx.entity();
        let shown = id.summary();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let id = id.clone();
            let collection = collection.clone();
            alert
                .confirm()
                .title("Delete this document?")
                .description(SharedString::from(format!(
                    "_id {shown} will be removed from {collection}. This cannot be undone."
                )))
                .ok_text("Delete")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, _, cx| {
                    this.update(cx, |app, cx| {
                        app.delete_doc(sid, tab_ix, collection.clone(), id.clone(), cx)
                    });
                    true
                })
        });
    }

    fn delete_doc(
        &mut self,
        session_id: SessionId,
        tab_ix: usize,
        collection: String,
        id: Doc,
        cx: &mut Context<Self>,
    ) {
        let req = QueryRequest {
            session_id,
            sql: format!(
                "{}.deleteOne({{ _id: {} }})",
                collection_expr(&collection),
                id.to_pretty()
            ),
            scope: RunScope::All,
            params: BTreeMap::new(),
        };
        self.set_status("Deleting document…", false, cx);
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.run_query(&req) })
                .await;
            this.update(cx, |app, cx| {
                let first = match res {
                    Err(e) => return app.set_status(format!("Delete failed: {e}"), true, cx),
                    Ok(r) => r.into_iter().next(),
                };
                if let Some(e) = first.as_ref().and_then(|s| s.error.clone()) {
                    return app.set_status(format!("Delete failed: {e}"), true, cx);
                }
                let n = first
                    .and_then(|s| s.result)
                    .and_then(|r| r.rows_affected)
                    .unwrap_or(0);
                if n == 0 {
                    app.set_status("Nothing deleted: the document is already gone", true, cx);
                } else {
                    app.set_status("Document deleted", false, cx);
                }
                if app.active == tab_ix {
                    app.load_page(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Insert · View · Edit · Delete as icon buttons, with the selected `_id` on the right.
    pub(super) fn render_doc_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return None;
        };
        if !self.is_mongo() || t.view != TableView::Data {
            return None;
        }
        let theme = cx.theme().clone();
        let read_only = self.session.as_ref().is_some_and(|s| s.read_only);
        let id_text = self
            .selected_doc(cx)
            .and_then(|d| d.get("_id").map(|id| id.summary()));
        let has = id_text.is_some();
        let action = |id: &'static str, ic: &'static str, tip: &'static str, enabled: bool| {
            Button::new(id)
                .small()
                .ghost()
                .icon(icon(ic))
                .tooltip(if read_only && id != "doc-view" {
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
                    action("doc-add", "plus", "Insert document", !read_only).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_doc_dialog(DocDialogKind::Insert, window, cx)
                        },
                    )),
                )
                .child(
                    action("doc-view", "eye", "View document", has).on_click(cx.listener(
                        |this, _, window, cx| this.open_doc_dialog(DocDialogKind::View, window, cx),
                    )),
                )
                .child(
                    action("doc-edit", "pencil", "Edit document", has && !read_only).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_doc_dialog(DocDialogKind::Edit, window, cx)
                        }),
                    ),
                )
                .child(div().w(px(1.)).h(px(16.)).mx_1().bg(theme.border))
                .child(
                    action("doc-delete", "trash", "Delete documents", !read_only).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_doc_dialog(DocDialogKind::Delete, window, cx)
                        }),
                    ),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .font_family(theme.mono_font_family.clone())
                        .child(match id_text {
                            Some(id) => format!("_id {id}"),
                            None => "No document selected".into(),
                        }),
                )
                .into_any_element(),
        )
    }
}
