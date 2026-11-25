//! Rename, duplicate, truncate and drop from a table or collection's right-click menu.
//! Each opens one dialog that asks first (the statements come from
//! `object_ops::statements`, so the button stays off for a missing or unchanged name);
//! drop and truncate say they cannot be undone and use the danger button. The work runs on a background thread through `Engine::run_object_op`, which
//! refuses read-only connections. A child module of `app`.

use super::*;
use thaan_core::engine::SessionId;
use thaan_core::object_ops::{self, ObjectOp};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum OpKind {
    Rename,
    Duplicate,
    Truncate,
    Drop,
    /// A new empty collection (MongoDB) in the target's database; its name is typed.
    Create,
}

/// The object an operation acts on.
#[derive(Clone)]
pub(super) struct OpTarget {
    /// The connection it belongs to.
    pub ws: SessionId,
    /// Set for an object of another database than the connection's own.
    pub database: Option<String>,
    pub object: ObjectRef,
    pub kind: ObjectKind,
    pub driver: String,
}

pub(super) struct OpDialog {
    target: OpTarget,
    op: OpKind,
    /// New name (rename, duplicate).
    name: Entity<InputState>,
    with_data: bool,
    running: bool,
    error: Option<String>,
}

impl OpDialog {
    fn op(&self, cx: &App) -> ObjectOp {
        let to = self.name.read(cx).value().trim().to_string();
        match self.op {
            OpKind::Rename => ObjectOp::Rename { to },
            OpKind::Duplicate => ObjectOp::Duplicate {
                to,
                with_data: self.with_data,
            },
            OpKind::Truncate => ObjectOp::Truncate,
            OpKind::Drop => ObjectOp::Drop,
            OpKind::Create => ObjectOp::Create,
        }
    }

    /// The object the statements act on: for Create, the typed name.
    fn object(&self, cx: &App) -> ObjectRef {
        let mut o = self.target.object.clone();
        if self.op == OpKind::Create {
            o.name = self.name.read(cx).value().trim().to_string();
        }
        o
    }
}

/// "table", "view", "collection"… for titles.
fn noun(t: &OpTarget) -> &'static str {
    match (t.driver.as_str(), t.kind) {
        ("mongodb", _) => "collection",
        ("redis", _) => "key",
        ("mqtt", _) => "topic",
        (_, ObjectKind::View) => "view",
        (_, ObjectKind::MaterializedView) => "materialized view",
        _ => "table",
    }
}

impl ThaanApp {
    pub(super) fn open_op_dialog(
        &mut self,
        target: OpTarget,
        op: OpKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let suggested = match op {
            OpKind::Rename => target.object.name.clone(),
            OpKind::Duplicate => format!("{}_copy", target.object.name),
            _ => String::new(),
        };
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |s, cx| s.set_value(suggested, window, cx));
        // The preview follows the name as it is typed.
        cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        if matches!(op, OpKind::Rename | OpKind::Duplicate | OpKind::Create) {
            name.update(cx, |s, cx| s.focus(window, cx));
        }
        self.op_dialog = Some(OpDialog {
            target,
            op,
            name,
            with_data: true,
            running: false,
            error: None,
        });
        let this = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            // Built from view state on every frame (the kit rebuilds the dialog).
            let parts = this.update(cx, |app, cx| app.op_dialog_parts(cx));
            let this2 = this.clone();
            let Some((title, body, footer)) = parts else {
                return dialog;
            };
            dialog
                .w(px(520.))
                .title(title)
                .child(body)
                .footer(footer)
                .on_close(move |_, _, cx| {
                    this2.update(cx, |app, _| app.op_dialog = None);
                })
        });
    }

    fn op_dialog_parts(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, AnyElement, AnyElement)> {
        let theme = cx.theme().clone();
        let d = self.op_dialog.as_ref()?;
        let t = &d.target;
        let what = noun(t);
        let name = &t.object.name;
        let title = match d.op {
            OpKind::Rename => format!("Rename {what} “{name}”"),
            OpKind::Duplicate => format!("Duplicate {what} “{name}”"),
            OpKind::Truncate => format!("Delete all rows of “{name}”?"),
            OpKind::Drop => format!("Drop {what} “{name}”?"),
            OpKind::Create => match &t.database {
                Some(db) => format!("New collection in {db}"),
                None => "New collection".to_string(),
            },
        };
        let op = d.op(cx);
        let preview = object_ops::statements(
            &thaan_core::dialect::Dialect::for_driver(&t.driver),
            t.kind,
            &d.object(cx),
            &op,
        );
        let warning = match d.op {
            OpKind::Truncate => {
                Some("Every row is deleted; the table stays. This can't be undone.")
            }
            OpKind::Drop => Some("The object and all its data are deleted. This can't be undone."),
            _ => None,
        };
        let body = v_flex()
            .gap_3()
            .when(
                matches!(d.op, OpKind::Rename | OpKind::Duplicate | OpKind::Create),
                |b| {
                    b.child(field(
                        if d.op == OpKind::Create {
                            "Name"
                        } else {
                            "New name"
                        },
                        Input::new(&d.name).into_any_element(),
                        &theme,
                    ))
                },
            )
            .when(d.op == OpKind::Duplicate, |b| {
                b.child(
                    Checkbox::new("op-with-data")
                        .label("Copy the rows too")
                        .checked(d.with_data)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            if let Some(d) = this.op_dialog.as_mut() {
                                d.with_data = *checked;
                            }
                            cx.notify();
                        })),
                )
            })
            .when_some(warning, |b, w| {
                b.child(div().text_sm().text_color(theme.danger).child(w))
            })
            // A name that is empty or unchanged just keeps the button off; only an
            // operation this database cannot do at all says why.
            .when_some(
                preview
                    .as_ref()
                    .err()
                    .filter(|e| matches!(e, thaan_core::Error::Unsupported(_)))
                    .map(|e| match e {
                        thaan_core::Error::Unsupported(m) => m.clone(),
                        other => other.to_string(),
                    }),
                |b, m| b.child(div().text_sm().text_color(theme.muted_foreground).child(m)),
            )
            .when_some(d.error.clone(), |b, e| {
                b.child(div().text_sm().text_color(theme.danger).child(e))
            })
            .into_any_element();
        let (label, busy) = match d.op {
            OpKind::Rename => ("Rename", "Renaming…"),
            OpKind::Duplicate => ("Duplicate", "Duplicating…"),
            OpKind::Truncate => ("Delete all rows", "Deleting…"),
            OpKind::Drop => ("Drop", "Dropping…"),
            OpKind::Create => ("Create", "Creating…"),
        };
        let danger = matches!(d.op, OpKind::Truncate | OpKind::Drop);
        let running = d.running;
        let ok = preview.is_ok();
        // Nothing destructive is preselected: the focus starts on Cancel or the name box.
        let run = Button::new("op-run")
            .label(if running { busy } else { label })
            .disabled(running || !ok)
            .on_click(cx.listener(|this, _, window, cx| this.run_op_dialog(window, cx)));
        let run = if danger { run.danger() } else { run.primary() };
        let footer = h_flex()
            .w_full()
            .justify_end()
            .gap_2()
            .child(
                Button::new("op-cancel")
                    .label("Cancel")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(run)
            .into_any_element();
        Some((div().child(title).into_any_element(), body, footer))
    }

    /// Runs the dialog's operation off the UI thread, opening a session for another
    /// database first when the object lives there.
    fn run_op_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.op_dialog.as_mut() else {
            return;
        };
        if d.running {
            return;
        }
        let op = d.op(cx);
        d.running = true;
        d.error = None;
        let mut t = d.target.clone();
        t.object = d.object(cx);
        let existing = match &t.database {
            None => Some(t.ws),
            Some(db) => self.tree_of(t.ws).and_then(|tree| tree.db_session(db)),
        };
        cx.notify();
        let engine = self.engine.clone();
        let (obj, kind, ws, database) = (t.object.clone(), t.kind, t.ws, t.database.clone());
        cx.spawn_in(window, async move |this, cx| {
            let op2 = op.clone();
            let res = cx
                .background_spawn(async move {
                    // A table of another database runs on that database's session.
                    let (sid, opened) = match (existing, &database) {
                        (Some(sid), _) => (sid, None),
                        (None, Some(db)) => {
                            let info = engine.open_database(ws, db)?;
                            (info.session_id, Some(info.session_id))
                        }
                        (None, None) => return Err(thaan_core::Error::UnknownSession(ws)),
                    };
                    let r = engine.run_object_op(sid, kind, &obj, &op2);
                    r.map(|_| (sid, opened))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match res {
                    Ok((sid, opened)) => {
                        if let (Some(id), Some(db)) = (opened, t.database.as_ref()) {
                            if let Some(tree) = this.tree_of(ws) {
                                tree.remember_db_session(db, id);
                            }
                        }
                        this.op_dialog = None;
                        window.close_dialog(cx);
                        this.after_object_op(&t, sid, &op, cx);
                    }
                    Err(e) => {
                        if let Some(d) = this.op_dialog.as_mut() {
                            d.running = false;
                            d.error = Some(e.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After an operation: close tabs that showed a dropped or renamed object, reload
    /// the open tab of a truncated one, refresh the tree and say what happened.
    fn after_object_op(
        &mut self,
        t: &OpTarget,
        sid: SessionId,
        op: &ObjectOp,
        cx: &mut Context<Self>,
    ) {
        let same =
            |tab: &Tab| matches!(tab, Tab::Table(tt) if tt.object == t.object && tt.sid == sid);
        if matches!(op, ObjectOp::Drop | ObjectOp::Rename { .. }) {
            while let Some(ix) = self.tabs.iter().position(same) {
                self.close_tab(ix, cx);
            }
        }
        if matches!(op, ObjectOp::Truncate) && self.tabs.get(self.active).is_some_and(same) {
            self.load_page(cx);
        }
        match &t.database {
            None => {
                self.load_databases(t.ws, cx);
                if self.active_sid() == Some(t.ws) {
                    self.refresh_schema(cx);
                }
            }
            Some(db) => self.load_database(t.ws, db.clone(), cx),
        }
        let name = &t.object.name;
        let msg = match op {
            ObjectOp::Rename { to } => format!("Renamed {name} to {to}"),
            ObjectOp::Duplicate { to, .. } => format!("Duplicated {name} as {to}"),
            ObjectOp::Truncate => format!("Deleted all rows of {name}"),
            ObjectOp::Drop => format!("Dropped {name}"),
            ObjectOp::Create => format!("Created collection {name}"),
        };
        self.set_status(msg, false, cx);
    }
}
