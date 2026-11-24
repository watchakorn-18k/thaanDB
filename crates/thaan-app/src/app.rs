//! Main window. All work is dispatched to `thaan_core::Engine`; this file only holds view
//! state and layout (see AGENTS.md, principle 1 and 3).

use crate::grid::ResultGrid;
use crate::theme;
use gpui_kit::base::{Disableable as _, Selectable as _, StyledExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{
    h_flex, h_resizable, resizable_panel, v_flex, v_resizable, ActiveTheme as _, Icon, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use thaan_core::commands::builtin_commands;
use thaan_core::connection::ConnectionConfig;
use thaan_core::driver::{DriverInfo, ParamKind};
use thaan_core::engine::{Engine, QueryRequest, RunScope, SessionInfo, StatementResult, TablePage};
use thaan_core::export::{ExportFormat, ExportOptions};
use thaan_core::schema::{ObjectKind, ObjectRef, SchemaObject, TableInfo};
use thaan_core::store::{HistoryEntry, SavedQuery};
use thaan_core::table::{PageRequest, Sort};
use thaan_core::ResultSet;

actions!(
    thaan,
    [
        RunCurrent,
        RunAll,
        NewTab,
        CloseTab,
        TogglePalette,
        ToggleTheme,
        RefreshSchema,
        Disconnect
    ]
);

#[cfg(target_os = "macos")]
macro_rules! m {
    ($k:literal) => {
        concat!("cmd-", $k)
    };
}
#[cfg(not(target_os = "macos"))]
macro_rules! m {
    ($k:literal) => {
        concat!("ctrl-", $k)
    };
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(m!("enter"), RunCurrent, Some("ThaanApp")),
        KeyBinding::new(m!("shift-enter"), RunAll, Some("ThaanApp")),
        KeyBinding::new(m!("t"), NewTab, Some("ThaanApp")),
        KeyBinding::new(m!("w"), CloseTab, Some("ThaanApp")),
        KeyBinding::new(m!("k"), TogglePalette, Some("ThaanApp")),
        KeyBinding::new(m!("r"), RefreshSchema, Some("ThaanApp")),
    ]);
}

fn icon(name: &str) -> Icon {
    Icon::empty().path(SharedString::from(format!("icons/{name}.svg")))
}

#[derive(Clone, Copy, PartialEq)]
enum Rail {
    Objects,
    Saved,
    History,
}

#[derive(Clone, Copy, PartialEq)]
enum TableView {
    Data,
    Structure,
}

struct QueryTab {
    title: SharedString,
    saved_id: Option<String>,
    editor: Entity<EditorState>,
    results: Vec<StatementResult>,
    active_result: usize,
    grid: Entity<TableState<ResultGrid>>,
    running: bool,
}

struct TableTab {
    object: ObjectRef,
    info: Option<TableInfo>,
    ddl: Option<String>,
    filter: Entity<InputState>,
    page: Option<TablePage>,
    page_ix: u64,
    sort: Vec<Sort>,
    view: TableView,
    grid: Entity<TableState<ResultGrid>>,
    error: Option<String>,
    loading: bool,
}

enum Tab {
    Query(QueryTab),
    Table(Box<TableTab>),
}

impl Tab {
    fn title(&self) -> SharedString {
        match self {
            Tab::Query(q) => q.title.clone(),
            Tab::Table(t) => t.object.name.clone().into(),
        }
    }
}

pub struct ThaanApp {
    engine: Arc<Engine>,
    focus: FocusHandle,
    // connect screen
    drivers: Vec<DriverInfo>,
    driver: DriverInfo,
    conn_name: Entity<InputState>,
    conn_url: Entity<InputState>,
    conn_params: Vec<(String, ParamKind, Entity<InputState>)>,
    conn_read_only: bool,
    connections: Vec<ConnectionConfig>,
    // workspace
    session: Option<SessionInfo>,
    objects: Vec<SchemaObject>,
    tree_filter: Entity<InputState>,
    rail: Rail,
    tabs: Vec<Tab>,
    active: usize,
    next_query: usize,
    palette_open: bool,
    palette_input: Entity<InputState>,
    status: Option<(SharedString, bool)>,
}

impl ThaanApp {
    pub fn new(engine: Arc<Engine>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let drivers = engine.drivers();
        // Invariant: main() registers at least one driver.
        let driver = drivers.first().cloned().expect("at least one driver is registered");
        let conn_params = Self::param_inputs(&driver, window, cx);
        let conn_name = cx.new(|cx| InputState::new(window, cx).placeholder("My database"));
        let conn_url = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("postgres://user@host/db   mysql://…   mongodb://…   sqlite:/path/app.db")
        });
        let tree_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let palette_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search commands"));
        cx.subscribe(&tree_filter, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        cx.subscribe(&palette_input, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let connections = engine.connections();
        Self {
            engine,
            focus: cx.focus_handle(),
            drivers,
            driver,
            conn_name,
            conn_url,
            conn_params,
            conn_read_only: false,
            connections,
            session: None,
            objects: Vec::new(),
            tree_filter,
            rail: Rail::Objects,
            tabs: Vec::new(),
            active: 0,
            next_query: 1,
            palette_open: false,
            palette_input,
            status: None,
        }
    }

    /// One input per driver parameter (the form is generated from `DriverInfo::params`).
    fn param_inputs(
        driver: &DriverInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<(String, ParamKind, Entity<InputState>)> {
        driver
            .params
            .iter()
            .map(|p| {
                let ph = p.placeholder.clone().unwrap_or_default();
                let st = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder(ph);
                    if p.kind == ParamKind::Password {
                        s = s.masked(true);
                    }
                    s
                });
                if let Some(d) = &p.default {
                    st.update(cx, |s, cx| s.set_value(d.clone(), window, cx));
                }
                (p.key.clone(), p.kind, st)
            })
            .collect()
    }

    fn select_driver(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.drivers.get(ix).cloned() {
            self.conn_params = Self::param_inputs(&d, window, cx);
            self.driver = d;
            cx.notify();
        }
    }

    fn is_mongo(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.driver == "mongodb")
    }

    fn set_status(&mut self, msg: impl Into<SharedString>, error: bool, cx: &mut Context<Self>) {
        self.status = Some((msg.into(), error));
        cx.notify();
    }

    // ---- connections ------------------------------------------------------------------

    fn form_config(&self, cx: &App) -> Result<ConnectionConfig, String> {
        let url = self.conn_url.read(cx).value().trim().to_string();
        let mut c = if !url.is_empty() {
            ConnectionConfig::from_url(&url).map_err(|e| e.to_string())?
        } else {
            let mut params = BTreeMap::new();
            for (k, _, st) in &self.conn_params {
                let v = st.read(cx).value().trim().to_string();
                if !v.is_empty() {
                    params.insert(k.clone(), v);
                }
            }
            ConnectionConfig {
                driver: self.driver.id.clone(),
                params,
                ..Default::default()
            }
        };
        let name = self.conn_name.read(cx).value().trim().to_string();
        if !name.is_empty() {
            c.name = name;
        }
        if c.name.is_empty() {
            // A file path names the connection by its file name, not the whole path.
            c.name = c
                .param("path")
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string())
                })
                .or(c.param("database").map(String::from))
                .unwrap_or_else(|| "connection".into());
        }
        c.read_only = self.conn_read_only;
        Ok(c)
    }

    fn connect(&mut self, c: ConnectionConfig, window: &mut Window, cx: &mut Context<Self>) {
        match self.engine.open(c) {
            Ok(info) => {
                self.session = Some(info);
                self.tabs.clear();
                self.refresh_schema(cx);
                self.new_query_tab(None, window, cx);
                self.set_status("Connected", false, cx);
            }
            Err(e) => self.set_status(format!("Connection failed: {e}"), true, cx),
        }
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.session.take() {
            self.engine.close(s.session_id).ok();
        }
        self.tabs.clear();
        self.objects.clear();
        self.connections = self.engine.connections();
        self.status = None;
        cx.notify();
    }

    fn refresh_schema(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        match self.engine.list_objects(s.session_id) {
            Ok(o) => self.objects = o,
            Err(e) => self.set_status(format!("Schema: {e}"), true, cx),
        }
        cx.notify();
    }

    // ---- tabs -------------------------------------------------------------------------

    fn new_grid(window: &mut Window, cx: &mut Context<Self>) -> Entity<TableState<ResultGrid>> {
        cx.new(|cx| {
            TableState::new(ResultGrid::new(ResultSet::default()), window, cx).sortable(true)
        })
    }

    fn new_query_tab(
        &mut self,
        saved: Option<SavedQuery>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = if self.is_mongo() { "javascript" } else { "sql" };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
        });
        let (title, saved_id) = match saved {
            Some(q) => {
                editor.update(cx, |e, cx| e.set_value(q.sql.clone(), window, cx));
                (q.name.into(), Some(q.id))
            }
            None => {
                let t = format!("Query {}", self.next_query);
                self.next_query += 1;
                (t.into(), None)
            }
        };
        let grid = Self::new_grid(window, cx);
        self.tabs.push(Tab::Query(QueryTab {
            title,
            saved_id,
            editor: editor.clone(),
            results: Vec::new(),
            active_result: 0,
            grid,
            running: false,
        }));
        self.active = self.tabs.len() - 1;
        editor.update(cx, |e, cx| e.focus(window, cx));
        cx.notify();
    }

    fn open_table(&mut self, object: ObjectRef, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| matches!(t, Tab::Table(tt) if tt.object == object))
        {
            self.active = i;
            cx.notify();
            return;
        }
        let hint = if self.is_mongo() {
            "Query document, e.g. { age: { $gt: 30 } }"
        } else {
            "WHERE clause, e.g. age > 30"
        };
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
        cx.subscribe_in(&filter, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                    t.page_ix = 0;
                }
                this.load_page(cx);
            }
        })
        .detach();
        let info = self
            .session
            .as_ref()
            .and_then(|s| self.engine.describe_table(s.session_id, &object).ok());
        self.tabs.push(Tab::Table(Box::new(TableTab {
            object,
            info,
            ddl: None,
            filter,
            page: None,
            page_ix: 0,
            sort: Vec::new(),
            view: TableView::Data,
            grid: Self::new_grid(window, cx),
            error: None,
            loading: false,
        })));
        self.active = self.tabs.len() - 1;
        self.load_page(cx);
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.tabs.remove(ix);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
            cx.notify();
        }
    }

    // ---- running queries -------------------------------------------------------------

    fn run(&mut self, current_only: bool, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let sid = session.session_id;
        let ix = self.active;
        let Some(Tab::Query(q)) = self.tabs.get_mut(ix) else {
            return;
        };
        if q.running {
            return;
        }
        let ed = q.editor.read(cx);
        let sql = ed.value().to_string();
        let sel = ed.selected_range();
        let scope = if !sel.is_empty() {
            RunScope::Range {
                start: sel.start,
                end: sel.end,
            }
        } else if current_only {
            RunScope::Current {
                cursor: ed.cursor(),
            }
        } else {
            RunScope::All
        };
        q.running = true;
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
                let Some(Tab::Query(q)) = this.tabs.get_mut(ix) else {
                    return;
                };
                q.running = false;
                match res {
                    Ok(results) => {
                        let last_ok = results.iter().rposition(|r| {
                            r.result.as_ref().is_some_and(|r| !r.columns.is_empty())
                        });
                        q.active_result = last_ok.unwrap_or(results.len().saturating_sub(1));
                        let msg = summarize(&results);
                        q.results = results;
                        let rs = q
                            .results
                            .get(q.active_result)
                            .and_then(|r| r.result.clone())
                            .unwrap_or_default();
                        q.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(rs);
                            g.refresh(cx);
                        });
                        let failed = q.results.iter().any(|r| r.error.is_some());
                        // DDL may have changed the schema.
                        if q.results.iter().any(|r| is_ddl(&r.sql)) {
                            this.refresh_schema(cx);
                        }
                        this.set_status(msg, failed, cx);
                    }
                    Err(e) => this.set_status(e.to_string(), true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_result(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(Tab::Query(q)) = self.tabs.get_mut(self.active) {
            q.active_result = i;
            let rs = q
                .results
                .get(i)
                .and_then(|r| r.result.clone())
                .unwrap_or_default();
            q.grid.update(cx, |g, cx| {
                g.delegate_mut().set(rs);
                g.refresh(cx);
            });
            cx.notify();
        }
    }

    fn load_page(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let sid = session.session_id;
        let ix = self.active;
        let Some(Tab::Table(t)) = self.tabs.get_mut(ix) else {
            return;
        };
        let raw = t.filter.read(cx).value().trim().to_string();
        let req = PageRequest {
            table: t.object.clone(),
            filters: vec![],
            raw_where: (!raw.is_empty()).then_some(raw),
            sort: t.sort.clone(),
            page: t.page_ix,
            page_size: 100,
        };
        t.loading = true;
        cx.notify();
        let engine = self.engine.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move { engine.fetch_page(sid, &req) })
                .await;
            this.update(cx, |this, cx| {
                let Some(Tab::Table(t)) = this.tabs.get_mut(ix) else {
                    return;
                };
                t.loading = false;
                match res {
                    Ok(page) => {
                        t.error = None;
                        let rs = page.result.clone();
                        t.page = Some(page);
                        t.grid.update(cx, |g, cx| {
                            g.delegate_mut().set(rs);
                            g.refresh(cx);
                        });
                    }
                    Err(e) => t.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current_result(&self) -> Option<ResultSet> {
        match self.tabs.get(self.active)? {
            Tab::Query(q) => q.results.get(q.active_result)?.result.clone(),
            Tab::Table(t) => t.page.as_ref().map(|p| p.result.clone()),
        }
    }

    fn copy_result(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let (Some(s), Some(rs)) = (&self.session, self.current_result()) else {
            return;
        };
        let mut opts = ExportOptions::new(format);
        if let Some(Tab::Table(t)) = self.tabs.get(self.active) {
            opts.table_name = Some(t.object.name.clone());
        }
        match self.engine.export_result(s.session_id, &rs, &opts) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status(
                    format!(
                        "Copied {} as {}",
                        plural(rs.rows.len() as u64, "row"),
                        format.label()
                    ),
                    false,
                    cx,
                );
            }
            Err(e) => self.set_status(e.to_string(), true, cx),
        }
    }

    fn export_table_to_file(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let Some(s) = &self.session else { return };
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return;
        };
        let raw = t.filter.read(cx).value().trim().to_string();
        let req = PageRequest {
            table: t.object.clone(),
            filters: vec![],
            raw_where: (!raw.is_empty()).then_some(raw),
            sort: t.sort.clone(),
            page: 0,
            page_size: 100,
        };
        let dir = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let path = dir.join(format!("{}.{}", t.object.name, format.extension()));
        let r = self
            .engine
            .export_table(s.session_id, &req, &ExportOptions::new(format))
            .and_then(|text| std::fs::write(&path, text).map_err(Into::into));
        match r {
            Ok(()) => self.set_status(format!("Exported to {}", path.display()), false, cx),
            Err(e) => self.set_status(e.to_string(), true, cx),
        }
    }

    fn save_current_query(&mut self, cx: &mut Context<Self>) {
        let Some(Tab::Query(q)) = self.tabs.get_mut(self.active) else {
            return;
        };
        let sql = q.editor.read(cx).value().to_string();
        let saved = SavedQuery {
            id: q.saved_id.clone().unwrap_or_default(),
            name: q.title.to_string(),
            folder: None,
            sql,
            connection_id: self.session.as_ref().map(|s| s.connection_id.clone()),
            updated_ms: 0,
        };
        match self.engine.save_query(saved) {
            Ok(s) => {
                q.saved_id = Some(s.id);
                self.set_status(format!("Saved “{}”", s.name), false, cx);
            }
            Err(e) => self.set_status(e.to_string(), true, cx),
        }
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = !self.palette_open;
        if self.palette_open {
            self.palette_input.update(cx, |s, cx| {
                s.set_value("", window, cx);
                s.focus(window, cx);
            });
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn run_palette_command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        match id {
            "query.runAll" => self.run(false, cx),
            "query.runCurrent" | "query.runSelection" => self.run(true, cx),
            "tab.new" => self.new_query_tab(None, window, cx),
            "tab.close" => self.close_tab(self.active, cx),
            "schema.refresh" | "schema.listObjects" => self.refresh_schema(cx),
            "connection.close" => self.disconnect(cx),
            "app.toggleTheme" => theme::toggle(window, cx),
            "savedQuery.save" => self.save_current_query(cx),
            "history.list" => self.rail = Rail::History,
            "savedQuery.list" => self.rail = Rail::Saved,
            "result.export" => self.copy_result(ExportFormat::Csv, cx),
            "history.clear" => {
                self.engine.clear_history().ok();
            }
            other => self.set_status(
                format!("`{other}` is available from the CLI: thaan run {other}"),
                false,
                cx,
            ),
        }
        cx.notify();
    }
}

fn is_ddl(sql: &str) -> bool {
    let l = sql.trim_start().to_ascii_lowercase();
    ["create", "drop", "alter", "rename"]
        .iter()
        .any(|k| l.starts_with(k))
}

fn summarize(results: &[StatementResult]) -> String {
    if let Some(r) = results.iter().find(|r| r.error.is_some()) {
        return format!("Error: {}", r.error.as_deref().unwrap_or_default());
    }
    let ms: u64 = results
        .iter()
        .filter_map(|r| r.result.as_ref())
        .map(|r| r.elapsed_ms)
        .sum();
    let stmts = if results.len() > 1 {
        format!("{} · ", plural(results.len() as u64, "statement"))
    } else {
        String::new()
    };
    match results.last().and_then(|r| r.result.as_ref()) {
        Some(rs) if !rs.columns.is_empty() => {
            format!("{stmts}{} · {ms} ms", plural(rs.rows.len() as u64, "row"))
        }
        Some(rs) => format!(
            "{stmts}{} affected · {ms} ms",
            plural(rs.rows_affected.unwrap_or(0), "row")
        ),
        None => "No statements".into(),
    }
}

// ---- rendering ------------------------------------------------------------------------

impl ThaanApp {
    fn render_connect(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let saved = self.connections.clone();
        let form =
            v_flex()
                .gap_4()
                .w(px(440.))
                .p_6()
                .rounded(px(12.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .child(div().text_lg().font_medium().child("New connection"))
                .child(
                    h_flex().gap_1().flex_wrap().children(self.drivers.iter().enumerate().map(
                        |(i, d)| {
                            Button::new(("driver", i))
                                .small()
                                .label(d.name.clone())
                                .selected(d.id == self.driver.id)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_driver(i, window, cx)
                                }))
                        },
                    )),
                )
                .child(field(
                    "Name",
                    Input::new(&self.conn_name).into_any_element(),
                    &theme,
                ))
                .child(field(
                    "Connection URL",
                    Input::new(&self.conn_url).into_any_element(),
                    &theme,
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("Or fill in the {} fields", self.driver.name)),
                )
                .children(self.driver.params.iter().zip(&self.conn_params).map(
                    |(spec, (_, _, st))| {
                        field(&spec.label, Input::new(st).into_any_element(), &theme)
                    },
                ))
                .child(
                    Checkbox::new("ro")
                        .label("Read-only, refuse writes")
                        .checked(self.conn_read_only)
                        .on_click(cx.listener(|this, v: &bool, _, cx| {
                            this.conn_read_only = *v;
                            cx.notify();
                        })),
                )
                .child(
                    h_flex()
                        .mt_2()
                        .pt_4()
                        .border_t_1()
                        .border_color(theme.border)
                        .gap_2()
                        .justify_end()
                        .child(Button::new("test").label("Test").on_click(cx.listener(
                            |this, _, _, cx| match this.form_config(cx) {
                                Ok(c) => {
                                    let r = this.engine.test_connection(&c);
                                    if r.ok {
                                        this.set_status(
                                            format!("Connection OK ({} ms)", r.elapsed_ms),
                                            false,
                                            cx,
                                        );
                                    } else {
                                        this.set_status(r.error.unwrap_or_default(), true, cx);
                                    }
                                }
                                Err(e) => this.set_status(e, true, cx),
                            },
                        )))
                        .child(Button::new("save").label("Save").on_click(cx.listener(
                            |this, _, _, cx| {
                                match this.form_config(cx).map_err(|e| e.to_string()).and_then(
                                    |c| this.engine.save_connection(c).map_err(|e| e.to_string()),
                                ) {
                                    Ok(c) => {
                                        this.connections = this.engine.connections();
                                        this.set_status(format!("Saved “{}”", c.name), false, cx);
                                    }
                                    Err(e) => this.set_status(e, true, cx),
                                }
                            },
                        )))
                        .child(Button::new("connect").primary().label("Connect").on_click(
                            cx.listener(|this, _, window, cx| match this.form_config(cx) {
                                Ok(c) => this.connect(c, window, cx),
                                Err(e) => this.set_status(e, true, cx),
                            }),
                        )),
                );

        let count = saved.len();
        let list = v_flex()
            .w(px(280.))
            .pt_6()
            .gap_0p5()
            .child(
                group_label("Saved connections", (count > 0).then_some(count), &theme)
                    .px_2()
                    .pb_2(),
            )
            .when(saved.is_empty(), |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("None yet. Fill in the form and press Save."),
                )
            })
            .children(saved.into_iter().enumerate().map(|(i, c)| {
                let color = c
                    .color
                    .as_deref()
                    .and_then(parse_hex)
                    .unwrap_or(theme.primary);
                let subtitle = c
                    .param("path")
                    .or(c.param("host"))
                    .unwrap_or("")
                    .to_string();
                let id = c.id.clone();
                h_flex()
                    .id(("conn", i))
                    .gap_3()
                    .px_2()
                    .py_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.list_hover))
                    .child(conn_dot(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .overflow_hidden()
                            .gap_0p5()
                            .child(div().text_sm().font_medium().child(c.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(format!("{} · {subtitle}", c.driver)),
                            ),
                    )
                    .when(c.read_only, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Read-only"),
                        )
                    })
                    .child(
                        Button::new(("del", i))
                            .ghost()
                            .xsmall()
                            .icon(icon("trash"))
                            .tooltip("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.engine.delete_connection(&id).ok();
                                this.connections = this.engine.connections();
                                cx.stop_propagation();
                                cx.notify();
                            })),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.connect(c.clone(), window, cx)),
                    )
            }));

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .child(h_flex().gap_12().items_start().child(list).child(form))
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mongo = self.is_mongo();
        let theme = cx.theme().clone();
        let filter = self.tree_filter.read(cx).value().to_lowercase();
        let rail_btn = |id: &'static str,
                        ic: &'static str,
                        tip: &'static str,
                        r: Rail,
                        cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon(ic))
                .tooltip(tip)
                .selected(self.rail == r)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rail = r;
                    cx.notify();
                }))
        };
        let rail = v_flex()
            .w(px(40.))
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(rail_btn(
                "rail-obj",
                "database",
                "Objects",
                Rail::Objects,
                cx,
            ))
            .child(rail_btn(
                "rail-saved",
                "bookmark",
                "Saved queries",
                Rail::Saved,
                cx,
            ))
            .child(rail_btn("rail-hist", "clock", "History", Rail::History, cx))
            .child(div().flex_1())
            .child(
                Button::new("theme")
                    .ghost()
                    .small()
                    .icon(icon("sun-moon"))
                    .tooltip("Toggle theme")
                    .on_click(cx.listener(|_, _, window, cx| theme::toggle(window, cx))),
            )
            .child(
                Button::new("disc")
                    .ghost()
                    .small()
                    .icon(icon("unplug"))
                    .tooltip("Disconnect")
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
            );

        let content: AnyElement = match self.rail {
            Rail::Objects => {
                let mut groups: BTreeMap<ObjectKind, Vec<SchemaObject>> = BTreeMap::new();
                for o in &self.objects {
                    if filter.is_empty() || o.object.name.to_lowercase().contains(&filter) {
                        groups.entry(o.kind).or_default().push(o.clone());
                    }
                }
                let mut tree = v_flex()
                    .id("tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_0p5();
                for (gi, (kind, items)) in groups.into_iter().enumerate() {
                    tree = tree.child(
                        group_label(
                            if mongo && kind == ObjectKind::Table { "Collections" } else { kind.label_plural() },
                            Some(items.len()),
                            &theme,
                        )
                            .px_2()
                            .pt(px(if gi == 0 { 4. } else { 16. }))
                            .pb_1(),
                    );
                    let ic = match kind {
                        ObjectKind::Table => "table",
                        ObjectKind::View | ObjectKind::MaterializedView => "eye",
                        ObjectKind::Index => "list",
                        ObjectKind::Trigger => "zap",
                        _ => "code",
                    };
                    let openable = matches!(
                        kind,
                        ObjectKind::Table | ObjectKind::View | ObjectKind::MaterializedView
                    );
                    for (i, o) in items.into_iter().enumerate() {
                        let obj = o.object.clone();
                        let hover = theme.sidebar_accent;
                        tree = tree.child(
                            h_flex()
                                .id(SharedString::from(format!("obj-{kind:?}-{i}")))
                                .h(px(ROW_H))
                                .flex_none()
                                .gap_2()
                                .px_2()
                                .rounded(px(6.))
                                .text_sm()
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .child(icon(ic).size_4().text_color(theme.muted_foreground))
                                .child(div().flex_1().truncate().child(o.object.name.clone()))
                                .when_some(o.parent.clone(), |d, p| {
                                    d.child(
                                        div()
                                            .text_xs()
                                            .font_family(theme.mono_font_family.clone())
                                            .text_color(theme.muted_foreground)
                                            .child(p),
                                    )
                                })
                                .when(openable, |d| {
                                    d.on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_table(obj.clone(), window, cx)
                                    }))
                                }),
                        );
                    }
                }
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .px_2()
                            .pt_2()
                            .pb_1()
                            .gap_1()
                            .child(div().flex_1().child(Input::new(&self.tree_filter).small()))
                            .child(
                                Button::new("refresh")
                                    .ghost()
                                    .xsmall()
                                    .icon(icon("refresh-cw"))
                                    .tooltip("Refresh (⌘R)")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_schema(cx)),
                                    ),
                            ),
                    )
                    .child(tree)
                    .into_any_element()
            }
            Rail::Saved => {
                let items = self.engine.saved_queries();
                list_panel(
                    "Saved queries",
                    items.len(),
                    "Save a query with the bookmark button in its tab",
                    &theme,
                )
                .children(items.into_iter().enumerate().map(|(i, q)| {
                    let q2 = q.clone();
                    let id = q.id.clone();
                    h_flex()
                        .id(("saved", i))
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.sidebar_accent))
                        .child(
                            icon("file-code")
                                .size_4()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .overflow_hidden()
                                .gap_0p5()
                                .child(div().text_sm().truncate().child(q.name.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .truncate()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(first_line(&q.sql)),
                                ),
                        )
                        .child(
                            Button::new(("saved-del", i))
                                .ghost()
                                .xsmall()
                                .icon(icon("trash"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.engine.delete_saved_query(&id).ok();
                                    cx.stop_propagation();
                                    cx.notify();
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.new_query_tab(Some(q2.clone()), window, cx)
                        }))
                }))
                .into_any_element()
            }
            Rail::History => {
                let items: Vec<HistoryEntry> = self.engine.history(200);
                list_panel(
                    "History",
                    items.len(),
                    "Queries you run appear here",
                    &theme,
                )
                .children(items.into_iter().enumerate().map(|(i, h)| {
                    let sql = h.sql.clone();
                    let failed = h.error.is_some();
                    h_flex()
                        .id(("hist", i))
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.sidebar_accent))
                        .child(
                            div()
                                .size_4()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(6.)).rounded_full().bg(if failed {
                                    theme.danger
                                } else {
                                    theme.muted_foreground.opacity(0.5)
                                })),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .overflow_hidden()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_sm()
                                        .truncate()
                                        .font_family(theme.mono_font_family.clone())
                                        .child(first_line(&h.sql)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(if failed {
                                            "Failed".to_string()
                                        } else {
                                            format!(
                                                "{} · {} ms",
                                                plural(h.rows.unwrap_or(0), "row"),
                                                h.elapsed_ms
                                            )
                                        }),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let q = SavedQuery {
                                id: String::new(),
                                name: "From history".into(),
                                folder: None,
                                sql: sql.clone(),
                                connection_id: None,
                                updated_ms: 0,
                            };
                            this.new_query_tab(Some(q), window, cx);
                            if let Some(Tab::Query(t)) = this.tabs.last_mut() {
                                t.saved_id = None;
                            }
                        }))
                }))
                .into_any_element()
            }
        };

        h_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(rail)
            .child(div().flex_1().h_full().overflow_hidden().child(content))
    }

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .id("tabs")
            .h(px(36.))
            .w_full()
            .overflow_x_scroll()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                let active = i == self.active;
                let ic = if matches!(t, Tab::Query(_)) {
                    "file-code"
                } else {
                    "table"
                };
                h_flex()
                    .id(("tab", i))
                    .h_full()
                    .gap_2()
                    .pl_3()
                    .pr_1()
                    .text_sm()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(theme.background)
                            .border_b_2()
                            .border_color(theme.primary)
                    })
                    .when(!active, |d| {
                        d.text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.background.opacity(0.5)))
                    })
                    .child(icon(ic).size_3p5())
                    .child(t.title())
                    .child(
                        Button::new(("close", i))
                            .ghost()
                            .xsmall()
                            .icon(icon("x"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = i;
                        cx.notify();
                    }))
            }))
            .child(
                Button::new("new-tab")
                    .ghost()
                    .small()
                    .icon(icon("plus"))
                    .tooltip("New query (⌘T)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.new_query_tab(None, window, cx)),
                    ),
            )
    }

    fn render_results_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_1()
            .child(div().pr_1().text_xs().text_color(muted).child("Copy as"))
            .child(
                Button::new("copy-csv")
                    .ghost()
                    .xsmall()
                    .label("CSV")
                    .tooltip("Copy as CSV")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::Csv, cx)),
                    ),
            )
            .child(
                Button::new("copy-json")
                    .ghost()
                    .xsmall()
                    .label("JSON")
                    .tooltip("Copy as JSON")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::Json, cx)),
                    ),
            )
            .child(
                Button::new("copy-md")
                    .ghost()
                    .xsmall()
                    .label("MD")
                    .tooltip("Copy as Markdown")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::Markdown, cx)),
                    ),
            )
            .child(
                Button::new("copy-sql")
                    .ghost()
                    .xsmall()
                    .label("SQL")
                    .tooltip("Copy as INSERT statements")
                    .on_click(
                        cx.listener(|this, _, _, cx| this.copy_result(ExportFormat::SqlInsert, cx)),
                    ),
            )
    }

    fn render_query_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(Tab::Query(q)) = self.tabs.get(self.active) else {
            return div().into_any_element();
        };
        let running = q.running;
        let toolbar = h_flex()
            .h(px(36.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("run")
                    .primary()
                    .small()
                    .icon(icon("play"))
                    .label(if running { "Running…" } else { "Run" })
                    .tooltip("Run current statement or selection (⌘↵)")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.run(true, cx))),
            )
            .child(
                Button::new("run-all")
                    .small()
                    .label("Run all")
                    .tooltip("Run every statement (⌘⇧↵)")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.run(false, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("save-q")
                    .ghost()
                    .small()
                    .icon(icon("bookmark"))
                    .tooltip("Save query")
                    .on_click(cx.listener(|this, _, _, cx| this.save_current_query(cx))),
            );

        let result_tabs = h_flex()
            .gap_1()
            .children(q.results.iter().enumerate().map(|(i, r)| {
                let label = match (&r.result, &r.error) {
                    (_, Some(_)) => format!("{} · Error", i + 1),
                    (Some(rs), _) if !rs.columns.is_empty() => {
                        format!("{} · {}", i + 1, plural(rs.rows.len() as u64, "row"))
                    }
                    (Some(rs), _) => format!(
                        "{} · {} affected",
                        i + 1,
                        plural(rs.rows_affected.unwrap_or(0), "row")
                    ),
                    _ => format!("{}", i + 1),
                };
                Button::new(("res", i))
                    .xsmall()
                    .ghost()
                    .label(label)
                    .selected(i == q.active_result)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_result(i, cx)))
            }));

        let active = q.results.get(q.active_result);
        let body: AnyElement = match active {
            None => empty_state("Run a query to see results here", &theme),
            Some(r) if r.error.is_some() => div()
                .p_4()
                .text_sm()
                .text_color(theme.danger)
                .font_family(theme.mono_font_family.clone())
                .child(r.error.clone().unwrap_or_default())
                .into_any_element(),
            Some(r) if r.result.as_ref().is_some_and(|rs| rs.columns.is_empty()) => empty_state(
                &format!(
                    "{} affected",
                    plural(
                        r.result.as_ref().and_then(|x| x.rows_affected).unwrap_or(0),
                        "row"
                    )
                ),
                &theme,
            ),
            Some(_) => DataTable::new(&q.grid)
                .stripe(true)
                .bordered(false)
                .into_any_element(),
        };

        v_resizable("query-split")
            .child(
                resizable_panel().size(px(260.)).child(
                    v_flex().size_full().child(toolbar).child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(Editor::new(&q.editor).bordered(false).h_full()),
                    ),
                ),
            )
            .child(
                resizable_panel().child(
                    v_flex()
                        .size_full()
                        .child(
                            h_flex()
                                .h(px(36.))
                                .px_2()
                                .gap_2()
                                .border_b_1()
                                .border_color(theme.border)
                                .bg(theme.sidebar)
                                .child(result_tabs)
                                .child(div().flex_1())
                                .child(self.render_results_toolbar(cx)),
                        )
                        .child(div().flex_1().overflow_hidden().child(body)),
                ),
            )
            .into_any_element()
    }

    fn render_table_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(Tab::Table(t)) = self.tabs.get(self.active) else {
            return div().into_any_element();
        };
        let view = t.view;
        let seg = |id: &'static str, label: &'static str, v: TableView, cx: &mut Context<Self>| {
            Button::new(id)
                .xsmall()
                .ghost()
                .label(label)
                .selected(view == v)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                        t.view = v;
                        if v == TableView::Structure && t.ddl.is_none() {
                            if let Some(s) = &this.session {
                                t.ddl = this.engine.ddl(s.session_id, &t.object).ok();
                            }
                        }
                    }
                    cx.notify();
                }))
        };
        let header = h_flex()
            .h(px(36.))
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(seg("v-data", "Data", TableView::Data, cx))
            .child(seg("v-struct", "Structure", TableView::Structure, cx))
            .when(view == TableView::Data, |d| {
                d.child(div().flex_1().child(Input::new(&t.filter).small()))
                    .child(
                        Button::new("apply-filter")
                            .small()
                            .label("Apply")
                            .tooltip("Apply the WHERE clause (↵)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                    t.page_ix = 0;
                                }
                                this.load_page(cx);
                            })),
                    )
            })
            .when(view == TableView::Structure, |d| d.child(div().flex_1()));

        let body: AnyElement = match view {
            TableView::Data => {
                let pager = t
                    .page
                    .as_ref()
                    .map(|p| (p.page, p.page_count(), p.total_rows, p.result.elapsed_ms));
                let mono = theme.mono_font_family.clone();
                let footer = h_flex()
                    .h(px(36.))
                    .px_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when_some(pager, |d, (page, pages, total, ms)| {
                        d.child(
                            Button::new("prev")
                                .ghost()
                                .xsmall()
                                .icon(icon("chevron-left"))
                                .disabled(page == 0)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                        t.page_ix = t.page_ix.saturating_sub(1);
                                    }
                                    this.load_page(cx);
                                })),
                        )
                        .child(div().font_family(mono.clone()).child(format!(
                            "{} / {}",
                            page + 1,
                            pages
                        )))
                        .child(
                            Button::new("next")
                                .ghost()
                                .xsmall()
                                .icon(icon("chevron-right"))
                                .disabled(page + 1 >= pages)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(Tab::Table(t)) = this.tabs.get_mut(this.active) {
                                        t.page_ix += 1;
                                    }
                                    this.load_page(cx);
                                })),
                        )
                        .child(
                            div()
                                .pl_2()
                                .font_family(mono.clone())
                                .child(format!("{} · {ms} ms", plural(total, "row"))),
                        )
                    })
                    .child(div().flex_1())
                    .child(div().pr_1().child("Export"))
                    .child(
                        Button::new("export-csv")
                            .ghost()
                            .xsmall()
                            .label("CSV")
                            .tooltip("Whole table, with filter, to ~/Downloads")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_table_to_file(ExportFormat::Csv, cx)
                            })),
                    )
                    .child(
                        Button::new("export-json")
                            .ghost()
                            .xsmall()
                            .label("JSON")
                            .tooltip("Whole table, with filter, to ~/Downloads")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export_table_to_file(ExportFormat::Json, cx)
                            })),
                    )
                    .child(div().mx_2().w(px(1.)).h(px(16.)).bg(theme.border))
                    .child(self.render_results_toolbar(cx));
                let grid: AnyElement = match &t.error {
                    Some(e) => div()
                        .p_4()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(e.clone())
                        .into_any_element(),
                    None => DataTable::new(&t.grid)
                        .stripe(true)
                        .bordered(false)
                        .into_any_element(),
                };
                v_flex()
                    .size_full()
                    .child(div().flex_1().overflow_hidden().child(grid))
                    .child(footer)
                    .into_any_element()
            }
            TableView::Structure => {
                let info = t.info.clone();
                let ddl = t.ddl.clone().unwrap_or_default();
                let mono = theme.mono_font_family.clone();
                // Each section is its label plus its content, 8 apart; sections sit 32 apart.
                let section = |title: &str, n: Option<usize>, body: AnyElement| {
                    v_flex()
                        .gap_2()
                        .child(group_label(title, n, &theme))
                        .child(body)
                };
                v_flex()
                    .id("structure")
                    .size_full()
                    .overflow_y_scroll()
                    .p_6()
                    .gap_8()
                    .when_some(info, |d, info| {
                        d.child(section(
                            "Columns",
                            Some(info.columns.len()),
                            v_flex()
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.border)
                                .rounded(px(8.))
                                .child(struct_row(
                                    ["#", "Name", "Type", "Nullable", "Default", "Key"]
                                        .map(String::from),
                                    true,
                                    &theme,
                                ))
                                .children(info.columns.iter().enumerate().map(|(i, c)| {
                                    struct_row(
                                        [
                                            (i + 1).to_string(),
                                            c.name.clone(),
                                            c.data_type.clone(),
                                            if c.nullable {
                                                "Yes".into()
                                            } else {
                                                "No".into()
                                            },
                                            c.default.clone().unwrap_or_default(),
                                            c.primary_key
                                                .map(|p| format!("PK {p}"))
                                                .unwrap_or_default(),
                                        ],
                                        false,
                                        &theme,
                                    )
                                }))
                                .into_any_element(),
                        ))
                        .when(!info.indexes.is_empty(), |d| {
                            d.child(section(
                                "Indexes",
                                Some(info.indexes.len()),
                                v_flex()
                                    .gap_1()
                                    .children(info.indexes.iter().map(|ix| {
                                        div().text_sm().font_family(mono.clone()).child(format!(
                                            "{}{} ({})",
                                            ix.name,
                                            if ix.unique { " UNIQUE" } else { "" },
                                            ix.columns.join(", ")
                                        ))
                                    }))
                                    .into_any_element(),
                            ))
                        })
                        .when(!info.foreign_keys.is_empty(), |d| {
                            d.child(section(
                                "Foreign keys",
                                Some(info.foreign_keys.len()),
                                v_flex()
                                    .gap_1()
                                    .children(info.foreign_keys.iter().map(|fk| {
                                        div().text_sm().font_family(mono.clone()).child(format!(
                                            "({}) → {} ({})",
                                            fk.columns.join(", "),
                                            fk.ref_table,
                                            fk.ref_columns.join(", ")
                                        ))
                                    }))
                                    .into_any_element(),
                            ))
                        })
                    })
                    .child(section(
                        "DDL",
                        None,
                        div()
                            .p_4()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.sidebar)
                            .text_sm()
                            .font_family(mono.clone())
                            .child(ddl)
                            .into_any_element(),
                    ))
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().overflow_hidden().child(body))
            .into_any_element()
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let q = self.palette_input.read(cx).value().to_lowercase();
        let cmds: Vec<_> = builtin_commands()
            .into_iter()
            .filter(|c| {
                q.is_empty()
                    || c.title.to_lowercase().contains(&q)
                    || c.id.to_lowercase().contains(&q)
            })
            .take(12)
            .collect();
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(80.))
            .bg(theme.overlay)
            .child(
                v_flex()
                    .id("palette")
                    .w(px(560.))
                    .max_h(px(440.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .p_2()
                    .gap_0p5()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.palette_open = false;
                        cx.notify();
                    }))
                    .child(div().pb_1().child(Input::new(&self.palette_input)))
                    .children(cmds.into_iter().enumerate().map(|(i, c)| {
                        let id = c.id;
                        h_flex()
                            .id(("cmd", i))
                            .flex_none()
                            .h(px(32.))
                            .px_2()
                            .gap_3()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .child(div().flex_1().text_sm().truncate().child(c.title))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(c.category),
                            )
                            .when_some(c.shortcut, |d, s| d.child(keycap(s, &theme)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_palette_command(id, window, cx)
                            }))
                    })),
            )
    }
}

impl Render for ThaanApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let root = v_flex()
            .key_context("ThaanApp")
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &RunCurrent, _, cx| this.run(true, cx)))
            .on_action(cx.listener(|this, _: &RunAll, _, cx| this.run(false, cx)))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| {
                if this.session.is_some() {
                    this.new_query_tab(None, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_tab(this.active, cx)))
            .on_action(cx.listener(|this, _: &RefreshSchema, _, cx| this.refresh_schema(cx)))
            .on_action(cx.listener(|this, _: &Disconnect, _, cx| this.disconnect(cx)))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| theme::toggle(window, cx)))
            .on_action(
                cx.listener(|this, _: &TogglePalette, window, cx| this.toggle_palette(window, cx)),
            );

        // Title bar (we keep the native one and draw a slim header below it).
        let title = match &self.session {
            Some(s) => {
                let color = s
                    .color
                    .as_deref()
                    .and_then(parse_hex)
                    .unwrap_or(theme.primary);
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(conn_dot(color))
                    .child(div().font_medium().child(s.name.clone()))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(s.driver.clone()),
                    )
                    .when(s.read_only, |d| {
                        d.child(
                            h_flex()
                                .gap_1()
                                .text_color(theme.muted_foreground)
                                .child(icon("lock").size_3p5())
                                .child("Read-only"),
                        )
                    })
            }
            None => h_flex()
                .gap_2()
                .items_center()
                .child(crate::logo::mark(theme.primary, theme.foreground))
                .child(div().font_medium().child("ThaanDB")),
        };
        let mod_key = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        };
        let header = h_flex()
            .h(px(36.))
            .px_3()
            .gap_3()
            .text_sm()
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.title_bar_border)
            .child(title)
            .child(div().flex_1())
            .child(
                h_flex()
                    .id("palette")
                    .h(px(24.))
                    .px_2()
                    .gap_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(div().pr_1().child("Commands"))
                    .child(keycap(mod_key, &theme))
                    .child(keycap("K", &theme))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_palette(window, cx))),
            );

        let status = h_flex()
            .h(px(24.))
            .px_3()
            .text_xs()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .when_some(self.status.clone(), |d, (msg, err)| {
                d.child(
                    div()
                        .truncate()
                        .text_color(if err {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(msg),
                )
            });

        let main: AnyElement = if self.session.is_none() {
            self.render_connect(window, cx).into_any_element()
        } else {
            let content = if self.tabs.is_empty() {
                empty_state(
                    "Open a table from the sidebar or press ⌘T for a new query",
                    &theme,
                )
            } else if matches!(self.tabs.get(self.active), Some(Tab::Query(_))) {
                self.render_query_tab(cx)
            } else {
                self.render_table_tab(cx)
            };
            h_resizable("main-split")
                .child(
                    resizable_panel()
                        .size(px(270.))
                        .size_range(px(180.)..px(520.))
                        .child(self.render_sidebar(cx)),
                )
                .child(
                    resizable_panel().child(
                        v_flex()
                            .size_full()
                            .child(self.render_tabs(cx))
                            .child(div().flex_1().overflow_hidden().child(content)),
                    ),
                )
                .into_any_element()
        };

        root.child(header)
            .child(div().flex_1().overflow_hidden().child(main))
            .child(status)
            .when(self.palette_open, |d| d.child(self.render_palette(cx)))
    }
}

// ---- small view helpers ---------------------------------------------------------------

type ThemeRef = gpui_kit::component::Theme;

/// Height of every one-line row in the sidebar, the palette and the saved list.
const ROW_H: f32 = 28.;

fn field(label: &str, input: AnyElement, theme: &ThemeRef) -> impl IntoElement {
    v_flex()
        .gap_1p5()
        .child(
            div()
                .text_xs()
                .font_medium()
                .text_color(theme.secondary_foreground)
                .child(label.to_string()),
        )
        .child(input)
}

/// A group label in sentence case with an optional count in muted mono, e.g. `Tables 2`.
fn group_label(title: &str, count: Option<usize>, theme: &ThemeRef) -> Div {
    h_flex()
        .gap_1p5()
        .items_baseline()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(div().font_medium().child(title.to_string()))
        .when_some(count, |d, n| {
            d.child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child(n.to_string()),
            )
        })
}

/// A small keyboard key.
fn keycap(key: &str, theme: &ThemeRef) -> impl IntoElement {
    div()
        .min_w(px(18.))
        .h(px(18.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.5))
        .text_xs()
        .font_family(theme.mono_font_family.clone())
        .text_color(theme.muted_foreground)
        .child(key.to_string())
}

/// The connection's own colour as a dot: the one signature mark, drawn the same everywhere.
fn conn_dot(color: Hsla) -> impl IntoElement {
    div().flex_none().size(px(8.)).rounded_full().bg(color)
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn struct_row(cells: [String; 6], head: bool, theme: &ThemeRef) -> impl IntoElement {
    let widths = [40., 180., 140., 80., 160., 80.];
    h_flex()
        .h(px(32.))
        .text_sm()
        .border_b_1()
        .border_color(theme.border)
        .when(head, |d| {
            d.bg(theme.table_head)
                .text_xs()
                .font_medium()
                .text_color(theme.muted_foreground)
        })
        .children(
            cells
                .into_iter()
                .zip(widths)
                .enumerate()
                .map(|(i, (c, w))| {
                    div()
                        .w(px(w))
                        .px_3()
                        .truncate()
                        .when(i == 0, |d| {
                            d.font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                        })
                        .child(c)
                }),
        )
}

fn list_panel(title: &str, count: usize, hint: &str, theme: &ThemeRef) -> Stateful<Div> {
    v_flex()
        .id(SharedString::from(format!("panel-{title}")))
        .size_full()
        .overflow_y_scroll()
        .px_2()
        .pb_2()
        .gap_0p5()
        .child(
            group_label(title, (count > 0).then_some(count), theme)
                .px_2()
                .pt_3()
                .pb_1(),
        )
        .when(count == 0, |d| {
            d.child(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(hint.to_string()),
            )
        })
}

fn empty_state(text: &str, theme: &ThemeRef) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn parse_hex(s: &str) -> Option<Hsla> {
    let h = s.trim_start_matches('#');
    u32::from_str_radix(h, 16)
        .ok()
        .filter(|_| h.len() == 6)
        .map(|v| rgb(v).into())
}
