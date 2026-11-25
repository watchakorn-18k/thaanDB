//! The Data transfer dialog: copy tables or collections from one connection and
//! database to another (any engine to any engine), through `Engine::transfer`.
//!
//! Source and target each pick a saved connection and one of its databases; the dialog
//! opens its own sessions for them in the background (the workspace's connections are
//! left alone) and closes them with the dialog. The source's tables are listed with
//! checkboxes. The copy runs off the UI thread, reports progress after every batch and
//! can be cancelled. A child module of `app`.

use super::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use thaan_core::engine::SessionId;
use thaan_core::transfer::{ExistingTable, TransferContent, TransferProgress, TransferRequest};

type Picker = Entity<SelectState<Vec<SharedString>>>;

/// A saved connection in the picker: its colour dot, its database's logo and its name,
/// in the list and in the picked value alike.
#[derive(Clone)]
struct ConnItem {
    name: SharedString,
    driver: String,
    color: Option<Hsla>,
    ix: usize,
}

impl ConnItem {
    fn row(&self, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_2()
            .min_w_0()
            .child(match self.color {
                Some(c) => conn_dot(c).into_any_element(),
                None => div().flex_none().size(px(8.)).into_any_element(),
            })
            .child(db_icon(&self.driver).size_3p5().text_color(muted))
            .child(div().truncate().child(self.name.clone()))
            .into_any_element()
    }
}

impl gpui_kit::component::select::SelectItem for ConnItem {
    type Value = usize;
    fn title(&self) -> SharedString {
        self.name.clone()
    }
    fn display_title(&self) -> Option<AnyElement> {
        // No App here: draw the same row without the muted ink (it inherits the text).
        Some(
            h_flex()
                .gap_2()
                .min_w_0()
                .child(match self.color {
                    Some(c) => conn_dot(c).into_any_element(),
                    None => div().flex_none().size(px(8.)).into_any_element(),
                })
                .child(db_icon(&self.driver).size_3p5())
                .child(div().truncate().child(self.name.clone()))
                .into_any_element(),
        )
    }
    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.row(cx)
    }
    fn value(&self) -> &usize {
        &self.ix
    }
}

/// One side of the transfer: a connection, then a database on it.
struct Endpoint {
    conn: Entity<SelectState<Vec<ConnItem>>>,
    db: Picker,
    /// Index into the dialog's `connections`.
    conn_ix: Option<usize>,
    /// The session on the picked database, once open.
    sid: Option<SessionId>,
    /// The connection's first session; databases open beside it.
    base: Option<SessionId>,
    busy: Option<&'static str>,
    error: Option<String>,
}

pub(super) struct TransferDialog {
    engine: Arc<Engine>,
    connections: Vec<ConnectionConfig>,
    source: Endpoint,
    target: Endpoint,
    /// Tables and collections of the source database.
    objects: Vec<ObjectRef>,
    checked: BTreeSet<String>,
    search: Entity<InputState>,
    content: TransferContent,
    existing: Picker,
    running: bool,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Option<TransferProgress>>>,
    /// The last result or error, and whether it is an error.
    note: Option<(String, bool)>,
    /// Every session this dialog opened, closed with it.
    opened: Vec<SessionId>,
}

const EXISTING: [&str; 3] = ["Append rows", "Empty it first", "Drop and recreate"];

fn existing_from(label: &str) -> ExistingTable {
    match label {
        "Empty it first" => ExistingTable::Truncate,
        "Drop and recreate" => ExistingTable::Replace,
        _ => ExistingTable::Append,
    }
}

impl TransferDialog {
    /// A new dialog; `source` preselects a saved connection (by id) and database.
    pub(super) fn new(
        engine: Arc<Engine>,
        connections: Vec<ConnectionConfig>,
        source: Option<(String, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let names: Vec<ConnItem> = connections
            .iter()
            .enumerate()
            .map(|(ix, c)| ConnItem {
                name: SharedString::from(c.name.clone()),
                driver: c.driver.clone(),
                color: c.color.as_deref().and_then(parse_hex),
                ix,
            })
            .collect();
        let endpoint = |window: &mut Window, cx: &mut Context<Self>| Endpoint {
            conn: cx.new(|cx| SelectState::new(names.clone(), None, window, cx).searchable(true)),
            db: cx.new(|cx| {
                SelectState::new(Vec::<SharedString>::new(), None, window, cx).searchable(true)
            }),
            conn_ix: None,
            sid: None,
            base: None,
            busy: None,
            error: None,
        };
        let (src, dst) = (endpoint(window, cx), endpoint(window, cx));
        for (is_source, ep) in [(true, &src), (false, &dst)] {
            cx.subscribe_in(
                &ep.conn,
                window,
                move |this, _, ev: &SelectEvent<Vec<ConnItem>>, window, cx| {
                    if let SelectEvent::Confirm(Some(ix)) = ev {
                        this.pick_connection(is_source, *ix, None, window, cx);
                    }
                },
            )
            .detach();
            cx.subscribe_in(
                &ep.db,
                window,
                move |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(db)) = ev {
                        this.pick_database(is_source, db.to_string(), window, cx);
                    }
                },
            )
            .detach();
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tables"));
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let existing = cx.new(|cx| {
            SelectState::new(
                EXISTING
                    .iter()
                    .map(|s| SharedString::from(*s))
                    .collect::<Vec<_>>(),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let mut this = Self {
            engine,
            connections,
            source: src,
            target: dst,
            objects: Vec::new(),
            checked: BTreeSet::new(),
            search,
            content: TransferContent::StructureAndData,
            existing,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(None)),
            note: None,
            opened: Vec::new(),
        };
        if let Some((id, db)) = source {
            if let Some(ix) = this.connections.iter().position(|c| c.id == id) {
                this.source.conn.update(cx, |s, cx| {
                    s.set_selected_index(Some(IndexPath::new(ix)), window, cx)
                });
                this.pick_connection(true, ix, db, window, cx);
            }
        }
        this
    }

    fn side(&mut self, is_source: bool) -> &mut Endpoint {
        if is_source {
            &mut self.source
        } else {
            &mut self.target
        }
    }

    /// Connects to the picked connection in the background and lists its databases.
    fn pick_connection(
        &mut self,
        is_source: bool,
        ix: usize,
        database: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.connections.get(ix).cloned() else {
            return;
        };
        let ep = self.side(is_source);
        ep.conn_ix = Some(ix);
        ep.sid = None;
        ep.base = None;
        ep.error = None;
        ep.busy = Some("Connecting…");
        if is_source {
            self.objects.clear();
            self.checked.clear();
        }
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let info = engine.open(c)?;
                    let list = engine.list_databases(info.session_id)?;
                    Ok::<_, thaan_core::Error>((info.session_id, list))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match res {
                    Ok((sid, list)) => {
                        this.opened.push(sid);
                        let ep = this.side(is_source);
                        ep.base = Some(sid);
                        ep.sid = Some(sid);
                        ep.busy = None;
                        let items: Vec<SharedString> = list
                            .databases
                            .iter()
                            .cloned()
                            .map(SharedString::from)
                            .collect();
                        let current = list.current.clone();
                        let db = ep.db.clone();
                        db.update(cx, |s, cx| {
                            s.set_items(items, window, cx);
                            if let Some(c) = &current {
                                s.set_selected_value(&SharedString::from(c.clone()), window, cx);
                            }
                        });
                        match database.filter(|d| Some(d) != current.as_ref()) {
                            Some(d) => {
                                db.update(cx, |s, cx| {
                                    s.set_selected_value(&SharedString::from(d.clone()), window, cx)
                                });
                                this.pick_database(is_source, d, window, cx);
                            }
                            None if is_source => this.load_objects(window, cx),
                            None => {}
                        }
                    }
                    Err(e) => {
                        let ep = this.side(is_source);
                        ep.busy = None;
                        ep.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens a session on the picked database (beside the connection's first one).
    fn pick_database(
        &mut self,
        is_source: bool,
        db: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ep = self.side(is_source);
        let Some(base) = ep.base else { return };
        ep.busy = Some("Opening database…");
        ep.error = None;
        if is_source {
            self.objects.clear();
            self.checked.clear();
        }
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.open_database(base, &db) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(info) => {
                        this.opened.push(info.session_id);
                        let ep = this.side(is_source);
                        ep.sid = Some(info.session_id);
                        ep.busy = None;
                        if is_source {
                            this.load_objects(window, cx);
                        }
                    }
                    Err(e) => {
                        let ep = this.side(is_source);
                        ep.busy = None;
                        ep.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Lists the source database's tables (collections on MongoDB).
    fn load_objects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sid) = self.source.sid else { return };
        self.source.busy = Some("Loading tables…");
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.list_objects(sid) })
                .await;
            this.update(cx, |this, cx| {
                this.source.busy = None;
                match res {
                    Ok(list) => {
                        this.objects = list
                            .into_iter()
                            .filter(|o| o.kind == ObjectKind::Table)
                            .map(|o| o.object)
                            .collect();
                        this.objects.sort_by(|a, b| a.name.cmp(&b.name));
                    }
                    Err(e) => this.source.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Source and target trade places (connection, database and session).
    fn swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        std::mem::swap(&mut self.source, &mut self.target);
        self.objects.clear();
        self.checked.clear();
        self.load_objects(window, cx);
        cx.notify();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(source), Some(target)) = (self.source.sid, self.target.sid) else {
            return;
        };
        if self.running || self.checked.is_empty() {
            return;
        }
        let tables: Vec<ObjectRef> = self
            .objects
            .iter()
            .filter(|o| self.checked.contains(&o.name))
            .cloned()
            .collect();
        let existing = self
            .existing
            .read(cx)
            .selected_value()
            .map_or(ExistingTable::Append, |v| existing_from(v));
        let req = TransferRequest {
            source,
            target,
            tables,
            content: self.content,
            existing,
            batch_size: 500,
        };
        self.running = true;
        self.note = None;
        self.cancel.store(false, Ordering::Relaxed);
        *self.progress.lock().unwrap_or_else(|p| p.into_inner()) = None;
        cx.notify();
        let (engine, cancel, progress) = (
            self.engine.clone(),
            self.cancel.clone(),
            self.progress.clone(),
        );
        let done = Arc::new(AtomicBool::new(false));
        // Redraw the progress line a few times a second while the copy runs.
        let ticking = done.clone();
        cx.spawn_in(window, async move |this, cx| {
            while !ticking.load(Ordering::Relaxed) {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let r = engine.transfer(
                        &req,
                        &|p| *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone()),
                        &cancel,
                    );
                    done.store(true, Ordering::Relaxed);
                    r
                })
                .await;
            this.update(cx, |this, cx| {
                this.running = false;
                this.note = Some(match res {
                    Ok(r) if r.cancelled => (
                        format!(
                            "Cancelled after {} · {}",
                            plural(r.tables as u64, "table"),
                            plural(r.rows, "row")
                        ),
                        false,
                    ),
                    Ok(r) => (
                        format!(
                            "Copied {} · {}",
                            plural(r.tables as u64, "table"),
                            plural(r.rows, "row")
                        ),
                        false,
                    ),
                    Err(e) => (e.to_string(), true),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Closes the sessions this dialog opened (when the dialog closes).
    pub(super) fn close_sessions(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        let ids = std::mem::take(&mut self.opened);
        let engine = self.engine.clone();
        cx.background_spawn(async move {
            for id in ids {
                engine.close(id).ok();
            }
        })
        .detach();
    }

    fn endpoint_view(&self, is_source: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ep = if is_source {
            &self.source
        } else {
            &self.target
        };
        let label = |t: &str| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t.to_string())
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(if is_source { "From" } else { "To" }),
            )
            .child(label("Connection"))
            .child(
                Select::new(&ep.conn)
                    .small()
                    .placeholder("Choose a connection")
                    .search_placeholder("Search connections"),
            )
            .child(label("Database"))
            .child(
                Select::new(&ep.db)
                    .small()
                    .placeholder("Choose a database")
                    .disabled(ep.base.is_none()),
            )
            .child(
                div()
                    .h(px(16.))
                    .text_xs()
                    .when_some(ep.busy, |d, b| {
                        d.text_color(theme.muted_foreground).child(b)
                    })
                    .when_some(ep.error.clone(), |d, e| {
                        d.text_color(theme.danger).truncate().child(e)
                    }),
            )
            .into_any_element()
    }
}

impl Render for TransferDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let filter = self.search.read(cx).value().trim().to_lowercase();
        let shown: Vec<&ObjectRef> = self
            .objects
            .iter()
            .filter(|o| filter.is_empty() || o.name.to_lowercase().contains(&filter))
            .collect();
        let all_checked = !shown.is_empty() && shown.iter().all(|o| self.checked.contains(&o.name));
        let shown_names: Vec<String> = shown.iter().map(|o| o.name.clone()).collect();

        let mut list = v_flex().gap_0p5();
        for o in &shown {
            let name = o.name.clone();
            list = list.child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .rounded(px(6.))
                    .hover(|s| s.bg(theme.list_hover))
                    .child(
                        Checkbox::new(SharedString::from(format!("obj-{}", o.name)))
                            .label(o.name.clone())
                            .checked(self.checked.contains(&o.name))
                            .disabled(self.running)
                            .on_click(cx.listener(move |this, v: &bool, _, cx| {
                                if *v {
                                    this.checked.insert(name.clone());
                                } else {
                                    this.checked.remove(&name);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        let objects = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Tables"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(mono.clone())
                            .text_color(theme.muted_foreground)
                            .child(format!("{} of {}", self.checked.len(), self.objects.len())),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("tr-all")
                            .xsmall()
                            .ghost()
                            .label(if all_checked { "Clear" } else { "Select all" })
                            .disabled(shown.is_empty() || self.running)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for n in &shown_names {
                                    if all_checked {
                                        this.checked.remove(n);
                                    } else {
                                        this.checked.insert(n.clone());
                                    }
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Input::new(&self.search)
                    .small()
                    .prefix(icon("search").size_3p5()),
            )
            .child(
                div()
                    .id("tr-objects")
                    .h(px(184.))
                    .overflow_y_scroll()
                    .p_1()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .child(if self.objects.is_empty() {
                        div()
                            .p_3()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(if self.source.sid.is_none() {
                                "Choose where to copy from"
                            } else {
                                "No tables here"
                            })
                            .into_any_element()
                    } else {
                        list.into_any_element()
                    }),
            );

        let content = |id: &'static str,
                       label: &'static str,
                       hint: &'static str,
                       v: TransferContent,
                       cx: &mut Context<Self>| {
            h_flex()
                .gap_2()
                .child(
                    Radio::new(id)
                        .label(label)
                        .checked(self.content == v)
                        .disabled(self.running)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.content = v;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(hint),
                )
        };
        let options = v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Copy"),
            )
            .child(content(
                "tr-both",
                "Structure and rows",
                "",
                TransferContent::StructureAndData,
                cx,
            ))
            .child(content(
                "tr-structure",
                "Structure only",
                "creates the tables, no rows",
                TransferContent::StructureOnly,
                cx,
            ))
            .child(content(
                "tr-data",
                "Rows only",
                "into tables that already exist",
                TransferContent::DataOnly,
                cx,
            ))
            .child(
                h_flex()
                    .pt_1()
                    .gap_3()
                    .child(div().text_sm().child("When a table already exists"))
                    .child(
                        div()
                            .w(px(200.))
                            .flex_none()
                            .child(Select::new(&self.existing).small()),
                    ),
            );

        let progress = self.progress.lock().ok().and_then(|p| p.clone());
        let status: Option<(String, bool)> = if self.running {
            Some((
                match progress {
                    Some(p) => format!(
                        "Copying {} · {} of {} rows",
                        p.table, p.rows_done, p.rows_total
                    ),
                    None => "Starting…".into(),
                },
                false,
            ))
        } else {
            self.note.clone()
        };
        let ready = self.source.sid.is_some()
            && self.target.sid.is_some()
            && !self.checked.is_empty()
            && self.source.sid != self.target.sid;
        let footer = h_flex()
            .pt_3()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_family(mono)
                    .when_some(status, |d, (s, err)| {
                        d.text_color(if err {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(s)
                    }),
            )
            .child(
                Button::new("tr-cancel")
                    .small()
                    .label(if self.running { "Stop" } else { "Close" })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.running {
                            this.cancel.store(true, Ordering::Relaxed);
                        } else {
                            window.close_dialog(cx);
                        }
                    })),
            )
            .child(
                Button::new("tr-start")
                    .small()
                    .primary()
                    .icon(icon("arrow-left-right"))
                    .label(if self.running {
                        "Copying…"
                    } else {
                        "Start transfer"
                    })
                    .disabled(!ready || self.running)
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
            );

        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .child(self.endpoint_view(true, cx))
                    .child(
                        div().pt(px(44.)).child(
                            Button::new("tr-swap")
                                .ghost()
                                .small()
                                .icon(icon("arrow-left-right"))
                                .tooltip("Swap source and target")
                                .disabled(self.running)
                                .on_click(cx.listener(|this, _, window, cx| this.swap(window, cx))),
                        ),
                    )
                    .child(self.endpoint_view(false, cx)),
            )
            .child(objects)
            .child(options)
            .child(footer)
    }
}

impl ThaanApp {
    /// Opens Data transfer; `source` preselects a saved connection (by id) and database.
    pub(super) fn open_transfer_dialog(
        &mut self,
        source: Option<(String, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let engine = self.engine.clone();
        let connections = self.connections.clone();
        let dialog = cx.new(|cx| TransferDialog::new(engine, connections, source, window, cx));
        window.open_dialog(cx, move |modal, _, _| {
            let closing = dialog.clone();
            modal
                .title(
                    h_flex()
                        .gap_2()
                        .child(icon("arrow-left-right").size_4())
                        .child("Data transfer"),
                )
                .w(px(820.))
                .child(dialog.clone())
                .on_close(move |_, _, cx| {
                    closing.update(cx, |d, cx| d.close_sessions(cx));
                })
        });
    }
}
